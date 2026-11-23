#!/usr/bin/env python3
"""Differential conformance harness for `n2t-vm` vs the official Java VM Emulator.

Compares:
  1. All 11 official VM-emulator tests under $N2T_COURSE_DIR/projects/07 and 08
     (reproducing each *VME.tst script's `set`s, `vmstep`s, and output cells).
  2. Project 12 OS test programs and Project 09 / 11 Jack programs compiled with
     the Java JackCompiler into a scratch directory (without OS .vm files, so
     both emulators use their built-in Jack OS), comparing RAM samples and all
     8192 screen words (with scripted RAM[24576] keyboard events for interactive
     programs).
  3. An original probe corpus covering every built-in OS class/function and
     runtime error condition (VM core + OS ERR1..ERR20), comparing RAM, screen,
     and error/halt status.

Exits 0 with a skip message if N2T_COURSE_DIR is unset or missing (CI-safe).
Exits non-zero on any mismatch.
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence

REPO_ROOT = Path(__file__).resolve().parents[2]
VMRUN_BIN = REPO_ROOT / "target" / "release" / "examples" / "vmrun"
DEFAULT_SCRATCH = Path.home() / "n2t-private" / "tmp" / "vm"
GUI_BIN = DEFAULT_SCRATCH / "bin" / "g"
HEADLESS_BIN = DEFAULT_SCRATCH / "bin" / "j"
TOOLS_COPY = DEFAULT_SCRATCH / "tools"


@dataclass
class CaseResult:
    group: str
    name: str
    steps: int
    ram_cells: int
    screen_words: int
    ok: bool
    detail: str = ""


def ensure_vmrun() -> Path:
    if not VMRUN_BIN.exists():
        print("Building n2t-vm vmrun (release)...", flush=True)
        subprocess.run(
            ["cargo", "build", "--release", "-p", "n2t-vm", "--example", "vmrun", "-j", "3"],
            cwd=REPO_ROOT,
            check=True,
        )
    return VMRUN_BIN


def run_vmrun(
    target: Path,
    sets: Sequence[tuple[str, int]] = (),
    steps: int = 1_000_000,
    key_events: Sequence[tuple[int, int]] = (),
    dumps: Sequence[str] = (),
    dump_screen: bool = False,
    show_status: bool = False,
) -> tuple[dict[str, str], list[int], str]:
    cmd = [str(VMRUN_BIN), str(target), "--steps", str(steps)]
    for k, v in sets:
        cmd += ["--set", f"{k}={v}"]
    for s, c in key_events:
        cmd += ["--key-at", f"{s}:{c}"]
    for d in dumps:
        cmd += ["--dump", d]
    if dump_screen:
        cmd.append("--dump-screen")
    if show_status:
        cmd.append("--status")
    proc = subprocess.run(cmd, text=True, capture_output=True)
    if proc.returncode != 0:
        raise RuntimeError(f"vmrun failed (rc={proc.returncode}): {proc.stderr.strip()}")
    values: dict[str, str] = {}
    screen: list[int] = []
    status = ""
    for line in proc.stdout.splitlines():
        if line.startswith("STATUS: "):
            status = line[len("STATUS: ") :].strip()
        elif line.startswith("SCREEN["):
            _, rest = line.split(":", 1)
            screen.extend(int(x) for x in rest.split())
        elif "=" in line:
            k, v = line.split("=", 1)
            values[k.strip()] = v.strip()
    return values, screen, status


def run_java_headless(work_dir: Path, tst_body: str, timeout: int = 60) -> tuple[str, Path]:
    tst_path = work_dir / "T.tst"
    out_path = work_dir / "T.out"
    tst_path.write_text(tst_body)
    if out_path.exists():
        out_path.unlink()
    vme = TOOLS_COPY / "VMEmulator.sh"
    if not vme.exists():
        vme = Path(os.environ["N2T_COURSE_DIR"]) / "tools" / "VMEmulator.sh"
    proc = subprocess.run(
        ["timeout", str(timeout), "sh", str(vme), str(tst_path)],
        text=True,
        capture_output=True,
    )
    return (proc.stdout + proc.stderr).strip(), out_path


def run_java_gui(work_dir: Path, tst_body: str, timeout: int = 90) -> tuple[str, Path]:
    if not GUI_BIN.exists():
        raise RuntimeError(f"GUI driver not found at {GUI_BIN}")
    proc = subprocess.run(
        [str(GUI_BIN), str(work_dir), str(timeout)],
        input=tst_body,
        text=True,
        capture_output=True,
    )
    return (proc.stdout + proc.stderr).strip(), work_dir / "T.out"


def dump_range_tst(addrs: Sequence[int], chunk: int = 16) -> str:
    """Generates output-list / output blocks with <= 16 columns (Java GUI crashes > 20)."""
    lines = []
    for i in range(0, len(addrs), chunk):
        part = addrs[i : i + chunk]
        cols = " ".join(f"RAM[{a}]%D1.6.1" for a in part)
        lines.append(f"output-list {cols};")
        lines.append("output;")
    return "\n".join(lines)


def parse_table_rows(out_path: Path) -> list[list[str]]:
    if not out_path.exists():
        return []
    rows = []
    for line in out_path.read_text().splitlines():
        if not line.startswith("|"):
            continue
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if not cells:
            continue
        # Skip header rows (contain letters like RAM, sp, local, etc.)
        if any(re.search(r"[A-Za-z]", c) for c in cells):
            continue
        rows.append(cells)
    return rows


# ---------------------------------------------------------------------------
# 1. Projects 07 & 08: Official *VME.tst tests
# ---------------------------------------------------------------------------


def parse_vme_tst(tst_text: str) -> dict:
    # Strip // comments
    cleaned = re.sub(r"//.*", "", tst_text)
    load_m = re.search(r"\bload\b(?:\s+([^,;]+))?", cleaned)
    load_target = load_m.group(1).strip() if (load_m and load_m.group(1)) else None

    sets: list[tuple[str, int]] = []
    for m in re.finditer(r"\bset\s+([A-Za-z0-9_\[\]]+)\s+(-?\d+)", cleaned):
        sets.append((m.group(1), int(m.group(2))))

    repeat_m = re.search(r"\brepeat\s+(\d+)\s*\{\s*vmstep;\s*\}", cleaned)
    steps = int(repeat_m.group(1)) if repeat_m else 0

    output_vars: list[str] = []
    for m in re.finditer(r"\boutput-list\s+([^;]+);", cleaned):
        for tok in m.group(1).split():
            var = tok.split("%", 1)[0].strip()
            if var:
                output_vars.append(var)

    return {
        "load": load_target,
        "sets": sets,
        "steps": steps,
        "output_vars": output_vars,
    }


def run_project_07_08(course_dir: Path, scratch: Path) -> list[CaseResult]:
    results: list[CaseResult] = []
    tst_files = sorted(
        list((course_dir / "projects" / "07").rglob("*VME.tst"))
        + list((course_dir / "projects" / "08").rglob("*VME.tst"))
    )
    for tst_src in tst_files:
        rel = tst_src.relative_to(course_dir / "projects")
        name = str(rel.parent)
        spec = parse_vme_tst(tst_src.read_text(errors="replace"))

        # Copy test directory to scratch so Java never writes inside course_dir
        work = scratch / "p0708" / rel.parent
        if work.exists():
            shutil.rmtree(work)
        shutil.copytree(tst_src.parent, work)

        # Run official Java VMEmulator.sh on the copied *VME.tst
        vme = TOOLS_COPY / "VMEmulator.sh"
        if not vme.exists():
            vme = course_dir / "tools" / "VMEmulator.sh"
        java_tst = work / tst_src.name
        proc = subprocess.run(
            ["timeout", "60", "sh", str(vme), str(java_tst)],
            text=True,
            capture_output=True,
        )
        out_m = re.search(r"\boutput-file\s+([^\s,;]+)", tst_src.read_text(errors="replace"))
        java_out = work / (out_m.group(1) if out_m else "out")
        java_rows = parse_table_rows(java_out)
        java_vals = [int(x) for row in java_rows for x in row]

        # Run n2t-vm vmrun on the exact same input, sets, steps, and output vars
        target = work / spec["load"] if spec["load"] else work
        vm_map, _, _ = run_vmrun(
            target,
            sets=spec["sets"],
            steps=spec["steps"],
            dumps=spec["output_vars"],
        )
        rust_vals = [int(vm_map[v]) for v in spec["output_vars"]]

        ok = proc.returncode == 0 and len(java_vals) == len(rust_vals) and java_vals == rust_vals
        detail = ""
        if not ok:
            detail = f"java={java_vals} rust={rust_vals} java_msg={(proc.stdout + proc.stderr).strip()}"
        r = CaseResult(
            group="p07-08",
            name=name,
            steps=spec["steps"],
            ram_cells=len(spec["output_vars"]),
            screen_words=0,
            ok=ok,
            detail=detail,
        )
        log_result(r)
        results.append(r)
    return results


# ---------------------------------------------------------------------------
# 2. Projects 09, 11, 12: Compiled Jack programs (built-in OS + full screen)
# ---------------------------------------------------------------------------


def compile_jack_dir(course_dir: Path, src_dir: Path, dst_dir: Path) -> None:
    if dst_dir.exists():
        shutil.rmtree(dst_dir)
    dst_dir.mkdir(parents=True)
    for jf in src_dir.glob("*.jack"):
        shutil.copy2(jf, dst_dir / jf.name)
    compiler = TOOLS_COPY / "JackCompiler.sh"
    if not compiler.exists():
        compiler = course_dir / "tools" / "JackCompiler.sh"
    proc = subprocess.run(
        ["timeout", "60", "sh", str(compiler), str(dst_dir)],
        text=True,
        capture_output=True,
    )
    if proc.returncode != 0 or not list(dst_dir.glob("*.vm")):
        raise RuntimeError(f"JackCompiler failed on {src_dir}: {proc.stdout} {proc.stderr}")
    # Remove .jack files so only .vm files remain
    for jf in dst_dir.glob("*.jack"):
        jf.unlink()


def build_step_key_tst(
    sets: Sequence[tuple[str, int]],
    total_steps: int,
    key_events: Sequence[tuple[int, int]],
    ram_addrs: Sequence[int],
    dump_screen: bool,
) -> str:
    lines = ["load, output-file T.out, output-list RAM[0]%D1.6.1;"]
    for k, v in sets:
        lines.append(f"set {k} {v};")

    events = sorted(key_events, key=lambda x: x[0])
    cur = 0
    for at, code in events:
        if at > total_steps:
            break
        delta = at - cur
        if delta > 0:
            lines.append(f"repeat {delta} {{ vmstep; }}")
            cur = at
        lines.append(f"set RAM[24576] {code};")
    if total_steps > cur:
        lines.append(f"repeat {total_steps - cur} {{ vmstep; }}")

    if ram_addrs:
        lines.append(dump_range_tst(ram_addrs, chunk=20))
    if dump_screen:
        lines.append(dump_range_tst(range(16384, 24576), chunk=20))
    return "\n".join(lines) + "\n"


def log_result(r: CaseResult) -> None:
    status = "PASS" if r.ok else f"FAIL: {r.detail}"
    print(f"  [{r.group:<7}] {r.name:<36} steps={r.steps:<6} ram={r.ram_cells:<4} scr={r.screen_words:<4} -> {status}", flush=True)


def run_jack_programs(course_dir: Path, scratch: Path) -> list[CaseResult]:
    results: list[CaseResult] = []
    # (label, rel_path, steps, sets, key_events, ram_addrs, dump_screen)
    programs: list[tuple[str, str, int, list[tuple[str, int]], list[tuple[int, int]], list[int], bool]] = [
        # Project 12 OS tests
        ("12/ArrayTest", "12/ArrayTest", 5000, [], [], list(range(8000, 8004)), False),
        ("12/MathTest", "12/MathTest", 5000, [], [], list(range(8000, 8014)), False),
        ("12/MemoryTest", "12/MemoryTest", 5000, [], [], list(range(8000, 8006)), False),
        ("12/OutputTest", "12/OutputTest", 5000, [], [], [0], True),
        ("12/ScreenTest", "12/ScreenTest", 5000, [], [], [0], True),
        ("12/StringTest", "12/StringTest", 5000, [], [], [0], True),
        # Project 12 SysTest (waits for key press -> calls Sys.wait -> prints)
        ("12/SysTest", "12/SysTest", 1500, [], [(200, 65), (250, 0)], [0], True),
        # Project 09 programs
        ("09/HelloWorld", "09/HelloWorld", 2000, [], [], [0], True),
        ("09/Fraction", "09/Fraction", 5000, [], [], [0], True),
        ("09/List", "09/List", 5000, [], [], [0], True),
        # Interactive Square game: move right (132), down (133), grow (88='X'), shrink (90='Z'), quit (81='Q')
        (
            "09/Square",
            "09/Square",
            6000,
            [],
            [(500, 132), (1500, 133), (2500, 88), (3500, 90), (4500, 81), (5000, 0)],
            [0],
            True,
        ),
        # Project 11 programs
        ("11/Seven", "11/Seven", 2000, [], [], [0], True),
        ("11/ConvertToBin", "11/ConvertToBin", 10000, [("RAM[8000]", 23451)], [], list(range(8000, 8017)), False),
        ("11/ComplexArrays", "11/ComplexArrays", 10000, [], [], [0], True),
        (
            "11/Square",
            "11/Square",
            6000,
            [],
            [(500, 132), (1500, 133), (2500, 88), (3500, 90), (4500, 81), (5000, 0)],
            [0],
            True,
        ),
        # Pong: runs bat + ball physics driven by arrow keys 130 (left), 132 (right), 140 (ESC quit)
        (
            "11/Pong",
            "11/Pong",
            4000,
            [],
            [(600, 132), (1800, 130), (3000, 0), (3600, 140)],
            [0],
            True,
        ),
    ]

    for label, rel, steps, sets, keys, ram_addrs, dump_scr in programs:
        src = course_dir / "projects" / rel
        if not src.exists():
            continue
        work = scratch / "jack" / label.replace("/", "_")
        compile_jack_dir(course_dir, src, work)

        tst = build_step_key_tst(sets, steps, keys, ram_addrs, dump_scr)
        java_log, out_path = run_java_gui(work, tst, timeout=90)
        rows = parse_table_rows(out_path)
        flat = [int(x) for row in rows for x in row]

        expected_total = len(ram_addrs) + (8192 if dump_scr else 0)
        java_ram = flat[: len(ram_addrs)]
        java_scr = flat[len(ram_addrs) :]

        dumps = [f"RAM[{a}]" for a in ram_addrs]
        vm_map, rust_scr, _ = run_vmrun(
            work,
            sets=sets,
            steps=steps,
            key_events=keys,
            dumps=dumps,
            dump_screen=dump_scr,
        )
        rust_ram = [int(vm_map[f"RAM[{a}]"]) for a in ram_addrs]

        ram_ok = len(flat) == expected_total and java_ram == rust_ram
        scr_ok = (not dump_scr) or (len(java_scr) == 8192 and java_scr == rust_scr)
        ok = ram_ok and scr_ok
        detail = ""
        if not ok:
            if len(flat) != expected_total:
                detail = f"java captured {len(flat)}/{expected_total} values; log={java_log[:200]}"
            elif not ram_ok:
                detail = f"RAM mismatch: java={java_ram} rust={rust_ram}"
            else:
                diffs = [
                    (16384 + i, java_scr[i], rust_scr[i])
                    for i in range(8192)
                    if java_scr[i] != rust_scr[i]
                ]
                detail = f"{len(diffs)} screen words differ; first 5: {diffs[:5]}"

        r = CaseResult(
            group="jack-os",
            name=label,
            steps=steps,
            ram_cells=len(ram_addrs),
            screen_words=8192 if dump_scr else 0,
            ok=ok,
            detail=detail,
        )
        log_result(r)
        results.append(r)
    return results


# ---------------------------------------------------------------------------
# 3. Original probe corpus: every built-in OS function & error condition
# ---------------------------------------------------------------------------


def build_os_probes() -> list[dict]:
    """Returns our own original VM probe definitions (no course material)."""
    probes = []

    # Probe 1: Math edge cases (negatives, overflow, -32768, sqrt boundaries)
    probes.append(
        {
            "name": "probe_math_edges",
            "vm": """function Main.main 0
