#!/usr/bin/env python3
"""Captures Java VM Emulator goldens for the original probe programs in
`crates/vm/tests/java_probes/`, so `cargo test` can check them without Java.

Each probe directory holds its own `.vm` files (original programs, no course
material) and a `probe.txt` header:

    steps 20000            # vmsteps the Java script runs
    keys 65 66 129 128     # optional: keys typed, one press+release each
    codes 65 66 129 128    # optional: actual Hack codes if Java's GUI remaps keys
    fast                   # optional: type keys quickly (long lines)
    ram 8000 8020          # RAM range [lo, hi) to record (repeatable)
    set RAM[0] 2046        # script variable setup (repeatable)
    single                 # load Main.vm directly rather than bootstrap a directory
    fault                  # run one failing step after recording RAM/screen
    screen                 # optional: record the whole screen

This script runs the Java emulator GUI (where the built-in OS exists) under
Xvfb, typing the keys through `KeyDrive.java` (public Swing APIs only: the
emulator is a black box), and writes `expected.txt` next to `probe.txt`:

    java <status line>
    ram <addr> <value>          # every recorded RAM cell
    screen <index> <value>      # every non-zero screen word (all others are 0)

Usage (needs the private tool copies under ~/n2t-private/tmp/vm):
    python3 conformance/vm/capture_java_probes.py [probe-name ...]
"""

from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
PROBES = REPO / "crates" / "vm" / "tests" / "java_probes"
PRIVATE = Path.home() / "n2t-private" / "tmp"
TOOLS = PRIVATE / "vm" / "tools"
JAVA = PRIVATE / "vm" / "jre" / "jdk-17.0.21.1+2-jre" / "bin" / "java"
WORK = PRIVATE / "vm-review" / "capture"
CLASSES = PRIVATE / "vm-review" / "gui"
LIBS = ["bin/classes", "bin/lib/Hack.jar", "bin/lib/HackGUI.jar", "bin/lib/Simulators.jar",
        "bin/lib/SimulatorsGUI.jar", "bin/lib/Compilers.jar"]


def compile_driver() -> None:
    CLASSES.mkdir(parents=True, exist_ok=True)
    cp = ":".join(str(TOOLS / lib) for lib in LIBS)
    subprocess.run(["javac", "--release", "17", "-nowarn", "-cp", cp, "-d", str(CLASSES),
                    str(Path(__file__).with_name("KeyDrive.java"))], check=True,
                   capture_output=True)


def read_header(path: Path) -> dict:
    h = {"steps": 500, "keys": [], "fast": False, "ram": [], "set": [],
         "screen": False, "single": False, "fault": False}
    for line in path.read_text().splitlines():
        parts = line.split("#", 1)[0].split()
        if not parts:
            continue
        if parts[0] == "steps":
            h["steps"] = int(parts[1])
        elif parts[0] == "keys":
            h["keys"] = [int(x) for x in parts[1:]]
        elif parts[0] == "codes":
            continue  # Rust replay only; send the original AWT events here.
        elif parts[0] in {"fast", "single", "screen", "fault"}:
            h[parts[0]] = True
        elif parts[0] == "ram":
            h["ram"].append((int(parts[1]), int(parts[2])))
        elif parts[0] == "set":
            h["set"].append(parts[1:3])
        else:
            raise ValueError(f"{path}: unknown probe directive {parts[0]}")
    return h


def script(h: dict) -> tuple[str, list[int]]:
    # The first output-list only opens the file; nothing is output with it.
    load = "load Main.vm" if h["single"] else "load"
    lines = [f"{load}, output-file T.out, output-list RAM[0]%D1.6.1;"]
    lines.extend(f"set {name} {value};" for name, value in h["set"])
    lines.append(f"repeat {h['steps']} {{ vmstep; }}")
    addrs = [addr for lo, hi in h["ram"] for addr in range(lo, hi)]
    if h["screen"]:
        addrs += list(range(16384, 24576))
    for i in range(0, len(addrs), 20):
        cols = " ".join(f"RAM[{a}]%D1.6.1" for a in addrs[i:i + 20])
        lines += [f"output-list {cols};", "output;"]
    if h["fault"]:
        lines.append("vmstep;")
    return "\n".join(lines) + "\n", addrs


def capture(probe: Path) -> None:
    h = read_header(probe / "probe.txt")
    work = WORK / probe.name
    shutil.rmtree(work, ignore_errors=True)
    work.mkdir(parents=True)
    for vm in probe.glob("*.vm"):
        shutil.copy(vm, work / vm.name)
    tst, addrs = script(h)
    (work / "T.tst").write_text(tst)
    kind = "f" if h["fast"] else "t"
    events = [f"{kind}{k}" for k in h["keys"]]
    per_key = 0.15 if h["fast"] else 0.55
    timeout = int(120 + per_key * len(events))
    cp = ":".join([str(CLASSES), "."] + LIBS)
    proc = subprocess.run(["xvfb-run", "-a", "-s", "-screen 0 1600x1200x24",
                           "timeout", str(timeout + 30), str(JAVA), "-cp", cp, "KeyDrive",
                           str(work / "T.tst"), str(timeout), *events],
                          cwd=TOOLS, capture_output=True, text=True)
    log = proc.stdout + proc.stderr
    statuses = [l[len("STATUS: "):] for l in log.splitlines() if l.startswith("STATUS: ")]
    dialogs = [l for l in log.splitlines() if l.startswith("CLICK ") and "No implementation" not in l]
    out = work / "T.out"
    values: list[int] = []
    if out.exists():
        for line in out.read_text().splitlines():
            cells = [c.strip() for c in line.strip().strip("|").split("|")]
            if cells and all(c.lstrip("-").isdigit() for c in cells):
                values += [int(c) for c in cells]
    if (proc.returncode or len(values) != len(addrs) or "TIMEOUT" in log or not statuses
            or (events and "KEYS DONE" not in log)):
        raise RuntimeError(f"{probe.name}: captured {len(values)}/{len(addrs)} values\n{log[-2000:]}")
    lines = [f"java {statuses[-1] if statuses else '?'}"]
    lines += [f"java-dialog {d[len('CLICK '):]}" for d in dialogs]
    ram_count = len(addrs) - (8192 if h["screen"] else 0)
    for i, (a, v) in enumerate(zip(addrs, values)):
        if i < ram_count:
            lines.append(f"ram {a} {v}")
        elif v != 0:
            lines.append(f"screen {a - 16384} {v}")
    (probe / "expected.txt").write_text("\n".join(lines) + "\n")
    print(f"{probe.name}: {lines[0]} ({len(lines) - 1} lines)", flush=True)


def main() -> None:
    compile_driver()
    names = sys.argv[1:]
    probes = [PROBES / name for name in names] if names else sorted(
        p for p in PROBES.iterdir() if p.is_dir()
    )
    ok = True
    for probe in probes:
        try:
            capture(probe)
        except RuntimeError as error:
            print(error, file=sys.stderr)
            ok = False
    if not ok:
        sys.exit(1)


if __name__ == "__main__":
    main()
