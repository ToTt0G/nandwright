"""Single adapter function wrapping the `n2t` CLI for conformance testing.

Prefers `n2t test --json <script.tst>` when supported by the binary, falling back
to parsing the human output of `n2t test <script.tst>` and its exit code today.
"""

from __future__ import annotations

import base64
import json
import os
import re
import signal
import subprocess
import time
from dataclasses import asdict, dataclass
from functools import lru_cache
from pathlib import Path

ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")
PASS_RE = re.compile(r"^PASS\s+.+?\((\d+)\s+lines?\s+compared\)", re.MULTILINE)
DONE_RE = re.compile(r"^DONE\s+.+?\(no compare file\)", re.MULTILINE)
LIMIT_RE = re.compile(r"^FAIL\s+.+?:\s+exceeded\s+\d+\s+commands", re.MULTILINE)
CMP_FAIL_RE = re.compile(r"comparison failure at line (\d+)")
LOC_RE = re.compile(r"-->\s*(.+?):(\d+):(\d+)")
ERR_LINE_RE = re.compile(r"^error(?:\[[^\]]+\])?:\s*(.+)$", re.MULTILINE)

UNSUPPORTED_DIALECT_MARKERS = (
    "is a CPU emulator command and is not supported by the hardware simulator",
    "is a VM emulator command and is not supported by the hardware simulator",
    "`load` requires a file name ending in `.hdl`",
    "expected a `.hdl` file name after `load`",
)


@dataclass
class N2tResult:
    mode: str  # "json" | "human"
    dialect: str
    exit_code: int | None
    stdout: str
    stderr: str
    out_file: str | None
    out_bytes_b64: str | None
    wall_time_s: float
    outcome: str  # "passed" | "comparison-failure" | "error" | "timeout" | "not-supported"
    checked: bool
    compared_lines: int | None
    comparison_line: int | None
    error_message: str | None
    script_line: int | None

    def out_bytes(self) -> bytes | None:
        if self.out_bytes_b64 is None:
            return None
        return base64.b64decode(self.out_bytes_b64)

    def to_dict(self) -> dict:
        return asdict(self)


@lru_cache(maxsize=8)
def supports_json_mode(n2t_bin: str) -> bool:
    """Detect whether `n2t test --json` is implemented by this binary."""
    try:
        res = subprocess.run(
            [n2t_bin, "test", "--help"],
            capture_output=True,
            text=True,
            timeout=5,
        )
        return "--json" in (res.stdout + res.stderr)
    except (OSError, subprocess.SubprocessError):
        return False


def _strip_ansi(text: str) -> str:
    return ANSI_RE.sub("", text)


def _extract_script_line(combined: str, script_path: Path) -> int | None:
    script_name = script_path.name
    for m in LOC_RE.finditer(combined):
        loc_file = m.group(1).strip()
        if loc_file.endswith(script_name) or Path(loc_file).name == script_name:
            return int(m.group(2))
    return None