push constant 32767
push constant 2
call Math.multiply 2
pop static 0
push constant 32767
push constant 1
add
push constant 1
neg
call Math.multiply 2
pop static 1
push constant 7
neg
push constant 2
call Math.divide 2
pop static 2
push constant 7
push constant 2
neg
call Math.divide 2
pop static 3
push constant 7
neg
push constant 2
neg
call Math.divide 2
pop static 4
push constant 32767
push constant 1
add
push constant 1
neg
call Math.divide 2
pop static 5
push constant 32767
push constant 1
add
call Math.abs 1
pop static 6
push constant 0
call Math.sqrt 1
pop static 7
push constant 32767
call Math.sqrt 1
pop static 8
push constant 32767
push constant 1
add
push constant 32767
call Math.min 2
pop static 9
push constant 32767
push constant 1
add
push constant 32767
call Math.max 2
pop static 10
push constant 0
return
""",
            "steps": 500,
            "ram_addrs": list(range(16, 27)),
            "dump_screen": False,
            "expect_error": False,
        }
    )

    # Probe 2: Memory alloc/deAlloc/split/coalesce + peek/poke + Array
    probes.append(
        {
            "name": "probe_memory_array",
            "vm": """function Main.main 0
push constant 5
call Memory.alloc 1
pop static 0
push constant 1
call Array.new 1
pop static 1
push static 0
call Memory.deAlloc 1
pop temp 0
push constant 3
call Memory.alloc 1
pop static 2
push constant 6
call Memory.alloc 1
pop static 3
push constant 1
call Memory.alloc 1
pop static 4
push static 3
call Memory.deAlloc 1
pop temp 0
push constant 3
call Memory.alloc 1
pop static 5
push constant 1
call Memory.alloc 1
pop static 6
push static 1
call Array.dispose 1
pop temp 0
push constant 8000
push constant 12345
call Memory.poke 2
pop temp 0
push constant 8000
call Memory.peek 1
pop static 7
push constant 0
call Memory.peek 1
pop static 8
push constant 0
return
""",
            "steps": 500,
            "ram_addrs": list(range(16, 25)) + list(range(2048, 2076)),
            "dump_screen": False,
            "expect_error": False,
        }
    )

    # Probe 3: String operations + heap layout + setInt/intValue edge cases
    probes.append(
        {
            "name": "probe_string_edges",
            "vm": """function Main.main 1
