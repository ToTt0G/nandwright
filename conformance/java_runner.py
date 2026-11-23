"""Official Java Nand2Tetris tool runner and ground-truth JSON cache."""

from __future__ import annotations

import base64
import json
import os
import re
import shutil
import signal
import subprocess
import tempfile
import time
from dataclasses import asdict, dataclass
from pathlib import Path

from scenarios import TestCase

CMP_FAIL_RE = re.compile(r"Comparison failure at line (\d+)")
SCRIPT_ERR_RE = re.compile(r"In script .*?, Line (\d+),\s*(.+)")
HDL_ERR_RE = re.compile(r"In HDL file .*?, Line (\d+),\s*(.+)")

TOOL_BY_DIALECT = {
    "hardware": "HardwareSimulator.sh",
    "cpu": "CPUEmulator.sh",
    "vm": "VMEmulator.sh",
}


@dataclass
class JavaTruth:
    case_id: str
    scenario: str
    project: str
    dialect: str
    script: str
    tool: str
    exit_code: int | None
    stdout: str
    stderr: str
    out_file: str | None
    out_bytes_b64: str | None
    wall_time_s: float
    outcome: str  # "passed" | "comparison-failure" | "error" | "timeout"
    checked: bool  # True if comparison succeeded; False if "End of script" without compare
    compared_lines: int | None
    comparison_line: int | None
    error_message: str | None
    script_line: int | None
    hdl_line: int | None
    needs_keyboard: bool = False
    workaround: str | None = None
    notes: str | None = None
    mutation_kind: str | None = None
    source_chip: str | None = None

    def out_bytes(self) -> bytes | None:
        if self.out_bytes_b64 is None:
            return None
        return base64.b64decode(self.out_bytes_b64)

    def to_dict(self) -> dict:
        return asdict(self)

    @classmethod
    def from_dict(cls, data: dict) -> JavaTruth:
        return cls(**data)


def truth_cache_path(private_dir: Path, case_id: str) -> Path:
    safe = case_id.replace("/", "__").replace("\\", "__")
    scenario = case_id.split("/", 1)[0]
    return private_dir / "truth" / scenario / f"{safe}.json"


def _classify_java(
    exit_code: int | None,
    stdout: str,
    stderr: str,
    out_bytes: bytes | None,
    timed_out: bool,
) -> tuple[str, bool, int | None, int | None, str | None, int | None, int | None]:
    """Return (outcome, checked, compared_lines, comparison_line, error_msg, script_line, hdl_line)."""
    if timed_out:
        return ("timeout", False, None, None, "timed out", None, None)

    combined = "\n".join(part for part in (stdout.strip(), stderr.strip()) if part).strip()

    if exit_code == 0 and "Comparison ended successfully" in combined:
        compared = None
        if out_bytes is not None:
            text = out_bytes.decode("latin-1")
            compared = len(text.splitlines())
        return ("passed", True, compared, None, None, None, None)

    if exit_code == 0 and "End of script" in combined:
        return ("passed", False, None, None, None, None, None)

    cm = CMP_FAIL_RE.search(combined)
    if cm:
        return ("comparison-failure", False, None, int(cm.group(1)), combined, None, None)

    script_line = None
    hdl_line = None
    sm = SCRIPT_ERR_RE.search(combined)
    if sm:
        script_line = int(sm.group(1))
    hm = HDL_ERR_RE.search(combined)
    if hm:
        hdl_line = int(hm.group(1))

    err_msg = combined or f"Java tool exited with code {exit_code}"
    return ("error", False, None, None, err_msg, script_line, hdl_line)


def run_java_case(
    case: TestCase,
    course_dir: Path,
    *,
    timeout_s: float = 20.0,
) -> JavaTruth:
    """Execute one TestCase with the official Java tool in an isolated scratch copy."""
    tool_name = TOOL_BY_DIALECT[case.dialect]
    tool_sh = course_dir / "tools" / tool_name
    effective_timeout = min(timeout_s, 3.0) if case.needs_keyboard else timeout_s

    with tempfile.TemporaryDirectory(prefix="n2t_java_") as tmp:
        work_dir = Path(tmp) / "work"
        shutil.copytree(case.stage_dir, work_dir)
        script_path = work_dir / case.rel_script
        out_path = (work_dir / case.out_file_name) if case.out_file_name else None
        if out_path and out_path.exists():
            out_path.unlink()

        t0 = time.monotonic()
        timed_out = False
        exit_code: int | None = None
        stdout = ""
        stderr = ""

        proc = subprocess.Popen(
            [str(tool_sh), str(script_path)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            errors="replace",
            start_new_session=True,
        )
        try:
            stdout, stderr = proc.communicate(timeout=effective_timeout)
            exit_code = proc.returncode
        except subprocess.TimeoutExpired:
            timed_out = True
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            try:
                stdout, stderr = proc.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                stdout, stderr = proc.communicate()

        wall = round(time.monotonic() - t0, 4)

        out_bytes: bytes | None = None
        if out_path and out_path.exists():
            out_bytes = out_path.read_bytes()

    outcome, checked, compared_lines, cmp_line, err_msg, script_line, hdl_line = _classify_java(
        exit_code, stdout, stderr, out_bytes, timed_out
    )

    return JavaTruth(
        case_id=case.case_id,
        scenario=case.scenario,
        project=case.project,
        dialect=case.dialect,
        script=case.rel_script,
        tool=tool_name,
        exit_code=exit_code,
        stdout=stdout,
        stderr=stderr,
        out_file=case.out_file_name,
        out_bytes_b64=base64.b64encode(out_bytes).decode("ascii") if out_bytes is not None else None,
        wall_time_s=wall,
        outcome=outcome,
        checked=checked,
        compared_lines=compared_lines,
        comparison_line=cmp_line,
        error_message=err_msg,
        script_line=script_line,
        hdl_line=hdl_line,
        needs_keyboard=case.needs_keyboard,
        workaround=case.workaround,
        notes=case.notes,
        mutation_kind=case.mutation_kind,
        source_chip=case.source_chip,
    )


def load_or_capture_truth(
    case: TestCase,
    course_dir: Path,
    private_dir: Path,
    *,
    refresh: bool = False,
    timeout_s: float = 20.0,
) -> tuple[JavaTruth, bool]:
    """Return (JavaTruth, was_cached)."""
    cache_file = truth_cache_path(private_dir, case.case_id)
    if not refresh and cache_file.exists():
        data = json.loads(cache_file.read_text(encoding="utf-8"))
        return JavaTruth.from_dict(data), True

    truth = run_java_case(case, course_dir, timeout_s=timeout_s)
    cache_file.parent.mkdir(parents=True, exist_ok=True)
    cache_file.write_text(
        json.dumps(truth.to_dict(), indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    return truth, False