def _parse_json_output(
    stdout: str,
    stderr: str,
    exit_code: int | None,
    dialect: str,
    script_path: Path,
    default_out_name: str | None,
    wall: float,
) -> N2tResult | None:
    """Parse one JSON object from `n2t test --json <script.tst>` stdout."""
    obj = None
    for line in stdout.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            obj = json.loads(line)
            break
        except json.JSONDecodeError:
            continue
    if not isinstance(obj, dict) or "outcome" not in obj:
        return None

    raw_outcome = obj.get("outcome")
    reported_dialect = obj.get("dialect", dialect)
    out_file_str = obj.get("out_file")
    out_path: Path | None = None
    if out_file_str:
        p = Path(out_file_str)
        out_path = p if p.is_absolute() else (script_path.parent / p)
    elif default_out_name:
        out_path = script_path.parent / default_out_name

    out_bytes = out_path.read_bytes() if (out_path and out_path.exists()) else None
    out_name = out_path.name if out_path else default_out_name

    if raw_outcome == "passed":
        return N2tResult(
            mode="json",
            dialect=reported_dialect,
            exit_code=exit_code,
            stdout=stdout,
            stderr=stderr,
            out_file=out_name,
            out_bytes_b64=base64.b64encode(out_bytes).decode("ascii") if out_bytes is not None else None,
            wall_time_s=wall,
            outcome="passed",
            checked=True,
            compared_lines=obj.get("compared_lines"),
            comparison_line=None,
            error_message=None,
            script_line=None,
        )

    if raw_outcome == "unchecked":
        return N2tResult(
            mode="json",
            dialect=reported_dialect,
            exit_code=exit_code,
            stdout=stdout,
            stderr=stderr,
            out_file=out_name,
            out_bytes_b64=base64.b64encode(out_bytes).decode("ascii") if out_bytes is not None else None,
            wall_time_s=wall,
            outcome="passed",
            checked=False,
            compared_lines=None,
            comparison_line=None,
            error_message=None,
            script_line=None,
        )

    if raw_outcome == "failed":
        mismatch = obj.get("mismatch") or {}
        cmp_line = mismatch.get("line")
        msg = (
            f"comparison failure at line {cmp_line}"
            if cmp_line is not None
            else "comparison failure"
        )
        return N2tResult(
            mode="json",
            dialect=reported_dialect,
            exit_code=exit_code,
            stdout=stdout,
            stderr=stderr,
            out_file=out_name,
            out_bytes_b64=base64.b64encode(out_bytes).decode("ascii") if out_bytes is not None else None,
            wall_time_s=wall,
            outcome="comparison-failure",
            checked=False,
            compared_lines=None,
            comparison_line=cmp_line,
            error_message=msg,
            script_line=None,
        )

    if raw_outcome == "limit":
        return N2tResult(
            mode="json",
            dialect=reported_dialect,
            exit_code=exit_code,
            stdout=stdout,
            stderr=stderr,
            out_file=out_name,
            out_bytes_b64=base64.b64encode(out_bytes).decode("ascii") if out_bytes is not None else None,
            wall_time_s=wall,
            outcome="timeout",
            checked=False,
            compared_lines=None,
            comparison_line=None,
            error_message="command limit exceeded",
            script_line=None,
        )

    err = obj.get("error") or {}
    err_msg = err.get("message") or stderr.strip() or stdout.strip()
    err_line = err.get("line")
    if dialect in {"cpu", "vm"} and any(m in err_msg for m in UNSUPPORTED_DIALECT_MARKERS):
        outcome = "not-supported"
    else:
        outcome = "error"

    return N2tResult(
        mode="json",
        dialect=reported_dialect,
        exit_code=exit_code,
        stdout=stdout,
        stderr=stderr,
        out_file=out_name,
        out_bytes_b64=base64.b64encode(out_bytes).decode("ascii") if out_bytes is not None else None,
        wall_time_s=wall,
        outcome=outcome,
        checked=False,
        compared_lines=None,
        comparison_line=None,
        error_message=err_msg,
        script_line=err_line,
    )