push constant 0
call String.new 1
pop static 0
push constant 8
call String.new 1
pop static 1
push static 1
push constant 32767
push constant 1
add
call String.setInt 2
pop temp 0
push static 1
call String.intValue 1
pop static 2
push static 1
call String.length 1
pop static 3
push static 1
push constant 0
call String.charAt 2
pop static 4
push static 1
push constant 5
call String.charAt 2
pop static 5
push static 1
call String.eraseLastChar 1
pop temp 0
push static 1
push constant 1
push constant 57
call String.setCharAt 3
pop temp 0
push static 1
call String.intValue 1
pop static 6
call String.backSpace 0
pop static 7
call String.doubleQuote 0
pop static 8
call String.newLine 0
pop static 9
push constant 6
call String.new 1
pop local 0
push local 0
push constant 45
call String.appendChar 2
push constant 52
call String.appendChar 2
push constant 50
call String.appendChar 2
push constant 65
call String.appendChar 2
pop temp 0
push local 0
call String.intValue 1
pop static 10
push local 0
call String.dispose 1
pop temp 0
push constant 0
return
""",
            "steps": 500,
            "ram_addrs": list(range(16, 27)) + list(range(2048, 2072)),
            "dump_screen": False,
            "expect_error": False,
        }
    )

    # Probe 4: Screen drawPixel, drawLine (all 8 octants + degenerate), drawRectangle, drawCircle (r=0,1,5,15,40, neg)
    probes.append(
        {
            "name": "probe_screen_all",
            "vm": """function Main.main 0
