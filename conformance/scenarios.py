"""Scenario discovery and staged workspace preparation (outside the repo).

All staged workspaces live under N2T_PRIVATE_DIR/stage/ so no course files or
solutions ever enter the repository.
"""

from __future__ import annotations

import re
import shutil
import subprocess
from dataclasses import dataclass
from pathlib import Path

from mutator import Mutation, generate_mutations

OUTPUT_FILE_RE = re.compile(r"\boutput-file\s+([^\s,;]+)")
LOAD_CMD_RE = re.compile(r"\bload(?:\s+([^\s,;]+))?\s*[,;]")


@dataclass(frozen=True)
class TestCase:
    case_id: str
    scenario: str
    project: str
    dialect: str  # "hardware" | "cpu" | "vm"
    stage_dir: Path  # directory containing the script (and sibling project files)
    rel_script: str  # script filename or relative path inside stage_dir
    out_file_name: str | None
    needs_keyboard: bool = False
    workaround: str | None = None
    notes: str | None = None
    mutation_kind: str | None = None
    source_chip: str | None = None

    @property
    def script_path(self) -> Path:
        return self.stage_dir / self.rel_script


@dataclass(frozen=True)
class SkippedCase:
    case_id: str
    scenario: str
    project: str
    dialect: str
    rel_script: str
    reason: str


def _parse_output_file(tst_path: Path) -> str | None:
    text = tst_path.read_text(encoding="utf-8", errors="replace")
    # Strip // comments for reliable regex match
    cleaned = re.sub(r"//[^\n]*", "", text)
    m = OUTPUT_FILE_RE.search(cleaned)
    return m.group(1) if m else None


def _project_for_rel(rel: Path) -> str:
    parts = rel.parts
    if not parts:
        return "unknown"
    if parts[0] == "03" and len(parts) >= 2 and parts[1] in {"a", "b"}:
        return f"03/{parts[1]}"
    return parts[0]


def _detect_dialect(tst_path: Path) -> str:
    text = tst_path.read_text(encoding="utf-8", errors="replace")
    cleaned = re.sub(r"//[^\n]*", "", text)
    if re.search(r"\bvmstep\b", cleaned):
        return "vm"
    if re.search(r"\bticktock\b", cleaned):
        return "cpu"
    m = LOAD_CMD_RE.search(cleaned)
    if m:
        target = m.group(1) or ""
        if target.endswith(".vm") or target == "":
            return "vm"
        if target.endswith(".asm") or target.endswith(".hack"):
            return "cpu"
    return "hardware"


def _copy_tree_clean(src: Path, dst: Path) -> None:
    if dst.exists():
        shutil.rmtree(dst)
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copytree(src, dst)


def _overlay_tree(src: Path, dst: Path) -> None:
    for item in src.rglob("*"):
        if item.is_file():
            rel = item.relative_to(src)
            target = dst / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(item, target)


def _run_java_tool(tool_sh: Path, arg: Path, timeout_s: float = 30.0) -> None:
    res = subprocess.run(
        [str(tool_sh), str(arg)],
        capture_output=True,
        text=True,
        timeout=timeout_s,
    )
    if res.returncode != 0:
        raise RuntimeError(
            f"{tool_sh.name} failed on {arg} (rc={res.returncode}): "
            f"{res.stdout.strip()} {res.stderr.strip()}"
        )