def run_n2t(
    script_path: Path,
    *,
    n2t_bin: Path,
    dialect: str = "hardware",
    out_file_name: str | None = None,
    timeout_s: float = 20.0,
    max_commands: int = 10_000_000,
    keys: list[str] | None = None,
) -> N2tResult:
    """Single adapter entry point invoking `n2t test` on one `.tst` script.

    Automatically uses `n2t test --json` when supported by `n2t_bin`, otherwise
    parses human-readable CLI output.
    """
    use_json = supports_json_mode(str(n2t_bin))
    cmd = [str(n2t_bin), "test", "--max-commands", str(max_commands)]
    if use_json:
        cmd.append("--json")
    if keys:
        cmd.extend(["--keys", ",".join(keys)])
    cmd.append(str(script_path))

    out_path = (script_path.parent / out_file_name) if out_file_name else None
    if out_path and out_path.exists():
        out_path.unlink()

    t0 = time.monotonic()
    timed_out = False
    exit_code: int | None = None
    stdout = ""
    stderr = ""

    proc = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        errors="replace",
        start_new_session=True,
    )
    try:
        stdout, stderr = proc.communicate(timeout=timeout_s)
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

    if timed_out:
        out_bytes = out_path.read_bytes() if (out_path and out_path.exists()) else None
        return N2tResult(
            mode="json" if use_json else "human",
            dialect=dialect,
            exit_code=exit_code,
            stdout=stdout,
            stderr=stderr,
            out_file=out_file_name,
            out_bytes_b64=base64.b64encode(out_bytes).decode("ascii") if out_bytes is not None else None,
            wall_time_s=wall,
            outcome="timeout",
            checked=False,
            compared_lines=None,
            comparison_line=None,
            error_message="timed out",
            script_line=None,
        )

    if use_json:
        parsed = _parse_json_output(
            stdout, stderr, exit_code, dialect, script_path, out_file_name, wall
        )
        if parsed is not None:
            return parsed

    # Human output fallback
    clean_out = _strip_ansi(stdout)
    clean_err = _strip_ansi(stderr)
    combined = "\n".join(part for part in (clean_out, clean_err) if part).strip()
    out_bytes = out_path.read_bytes() if (out_path and out_path.exists()) else None
    out_b64 = base64.b64encode(out_bytes).decode("ascii") if out_bytes is not None else None

    if dialect in {"cpu", "vm"} and any(m in combined for m in UNSUPPORTED_DIALECT_MARKERS):
        return N2tResult(
            mode="human",
            dialect=dialect,
            exit_code=exit_code,
            stdout=clean_out,
            stderr=clean_err,
            out_file=out_file_name,
            out_bytes_b64=out_b64,
            wall_time_s=wall,
            outcome="not-supported",
            checked=False,
            compared_lines=None,
            comparison_line=None,
            error_message="dialect not supported yet by n2t",
            script_line=None,
        )

    pm = PASS_RE.search(clean_out)
    if exit_code == 0 and pm:
        return N2tResult(
            mode="human",
            dialect=dialect,
            exit_code=exit_code,
            stdout=clean_out,
            stderr=clean_err,
            out_file=out_file_name,
            out_bytes_b64=out_b64,
            wall_time_s=wall,
            outcome="passed",
            checked=True,
            compared_lines=int(pm.group(1)),
            comparison_line=None,
            error_message=None,
            script_line=None,
        )

    if exit_code == 0 and DONE_RE.search(clean_out):
        return N2tResult(
            mode="human",
            dialect=dialect,
            exit_code=exit_code,
            stdout=clean_out,
            stderr=clean_err,
            out_file=out_file_name,
            out_bytes_b64=out_b64,
            wall_time_s=wall,
            outcome="passed",
            checked=False,
            compared_lines=None,
            comparison_line=None,
            error_message=None,
            script_line=None,
        )

    if LIMIT_RE.search(clean_out):
        return N2tResult(
            mode="human",
            dialect=dialect,
            exit_code=exit_code,
            stdout=clean_out,
            stderr=clean_err,
            out_file=out_file_name,
            out_bytes_b64=out_b64,
            wall_time_s=wall,
            outcome="timeout",
            checked=False,
            compared_lines=None,
            comparison_line=None,
            error_message="command limit exceeded",
            script_line=None,
        )

    cm = CMP_FAIL_RE.search(combined)
    if cm:
        return N2tResult(
            mode="human",
            dialect=dialect,
            exit_code=exit_code,
            stdout=clean_out,
            stderr=clean_err,
            out_file=out_file_name,
            out_bytes_b64=out_b64,
            wall_time_s=wall,
            outcome="comparison-failure",
            checked=False,
            compared_lines=None,
            comparison_line=int(cm.group(1)),
            error_message=f"comparison failure at line {cm.group(1)}",
            script_line=None,
        )

    script_line = _extract_script_line(combined, script_path)
    err_lines = ERR_LINE_RE.findall(combined)
    if err_lines:
        # Include first indented detail line after `cannot load ...:` if present
        err_msg = err_lines[0].strip()
        if err_msg.startswith("cannot load"):
            lines = combined.splitlines()
            for idx, ln in enumerate(lines):
                if ln.startswith("error: cannot load") and idx + 1 < len(lines):
                    detail = lines[idx + 1].strip()
                    if detail and not detail.startswith("-->"):
                        err_msg = f"{err_msg} {detail}"
                    break
    else:
        err_msg = combined.splitlines()[0] if combined else f"n2t exited with code {exit_code}"

    return N2tResult(
        mode="human",
        dialect=dialect,
        exit_code=exit_code,
        stdout=clean_out,
        stderr=clean_err,
        out_file=out_file_name,
        out_bytes_b64=out_b64,
        wall_time_s=wall,
        outcome="error",
        checked=False,
        compared_lines=None,
        comparison_line=None,
        error_message=err_msg,
        script_line=script_line,
    )