// Clear, set black, draw lines in all directions + ties
call Screen.clearScreen 0
pop temp 0
push constant 1
call Screen.setColor 1
pop temp 0
push constant 5
push constant 5
call Screen.drawPixel 2
pop temp 0
push constant 10
push constant 10
push constant 45
push constant 23
call Screen.drawLine 4
pop temp 0
push constant 95
push constant 23
push constant 60
push constant 10
call Screen.drawLine 4
pop temp 0
push constant 10
push constant 50
push constant 45
push constant 37
call Screen.drawLine 4
pop temp 0
push constant 95
push constant 37
push constant 60
push constant 50
call Screen.drawLine 4
pop temp 0
push constant 120
push constant 10
push constant 133
push constant 55
call Screen.drawLine 4
pop temp 0
push constant 150
push constant 55
push constant 163
push constant 10
call Screen.drawLine 4
pop temp 0
push constant 180
push constant 10
push constant 210
push constant 40
call Screen.drawLine 4
pop temp 0
push constant 220
push constant 40
push constant 250
push constant 10
call Screen.drawLine 4
pop temp 0
push constant 10
push constant 70
push constant 10
push constant 70
call Screen.drawLine 4
pop temp 0
// Rectangle + erase inner rectangle with white
push constant 20
push constant 80
push constant 120
push constant 140
call Screen.drawRectangle 4
pop temp 0
push constant 0
call Screen.setColor 1
pop temp 0
push constant 35
push constant 95
push constant 105
push constant 125
call Screen.drawRectangle 4
pop temp 0
// Circles: r=0, r=1, r=5, r=15, r=40, and r=-2 (silent no-op)
push constant 1
call Screen.setColor 1
pop temp 0
push constant 200
push constant 100
push constant 0
call Screen.drawCircle 3
pop temp 0
push constant 220
push constant 100
push constant 1
call Screen.drawCircle 3
pop temp 0
push constant 250
push constant 100
push constant 5
call Screen.drawCircle 3
pop temp 0
push constant 300
push constant 100
push constant 15
call Screen.drawCircle 3
pop temp 0
push constant 400
push constant 120
push constant 40
call Screen.drawCircle 3
pop temp 0
push constant 200
push constant 200
push constant 2
neg
call Screen.drawCircle 3
pop temp 0
push constant 0
return
""",
            "steps": 500,
            "ram_addrs": [0],
            "dump_screen": True,
            "expect_error": False,
        }
    )

    # Probe 5: Output all 95 printable glyphs, fallback box, printInt(-32768), cursor wrap, backSpace
    out_vm = ["function Main.main 0"]
    for c in range(32, 127):
        out_vm += [f"push constant {c}", "call Output.printChar 1", "pop temp 0"]
    out_vm += [
        "call Output.println 0",
        "pop temp 0",
        "push constant 32767",
        "push constant 1",
        "add",
        "call Output.printInt 1",
        "pop temp 0",
        "push constant 0",
        "call Output.printChar 1",
        "pop temp 0",
        "push constant 129",
        "call Output.printChar 1",
        "pop temp 0",
        "push constant 33",
        "call Output.printChar 1",
        "pop temp 0",
        "push constant 22",
        "push constant 63",
        "call Output.moveCursor 2",
        "pop temp 0",
        "push constant 90",
        "call Output.printChar 1",
        "pop temp 0",
        "push constant 89",
        "call Output.printChar 1",
        "pop temp 0",
        "push constant 1",
        "push constant 0",
        "call Output.moveCursor 2",
        "pop temp 0",
        "call Output.backSpace 0",
        "pop temp 0",
        "push constant 88",
        "call Output.printChar 1",
        "pop temp 0",
        "push constant 0",
        "return",
    ]
    probes.append(
        {
            "name": "probe_output_all",
            "vm": "\n".join(out_vm) + "\n",
            "steps": 1000,
            "ram_addrs": [0],
            "dump_screen": True,
            "expect_error": False,
        }
    )

    # Probe 6: Keyboard.keyPressed + Sys.wait(0)
    probes.append(
        {
            "name": "probe_keyboard_syswait",
            "vm": """function Main.main 0