def build_solved(course_dir: Path, private_dir: Path) -> list[TestCase]:
    """Stage course overlaid with private solutions and assemble project 04."""
    stage_root = private_dir / "stage" / "solved" / "projects"
    solutions_dir = private_dir / "solutions"
    _copy_tree_clean(course_dir / "projects", stage_root)
    if solutions_dir.exists():
        _overlay_tree(solutions_dir, stage_root)

    # Assemble Mult.asm and Fill.asm using the official Java Assembler
    assembler = course_dir / "tools" / "Assembler.sh"
    for asm_rel in ("04/mult/Mult.asm", "04/fill/Fill.asm"):
        asm_path = stage_root / asm_rel
        if asm_path.exists():
            _run_java_tool(assembler, asm_path)

    cases: list[TestCase] = []
    for subdir in ("demo", "01", "02", "03", "04", "05"):
        base = stage_root / subdir
        if not base.exists():
            continue
        for tst in sorted(base.rglob("*.tst")):
            rel = tst.relative_to(stage_root)
            rel_str = str(rel)
            dialect = _detect_dialect(tst)
            needs_kbd = rel_str in {"05/Memory.tst", "04/fill/Fill.tst"}
            workaround = None
            if rel_str == "05/Memory.tst":
                workaround = (
                    "Java HardwareSimulator has no headless keyboard input "
                    "(Keyboard[0] is read-only) and loops forever in "
                    "'while out <> 75 { eval, }' after writing 35 lines. "
                    "Both Java (timeout) and n2t (without --keys, limit) "
                    "stall at the same point; n2t --keys K,Y passes all 54 lines."
                )
            elif rel_str == "04/fill/Fill.tst":
                workaround = (
                    "Fill.tst is an infinite interactive loop ('repeat { ticktock; }') "
                    "with no output-file or compare-to; FillAutomatic.tst is the "
                    "automated headless test for Fill.asm."
                )
            cases.append(
                TestCase(
                    case_id=f"solved/{rel_str}",
                    scenario="solved",
                    project=_project_for_rel(rel),
                    dialect=dialect,
                    stage_dir=tst.parent,
                    rel_script=tst.name,
                    out_file_name=_parse_output_file(tst),
                    needs_keyboard=needs_kbd,
                    workaround=workaround,
                )
            )
    return cases


def build_untouched(course_dir: Path, private_dir: Path) -> list[TestCase]:
    """Stage exact course tree as shipped (unimplemented skeletons)."""
    stage_root = private_dir / "stage" / "untouched" / "projects"
    _copy_tree_clean(course_dir / "projects", stage_root)

    cases: list[TestCase] = []
    for subdir in ("demo", "01", "02", "03", "04", "05"):
        base = stage_root / subdir
        if not base.exists():
            continue
        for tst in sorted(base.rglob("*.tst")):
            rel = tst.relative_to(stage_root)
            rel_str = str(rel)
            dialect = _detect_dialect(tst)
            cases.append(
                TestCase(
                    case_id=f"untouched/{rel_str}",
                    scenario="untouched",
                    project=_project_for_rel(rel),
                    dialect=dialect,
                    stage_dir=tst.parent,
                    rel_script=tst.name,
                    out_file_name=_parse_output_file(tst),
                    needs_keyboard=False,
                )
            )
    return cases


def build_broken(
    course_dir: Path,
    private_dir: Path,
    *,
    seed: int = 20261123,
    mutation_count: int = 180,
) -> tuple[list[TestCase], list[Mutation]]:
    """Stage exp2 broken cases + deterministic runtime-generated mutation corpus."""
    cases: list[TestCase] = []

    # 1. evidence/exp2 cases
    exp2_src = private_dir / "evidence" / "exp2" / "cases"
    exp2_stage = private_dir / "stage" / "broken" / "exp2"
    if exp2_src.exists():
        _copy_tree_clean(exp2_src, exp2_stage)
        for case_dir in sorted(p for p in exp2_stage.iterdir() if p.is_dir()):
            for tst in sorted(case_dir.glob("*.tst")):
                cases.append(
                    TestCase(
                        case_id=f"broken/exp2/{case_dir.name}/{tst.name}",
                        scenario="broken",
                        project="exp2",
                        dialect="hardware",
                        stage_dir=case_dir,
                        rel_script=tst.name,
                        out_file_name=_parse_output_file(tst),
                    )
                )

    # 2. Deterministic mutation corpus generated from private solutions
    solutions_dir = private_dir / "solutions"
    mutations: list[Mutation] = []
    if solutions_dir.exists():
        mutations = generate_mutations(
            course_dir / "projects",
            solutions_dir,
            seed=seed,
            target_count=mutation_count,
        )
        mut_stage_root = private_dir / "stage" / "broken" / "mutations"
        if mut_stage_root.exists():
            shutil.rmtree(mut_stage_root)
        mut_stage_root.mkdir(parents=True, exist_ok=True)

        # Build a clean solved base per project dir to copy quickly
        solved_base = private_dir / "stage" / "broken" / "_solved_base"
        _copy_tree_clean(course_dir / "projects", solved_base)
        _overlay_tree(solutions_dir, solved_base)

        for mut in mutations:
            src_dir = (solved_base / mut.rel_hdl).parent
            dst_dir = mut_stage_root / mut.mut_id
            shutil.copytree(src_dir, dst_dir)
            # Overwrite only the mutated HDL file
            hdl_name = Path(mut.rel_hdl).name
            (dst_dir / hdl_name).write_text(mut.mutated_hdl, encoding="utf-8")
            tst_name = Path(mut.rel_tst).name
            tst_path = dst_dir / tst_name
            cases.append(
                TestCase(
                    case_id=f"broken/mutations/{mut.mut_id}",
                    scenario="broken",
                    project=f"mut/{mut.project}",
                    dialect="hardware",
                    stage_dir=dst_dir,
                    rel_script=tst_name,
                    out_file_name=_parse_output_file(tst_path),
                    notes=mut.description,
                    mutation_kind=mut.kind,
                    source_chip=mut.chip_name,
                )
            )

    return cases, mutations