call Keyboard.keyPressed 0
pop static 0
push constant 0
call Sys.wait 1
pop temp 0
push constant 0
return
""",
            "steps": 50,
            "sets": [("RAM[24576]", 131)],
            "ram_addrs": [16],
            "dump_screen": False,
            "expect_error": False,
        }
    )

    # Probes 7..25: Every OS error condition (compare ERR<code> on screen + halted/error status!)
    err_defs = [
        ("err_sys_wait_neg", "push constant 1\nneg\ncall Sys.wait 1\npop temp 0\n"),
        ("err_array_0", "push constant 0\ncall Array.new 1\npop temp 0\n"),
        ("err_math_div0", "push constant 5\npush constant 0\ncall Math.divide 2\npop temp 0\n"),
        ("err_math_sqrt_neg", "push constant 1\nneg\ncall Math.sqrt 1\npop temp 0\n"),
        ("err_mem_alloc0", "push constant 0\ncall Memory.alloc 1\npop temp 0\n"),
        ("err_mem_oom", "push constant 20000\ncall Memory.alloc 1\npop temp 0\n"),
        ("err_scr_px", "push constant 512\npush constant 0\ncall Screen.drawPixel 2\npop temp 0\n"),
        ("err_scr_line", "push constant 0\npush constant 0\npush constant 512\npush constant 0\ncall Screen.drawLine 4\npop temp 0\n"),
        ("err_scr_rect", "push constant 10\npush constant 10\npush constant 5\npush constant 20\ncall Screen.drawRectangle 4\npop temp 0\n"),
        ("err_scr_circ_c", "push constant 512\npush constant 10\npush constant 5\ncall Screen.drawCircle 3\npop temp 0\n"),
        ("err_scr_circ_r", "push constant 10\npush constant 10\npush constant 200\ncall Screen.drawCircle 3\npop temp 0\n"),
        ("err_str_neg", "push constant 1\nneg\ncall String.new 1\npop temp 0\n"),
        ("err_str_charat", "push constant 2\ncall String.new 1\npush constant 0\ncall String.charAt 2\npop temp 0\n"),
        ("err_str_setchar", "push constant 2\ncall String.new 1\npush constant 0\npush constant 65\ncall String.setCharAt 3\npop temp 0\n"),
        ("err_str_append", "push constant 0\ncall String.new 1\npush constant 65\ncall String.appendChar 2\npop temp 0\n"),
        ("err_str_erase", "push constant 1\ncall String.new 1\ncall String.eraseLastChar 1\npop temp 0\n"),
        ("err_str_setint", "push constant 1\ncall String.new 1\npush constant 10\ncall String.setInt 2\npop temp 0\n"),
        ("err_out_cursor", "push constant 23\npush constant 0\ncall Output.moveCursor 2\npop temp 0\n"),
        ("err_sys_error", "push constant 99\ncall Sys.error 1\npop temp 0\n"),
    ]
    # Check first 4 words of scanlines 0..10 (44 screen words covering ERR<code> at row 0 cols 0..7)
    err_screen_addrs = [16384 + dy * 32 + wc for dy in range(11) for wc in range(4)]
    for ename, body in err_defs:
        probes.append(
            {
                "name": ename,
                "vm": f"function Main.main 0\n{body}push constant 0\nreturn\n",
                "steps": 50,
                "ram_addrs": err_screen_addrs,
                "dump_screen": False,
                "expect_error": True,
            }
        )

    return probes


def run_probes(scratch: Path) -> list[CaseResult]:
    results: list[CaseResult] = []
    for p in build_os_probes():
        work = scratch / "probes" / p["name"]
        if work.exists():
            shutil.rmtree(work)
        work.mkdir(parents=True)
        (work / "Main.vm").write_text(p["vm"])

        sets = p.get("sets", [])
        ram_addrs = p["ram_addrs"]
        dump_scr = p["dump_screen"]
        steps = p["steps"]

        tst = build_step_key_tst(sets, steps, [], ram_addrs, dump_scr)
        java_log, out_path = run_java_gui(work, tst, timeout=45)
        rows = parse_table_rows(out_path)
        flat = [int(x) for row in rows for x in row]

        expected_total = len(ram_addrs) + (8192 if dump_scr else 0)
        java_ram = flat[: len(ram_addrs)]
        java_scr = flat[len(ram_addrs) :]

        dumps = [f"RAM[{a}]" for a in ram_addrs]
        vm_map, rust_scr, rust_status = run_vmrun(
            work,
            sets=sets,
            steps=steps,
            dumps=dumps,
            dump_screen=dump_scr,
            show_status=True,
        )
        rust_ram = [int(vm_map[f"RAM[{a}]"]) for a in ram_addrs]

        ram_ok = len(flat) == expected_total and java_ram == rust_ram
        scr_ok = (not dump_scr) or (len(java_scr) == 8192 and java_scr == rust_scr)
        err_ok = (not p["expect_error"]) or ("Error(" in rust_status)
        java_halted_ok = (not p["expect_error"]) or ("Program Halted:" in java_log)
        ok = ram_ok and scr_ok and err_ok and java_halted_ok
        detail = ""
        if not ok:
            if not ram_ok:
                detail = f"RAM mismatch (got {len(flat)}/{expected_total}): java={java_ram[:10]} rust={rust_ram[:10]}"
            elif not scr_ok:
                diffs = [
                    (16384 + i, java_scr[i], rust_scr[i])
                    for i in range(8192)
                    if java_scr[i] != rust_scr[i]
                ]
                detail = f"{len(diffs)} screen words differ; first 5: {diffs[:5]}"
            else:
                detail = f"status mismatch: rust={rust_status} java={java_log[:120]}"

        r = CaseResult(
            group="probes",
            name=p["name"],
            steps=steps,
            ram_cells=len(ram_addrs),
            screen_words=8192 if dump_scr else 0,
            ok=ok,
            detail=detail,
        )
        log_result(r)
        results.append(r)
    return results


def main() -> int:
    parser = argparse.ArgumentParser(description="Differential conformance test: n2t-vm vs Java VM Emulator")
    parser.add_argument("--only", choices=["all", "p0708", "jack", "probes"], default="all")
    args = parser.parse_args()

    course_env = os.environ.get("N2T_COURSE_DIR")
    if not course_env or not Path(course_env).is_dir():
        print("Skipping VM differential conformance: N2T_COURSE_DIR is not set or does not exist.")
        return 0
    course_dir = Path(course_env)

    ensure_vmrun()
    scratch = Path(tempfile.mkdtemp(prefix="n2t_vm_diff_"))
    t0 = time.monotonic()
    results: list[CaseResult] = []
    try:
        if args.only in ("all", "p0708"):
            results.extend(run_project_07_08(course_dir, scratch))
        if args.only in ("all", "jack"):
            results.extend(run_jack_programs(course_dir, scratch))
        if args.only in ("all", "probes"):
            results.extend(run_probes(scratch))
    finally:
        shutil.rmtree(scratch, ignore_errors=True)

    elapsed = time.monotonic() - t0
    total_steps = sum(r.steps for r in results)
    total_ram = sum(r.ram_cells for r in results)
    total_scr = sum(r.screen_words for r in results)
    passed = sum(1 for r in results if r.ok)
    failed = len(results) - passed

    print(f"\n{'GROUP':<8} {'CASE':<36} {'STEPS':>8} {'RAM':>6} {'SCREEN':>7}  RESULT")
    print("-" * 80)
    for r in results:
        status = "PASS" if r.ok else f"FAIL: {r.detail}"
        print(f"{r.group:<8} {r.name:<36} {r.steps:>8} {r.ram_cells:>6} {r.screen_words:>7}  {status}")
    print("-" * 80)
    print(
        f"TOTAL: {passed}/{len(results)} passed, {failed} failed | "
        f"{total_steps:,} steps, {total_ram:,} RAM cells, {total_scr:,} screen words compared in {elapsed:.1f}s"
    )
    return 0 if failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