def build_dialects(
    course_dir: Path,
    private_dir: Path,
) -> tuple[list[TestCase], list[SkippedCase]]:
    """Stage CPU and VM dialect scripts runnable with shipped course files."""
    stage_root = private_dir / "stage" / "dialects" / "projects"
    if stage_root.exists():
        shutil.rmtree(stage_root)
    stage_root.mkdir(parents=True, exist_ok=True)

    for proj in ("04", "07", "08", "12"):
        src = course_dir / "projects" / proj
        if src.exists():
            shutil.copytree(src, stage_root / proj)

    # For project 12 OS tests, copy shipped tools/OS/*.vm and compile shipped Main.jack
    os_vm_dir = course_dir / "tools" / "OS"
    jack_compiler = course_dir / "tools" / "JackCompiler.sh"
    for p12_test in ("ArrayTest", "MathTest", "MemoryTest"):
        tdir = stage_root / "12" / p12_test
        if tdir.exists() and os_vm_dir.exists():
            for vm_file in os_vm_dir.glob("*.vm"):
                shutil.copy2(vm_file, tdir / vm_file.name)
            main_jack = tdir / "Main.jack"
            if main_jack.exists():
                _run_java_tool(jack_compiler, main_jack)

    cases: list[TestCase] = []
    skipped: list[SkippedCase] = []

    for proj in ("04", "07", "08", "12"):
        pdir = stage_root / proj
        if not pdir.exists():
            continue
        for tst in sorted(pdir.rglob("*.tst")):
            rel = tst.relative_to(stage_root)
            rel_str = str(rel)
            dialect = _detect_dialect(tst)
            text = tst.read_text(encoding="utf-8", errors="replace")
            cleaned = re.sub(r"//[^\n]*", "", text)
            m = LOAD_CMD_RE.search(cleaned)
            loaded = m.group(1) if m else None

            # Check whether required artifact exists in course-shipped tree
            if loaded:
                target_path = tst.parent / loaded
                if not target_path.exists():
                    if proj == "04":
                        reason = (
                            f"needs student-assembled {loaded} "
                            "(covered by 'solved' scenario)"
                        )
                    else:
                        reason = (
                            f"needs student VM-translator output {loaded} "
                            "(not shipped in course zip)"
                        )
                    skipped.append(
                        SkippedCase(
                            case_id=f"dialects/{rel_str}",
                            scenario="dialects",
                            project=proj,
                            dialect=dialect,
                            rel_script=rel_str,
                            reason=reason,
                        )
                    )
                    continue

            cases.append(
                TestCase(
                    case_id=f"dialects/{rel_str}",
                    scenario="dialects",
                    project=proj,
                    dialect=dialect,
                    stage_dir=tst.parent,
                    rel_script=tst.name,
                    out_file_name=_parse_output_file(tst),
                    notes=(
                        "staged with shipped tools/OS/*.vm + JackCompiler(Main.jack)"
                        if proj == "12"
                        else None
                    ),
                )
            )

    return cases, skipped
