#!/usr/bin/env python3
"""Differential test of n2t-hack against the official Java assembler and CPU emulator.

Needs the course distribution (with tools/Assembler.sh and tools/CPUEmulator.sh)
in $N2T_COURSE_DIR (default ~/n2t-private/course) and Java on the PATH. Skips
with a message, exit status 0, when the course directory is missing.

Scratch files go to $N2T_SCRATCH_DIR (default ~/n2t-private/tmp/hack/conformance).
That path must not contain a '.': the official assembler silently writes
nothing when a directory name in the path contains one.

    python3 conformance/hack/run.py [--seed N] [--quick] [--jobs N]

Phases:
  1. assembler: every .asm under $N2T_COURSE_DIR/projects and
     crates/hack/tests/fixtures, plus whole-file edge cases and generated valid
     programs, assembled by both; the .hack files must be identical.
  2. cpu: course programs, fixtures, an exhaustive comp/dest/jump sweep and
     random instruction streams, single-stepped by both with A, D, PC and RAM
     cells output after every ticktock; the traces must be identical, including
     where an emulator error stops the script.
  3. invalid: hand-written invalid lines and fuzzed lines injected into valid
     programs; accept/reject must agree, accepted output must be identical and
     the first error line must be the same (Java's line is found by bisection,
     because its messages do not always carry a reliable line number).

Exits 1 if anything differs; details go to $N2T_SCRATCH_DIR/report.txt.
"""

import argparse
import concurrent.futures as cf
import os
import random
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
FIXTURES = REPO / "crates" / "hack" / "tests" / "fixtures"
COURSE = Path(os.environ.get("N2T_COURSE_DIR", "~/n2t-private/course")).expanduser()
SCRATCH = Path(os.environ.get("N2T_SCRATCH_DIR", "~/n2t-private/tmp/hack/conformance")).expanduser()
TOOLS = COURSE / "tools"

report_lines = []


def log(msg):
    print(msg, flush=True)


def note(msg):
    report_lines.append(msg)


# ---------------------------------------------------------------- tools ----


def workdir(prefix):
    return Path(tempfile.mkdtemp(dir=SCRATCH, prefix=prefix))


def java_assemble(src: bytes):
    """Returns (hack text or None, console text)."""
    d = workdir("ja")
    try:
        p = d / "P.asm"
        p.write_bytes(src)
        r = subprocess.run(["timeout", "60", "sh", str(TOOLS / "Assembler.sh"), str(p)],
                           capture_output=True)
        console = (r.stdout + r.stderr).decode("utf-8", "replace")
        h = d / "P.hack"
        # With exactly 32768 instructions Java throws while writing and leaves
        # a truncated file: that is a rejection.
        if h.exists() and "Exception" not in console:
            return h.read_bytes().decode().replace("\r\n", "\n"), console
        return None, console
    finally:
        shutil.rmtree(d, ignore_errors=True)


def our_assemble(src: bytes):
    """Returns (hack text or None, first error line or None)."""
    d = workdir("ra")
    try:
        p = d / "P.asm"
        p.write_bytes(src)
        r = subprocess.run([str(OUR_ASM), str(p)], capture_output=True, timeout=120)
        if r.returncode == 0:
            return (d / "P.hack").read_text(), None
        err = r.stderr.decode("utf-8", "replace")
        for line in err.splitlines():
            if line.startswith("first-error-line: "):
                return None, int(line.split(": ")[1])
        raise RuntimeError("our assembler failed: " + err[-2000:])
    finally:
        shutil.rmtree(d, ignore_errors=True)


def java_lines(src: bytes):
    """Splits into lines with terminators, like Java's readLine (\\n, \\r\\n, \\r)."""
    out, start, i = [], 0, 0
    while i < len(src):
        if src[i:i + 1] == b"\n":
            out.append(src[start:i + 1]); i += 1; start = i
        elif src[i:i + 1] == b"\r":
            j = i + 2 if src[i + 1:i + 2] == b"\n" else i + 1
            out.append(src[start:j]); i = j; start = j
        else:
            i += 1
    if start < len(src):
        out.append(src[start:])
    return out


def java_first_error_line(src: bytes):
    """Smallest k such that Java rejects lines[0..=k]. Assumes Java rejects src."""
    lines = java_lines(src)
    lo, hi = 0, len(lines) - 1
    while lo < hi:
        mid = (lo + hi) // 2
        ok, _ = java_assemble(b"".join(lines[:mid + 1]))
        if ok is None:
            hi = mid
        else:
            lo = mid + 1
    return lo


def hack_text(words):
    return "".join(format(w & 0xFFFF, "016b") + "\n" for w in words)


def parse_java_out(text):
    rows = []
    for line in text.splitlines()[1:]:
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if cells and cells != [""]:
            rows.append([int(c) for c in cells])
    return rows


def run_trace(hack: str, script_body: str, columns):
    """Runs the same script through Java and through our trace example.

    Returns (java_rows, java_error, our_rows, our_error)."""
    header = (f"load P.hack,\noutput-file T.out,\n"
              f"output-list {' '.join(c + '%D1.6.1' for c in columns)};\n")
    script = header + script_body
    d = workdir("ct")
    try:
        (d / "P.hack").write_text(hack)
        (d / "T.tst").write_text(script)
        r = subprocess.run(["timeout", "600", "sh", str(TOOLS / "CPUEmulator.sh"), str(d / "T.tst")],
                           capture_output=True)
        console = (r.stdout + r.stderr).decode("utf-8", "replace").strip()
        out = d / "T.out"
        java_rows = parse_java_out(out.read_text()) if out.exists() else []
        java_error = None if console.endswith("End of script") else console
        r2 = subprocess.run([str(OUR_TRACE), str(d / "T.tst")], capture_output=True, timeout=600)
        if r2.returncode != 0:
            raise RuntimeError("trace failed: " + r2.stderr.decode()[-2000:])
        our_rows, our_error = [], None
        for line in r2.stdout.decode().splitlines():
            if line.startswith("ERROR "):
                our_error = line[6:]
            else:
                our_rows.append([int(x) for x in line.split()])
        return java_rows, java_error, our_rows, our_error
    finally:
        shutil.rmtree(d, ignore_errors=True)


def pmap(fn, items):
    with cf.ThreadPoolExecutor(JOBS) as ex:
        return list(ex.map(fn, items))


# ---------------------------------------------------- phase 1: assembler ----


def edge_case_files():
    big = b"D=M\n"
    return {
        "bom": b"\xef\xbb\xbf@1\n",
        "bom-blank": b"\xef\xbb\xbf\n@1\n",
        "cr-only": b"@1\r@2\rD=M",
        "no-final-newline": b"@1\nD=M",
        "empty": b"",
        "comment-only": b"// nothing",
        "invalid-utf8": b"@a\xffb\n@a\xfeb\n@1 // \xff\n",
        "nul-in-comment": b"@1 // \x00\n",
        "32767-instructions": big * 32767 + b"(END)\n",
        "32768-instructions": big * 32768,
        "32769-instructions": big * 32769,
        "32767-variables": b"".join(b"@v%d\n" % i for i in range(32767)),
        "unicode-digits": "@٣\n@1１\n@-١\n@\U0001D7CE\n".encode(),
        "word-chars": "@a b\n@a​b\n@中\n@x\U0001F600\n".encode(),
    }


def gen_valid_program(rng: random.Random):
    """A random but valid program exercising symbols, labels and every mnemonic."""
    comps = ["0", "1", "-1", "D", "A", "!D", "!A", "-D", "-A", "D+1", "A+1", "D-1", "A-1",
             "D+A", "D-A", "A-D", "D&A", "D|A", "M", "!M", "-M", "M+1", "M-1", "D+M", "D-M",
             "M-D", "D&M", "D|M", "A+D", "A&D", "A|D", "M+D", "M&D", "M|D"]
    dests = ["", "M", "D", "MD", "A", "AM", "AD", "AMD"]
    jumps = ["", "JGT", "JEQ", "JGE", "JLT", "JNE", "JLE", "JMP"]
    names = ["LOOP", "END", "x", "y", "i", "R3", "SP", "SCREEN", "KBD", "a.b", "c$d", "e:f",
             "_g", "n1", "1st", "a+b", "-x", "R16", "THAT", "sum", "Δ", "@", ";"]
    lines = []
    for _ in range(rng.randint(1, 40)):
        k = rng.random()
        ws = lambda: rng.choice(["", "", " ", "\t", "  "])
        if k < 0.35:
            if rng.random() < 0.5:
                v = rng.choice([0, 1, 15, 16, 255, 16384, 24576, 32767, -1, -32768,
                                rng.randint(0, 32767), rng.randint(32768, 99999)])
                lines.append(f"{ws()}@{ws()}{v}")
            else:
                lines.append(f"{ws()}@{rng.choice(names)}")
        elif k < 0.45:
            lines.append(f"({ws()}{rng.choice(names[:16])}{ws()})")
        elif k < 0.95:
            d, c, j = rng.choice(dests), rng.choice(comps), rng.choice(jumps)
            s = (f"{d}{ws()}={ws()}" if d or rng.random() < 0.05 else "") + c
            if j or rng.random() < 0.05:
                s += f"{ws()};{ws()}{j}"
            lines.append(ws() + s + ws())
        else:
            lines.append(rng.choice(["", "// comment", "   ", "\t// x"]))
        if rng.random() < 0.15:
            lines[-1] += " // " + rng.choice(["note", "héllo", "a // b", ""])
    eol = rng.choice(["\n", "\n", "\r\n"])
    return (eol.join(lines) + rng.choice([eol, ""])).encode()


def phase_assembler(rng, quick):
    cases = []
    for root in ([COURSE / "projects"] if (COURSE / "projects").exists() else []) + [FIXTURES]:
        for p in sorted(root.rglob("*.asm")):
            cases.append((str(p.relative_to(root.parent)), p.read_bytes()))
    for name, src in edge_case_files().items():
        cases.append(("edge:" + name, src))
    for i in range(60 if quick else 300):
        cases.append((f"generated:{i}", gen_valid_program(rng)))

    def compare(case):
        name, src = case
        j, jcon = java_assemble(src)
        o, oline = our_assemble(src)
        if (j is None) != (o is None):
            return f"{name}: Java {'accepts' if j is not None else 'rejects: ' + jcon.strip()[-200:]}, " \
                   f"we {'accept' if o is not None else f'reject (line {oline})'}"
        if j is not None and j != o:
            jl, ol = j.splitlines(), o.splitlines()
            first = next((i for i in range(min(len(jl), len(ol))) if jl[i] != ol[i]), min(len(jl), len(ol)))
            return f"{name}: output differs at word {first} ({len(jl)} vs {len(ol)} words)"
        if j is None:
            jline = java_first_error_line(src)
            if jline != oline:
                return f"{name}: first error on line {jline} in Java, {oline} here"
        return None

    results = pmap(compare, cases)
    bad = [r for r in results if r]
    for b in bad:
        note("assembler: " + b)
    accepted = sum(1 for (n, s) in cases if not n.startswith("edge:"))
    log(f"assembler: {len(cases)} programs compared ({accepted} files/generated, "
        f"{len(cases) - accepted} edge cases), {len(bad)} mismatches")
    return len(cases), len(bad)


# ---------------------------------------------------------- phase 2: cpu ----

# The official test-script language allows at most 20 items in an output-list.
WATCH = [f"RAM[{i}]" for i in range(15)] + ["RAM[16384]", "RAM[24576]"]


def assemble_with_java(path: Path):
    hack, con = java_assemble(path.read_bytes())
    if hack is None:
        raise RuntimeError(f"{path} does not assemble with Java: {con}")
    return hack


def steps(n):
    return f"repeat {n} {{ ticktock; output; }}\n"


def sets(pairs):
    return "".join(f"set RAM[{a}] {v},\n" for a, v in pairs)


def course_and_fixture_traces(rng, quick):
    """(name, hack, script body, columns) for real programs."""
    traces = []
    cols = (["A", "D", "PC"] + [f"RAM[{i}]" for i in [0, 1, 2, 3, 4, 5, 16, 17, 18, 19, 100, 101, 102,
                                                      103, 200]]
            + ["RAM[16384]", "RAM[24576]"])
    long = 3000 if quick else 20000
    if (COURSE / "projects").exists():
        for p in sorted((COURSE / "projects").rglob("*.asm")):
            hack = assemble_with_java(p)
            body = sets([(0, rng.randint(0, 20)), (1, rng.randint(0, 20)), (24576, 0)]) + "output;\n" + steps(long)
            body += "set RAM[24576] 75,\n" + steps(long // 2) + "set RAM[24576] 0,\n" + steps(long // 2)
            traces.append((str(p.relative_to(COURSE)), hack, body, cols))
    inputs = {
        "Countdown": [(0, 7)],
        "SumArray": [(0, 5)] + [(100 + i, rng.randint(-32768, 32767)) for i in range(5)],
        "Fibonacci": [(0, 25)],
        "BubbleSort": [(0, 8)] + [(100 + i, rng.randint(-1000, 1000)) for i in range(8)],
        "KeyLog": [(0, 3)],
        "Diagonal": [],
        "Speed": [(i, rng.randint(-100, 100)) for i in range(16)],
        "Quirks": [],
        "LineEndings": [],
    }
    for p in sorted(FIXTURES.glob("*.asm")):
        hack = assemble_with_java(p)
        body = sets(inputs.get(p.stem, [])) + "output;\n" + steps(400)
        if p.stem == "KeyLog":
            for key in [65, 0, 66, 0, 140, 0]:
                body += f"set RAM[24576] {key},\n" + steps(60)
        else:
            body += steps(long // 4)
        traces.append(("fixture:" + p.stem, hack, body, cols))
    return traces


DOCUMENTED = {
    0b0101010: lambda d, y: 0, 0b0111111: lambda d, y: 1, 0b0111010: lambda d, y: -1,
    0b0001100: lambda d, y: d, 0b0110000: lambda d, y: y, 0b0001101: lambda d, y: ~d,
    0b0110001: lambda d, y: ~y, 0b0001111: lambda d, y: -d, 0b0110011: lambda d, y: -y,
    0b0011111: lambda d, y: d + 1, 0b0110111: lambda d, y: y + 1, 0b0001110: lambda d, y: d - 1,
    0b0110010: lambda d, y: y - 1, 0b0000010: lambda d, y: d + y, 0b0010011: lambda d, y: d - y,
    0b0000111: lambda d, y: y - d, 0b0000000: lambda d, y: d & y, 0b0010101: lambda d, y: d | y,
}
DOCUMENTED.update({c | 0x40: DOCUMENTED[c] for c in
                   [0b0110000, 0b0110001, 0b0110011, 0b0110111, 0b0110010,
                    0b0000010, 0b0010011, 0b0000111, 0b0000000, 0b0010101]})


def predicts_jump_error(word, a, d, m, latch):
    """Input selection only (the comparison never relies on it): whether the
    C-instruction would jump to a negative A from this state, and the ALU latch
    (c bits of the last documented comp) afterwards."""
    comp = (word >> 6) & 0x7F
    if comp in DOCUMENTED:
        latch = comp & 0x3F
    fn = DOCUMENTED[latch]  # the a=0 entry; the a-bit only selects y below
    out = fn(d, m if comp & 0x40 else a) & 0xFFFF
    new_a = out if word & 0x20 else a
    cls = 2 if out == 0 else 4 if out & 0x8000 else 1
    return bool(word & 7 & cls) and bool(new_a & 0x8000), latch


def sweep_traces(rng):
    """Every comp (128) x dest (8) x jump (8) code, each executed from a fresh
    random state by setting PC, A, D and RAM[A] before one ticktock.

    An emulator error ends a script, so states are chosen to avoid jumps to a
    negative address; words that jump there from every state tried get a
    script of their own."""
    words = [0xE000 | (c << 6) | (d << 3) | j for c in range(128) for d in range(8) for j in range(8)]
    traces = []
    cols = ["A", "D", "PC"] + WATCH

    def pick():
        a = rng.choice([rng.randint(0, 14), rng.randint(0, 14), 16384, 24576, 0, 1])
        d = rng.choice([0, 1, -1, -32768, 32767, rng.randint(-32768, 32767)])
        m = rng.choice([0, 1, -1, -32768, 32767, rng.randint(-32768, 32767)])
        return a, d, m

    def row(k, a, d, m):
        return f"set PC {k}, set A {a}, set D {d}, set RAM[{a}] {m}, ticktock, output;\n"

    erroring = []
    for chunk in range(0, len(words), 2048):
        part = words[chunk:chunk + 2048]
        body = []
        latch = 0  # each script runs in a fresh emulator: D&A
        for k, w in enumerate(part):
            for _ in range(30):
                a, d, m = pick()
                err, new_latch = predicts_jump_error(w, a, d & 0xFFFF, m & 0xFFFF, latch)
                if not err:
                    break
            else:
                erroring.append((w, a, d, m))
                a, d, m = 0, 0, 0
                w = part[k] = 0xE000  # D&A, no dest, no jump: harmless filler
                new_latch = 0
            latch = new_latch
            body.append(row(k, a, d, m))
        traces.append((f"sweep:{chunk // 2048}", hack_text(part), "".join(body), cols))
    for w, a, d, m in erroring:
        traces.append((f"sweep-error:{w:016b}", hack_text([w]), row(0, a, d, m), cols))
    return traces


def random_stream(rng: random.Random, n=48):
    words = []
    for _ in range(n):
        k = rng.random()
        if k < 0.4:
            v = rng.choice([rng.randint(0, n - 1)] * 4 + [rng.randint(0, 15)] * 3 + [
                16384, 16385, 24575, 24576, 24577, 30000, 32767, rng.randint(0, 32767)])
            words.append(v)
        elif k < 0.97:
            words.append(0xE000 | rng.randint(0, 0x1FFF))
        else:
            words.append(rng.choice([0x8000, rng.randint(0x8000, 0xDFFF)]))
    return words


def stream_traces(rng, count):
    traces = []
    cols = ["A", "D", "PC"] + WATCH
    for i in range(count):
        words = random_stream(rng)
        body = sets([(a, rng.randint(-32768, 32767)) for a in range(16)]) + "output;\n" + steps(300)
        traces.append((f"stream:{i}", hack_text(words), body, cols))
    return traces


def phase_cpu(rng, quick):
    traces = course_and_fixture_traces(rng, quick) + sweep_traces(rng) + stream_traces(rng, 60 if quick else 400)

    def compare(t):
        name, hack, body, cols = t
        jr, je, orows, oe = run_trace(hack, body, cols)
        problems = []
        if len(jr) != len(orows):
            problems.append(f"{len(jr)} rows from Java, {len(orows)} here")
        for i, (a, b) in enumerate(zip(jr, orows)):
            if a != b:
                diff = {cols[k]: (a[k], b[k]) for k in range(len(cols)) if a[k] != b[k]}
                problems.append(f"row {i} differs (Java, ours): {diff}")
                break
        if (je is None) != (oe is None):
            problems.append(f"error: Java {je!r}, ours {oe!r}")
        return name, min(len(jr), len(orows)), problems, je is not None

    results = pmap(compare, traces)
    rows = sum(r[1] for r in results)
    errors = sum(1 for r in results if r[3])
    bad = [r for r in results if r[2]]
    for name, _, problems, _ in bad:
        note(f"cpu: {name}: " + "; ".join(problems))
    log(f"cpu: {len(traces)} programs, {rows} states compared "
        f"({errors} runs ended in an emulator error), {len(bad)} mismatches")
    return len(traces), rows, len(bad)


# ------------------------------------------------------ phase 3: invalid ----

INVALID_LINES = [
    "d=a", "D=a", "DM=M", "MA=M", "null=M", "D=1+M", "D=M+0", "D=D+D", "D=1-A", "D=-D-1",
    ";JMP", "0;jmp", "D;JMP;", "D=M;;JMP", "D==M", "D=M=A", "D=", "=", ";", "=;JMP", "D=;JMP",
    "JMP", "D=M;null", "D=M;JMP x", "@", "@ ", "@a;JMP", "@a=b", "@1/2", "@1/", "@aéb", "@a#b",
    "@(x)", '@"x"', "@1\x0c", "@x\x00", "(L", "L)", "()", "(L)D=M", "((L)", "(a;b)", "(a=b)",
    "(a/b)", "(L//)", "(L)(M)", "(a#b)", "(L)x", "/ c", "/* c */", "\x0c@1", "@1 ",
    "M=D+1;JGT JMP", "AMD=M+1;", "LOOP", "@LOOP; 0;JMP", "D=!", "D=M+", "#", "é", "@-",
    "D=M;jgt", "AM=M+1;JMPP", "D=M JMP", "D=M|", "+", "D;J",
]


def gen_fuzz_line(rng: random.Random):
    pieces = {
        "dest": ["", "D", "M", "A", "MD", "AM", "AD", "AMD", "DM", "d", "null", "MA", "X", "DD"],
        "comp": ["0", "1", "-1", "D", "A", "M", "D+1", "M-1", "D+M", "M+D", "D&A", "A|D", "!M",
                 "1+M", "D+D", "M+0", "d", "-D-1", "", "A+A", "!0", "D-D", "0x1", "m", "D+a"],
        "jump": ["JMP", "JGT", "JEQ", "JNE", "", "jmp", "null", "JMPP", "J", "JMP;"],
        "sym": ["x", "LOOP", "1x", "a+b", "é", "Δ", "#", ";", "=", "(", ")", "a.b", "-", "+5",
                "32768", "-32769", "007", "٣", "", "a b", "a/b", "a//b", "R16", "SP"],
    }
    k = rng.random()
    ch = rng.choice
    if k < 0.3:
        line = "@" + ch(pieces["sym"])
    elif k < 0.45:
        line = ch(["(", ""]) + ch(pieces["sym"]) + ch([")", "", "))"])
    else:
        line = ""
        if rng.random() < 0.7:
            line += ch(pieces["dest"]) + ch(["=", "=", "==", ""])
        line += ch(pieces["comp"])
        if rng.random() < 0.5:
            line += ch([";", ";", ";;", ""]) + ch(pieces["jump"])
    # Sprinkle whitespace and odd characters.
    out = []
    for c in line:
        if rng.random() < 0.1:
            out.append(ch([" ", "\t", " ", "\x0c", "/"]))
        out.append(c)
    if rng.random() < 0.2:
        out.append(ch([" // c", "//", "/", " /x"]))
    return "".join(out)


def base_program(rng):
    return ["@2", "D=A", "(LOOP)", "@x", "M=D", "@LOOP", "D;JGT", "@END", "0;JMP", "(END)", "@END", "0;JMP"]


def phase_invalid(rng, quick):
    cases = []
    for line in INVALID_LINES:
        lines = base_program(rng)
        pos = rng.randint(0, len(lines))
        lines.insert(pos, line)
        cases.append((f"invalid:{line!r}", ("\n".join(lines) + "\n").encode()))
    for i in range(100 if quick else 600):
        lines = base_program(rng)
        for _ in range(rng.randint(1, 3)):
            lines.insert(rng.randint(0, len(lines)), gen_fuzz_line(rng))
        cases.append((f"fuzz:{i}", ("\n".join(lines) + "\n").encode()))

    def compare(case):
        name, src = case
        j, jcon = java_assemble(src)
        o, oline = our_assemble(src)
        if (j is None) != (o is None):
            return name, "accept/reject differs: Java " + ("accepts" if j is not None else "rejects: " + jcon.strip()[-150:]) + \
                   ", we " + ("accept" if o is not None else f"reject (line {oline})"), j is None, src
        if j is not None:
            return name, (None if j == o else "accepted, but output differs"), False, src
        jline = java_first_error_line(src)
        return name, (None if jline == oline else f"first error line: Java {jline}, ours {oline}"), True, src

    results = pmap(compare, cases)
    rejected = sum(1 for r in results if r[2])
    bad = [r for r in results if r[1]]
    for name, problem, _, src in bad:
        note(f"invalid: {name}: {problem}\n    source: {src!r}")
    log(f"invalid: {len(cases)} programs ({rejected} rejected by Java, "
        f"{len(cases) - rejected} accepted), {len(bad)} mismatches")
    return len(cases), len(bad)


# ------------------------------------------------------------------ main ----


def main():
    global OUR_ASM, OUR_TRACE, JOBS
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--seed", type=int, default=2026)
    ap.add_argument("--quick", action="store_true", help="fewer generated programs")
    ap.add_argument("--jobs", type=int, default=3)
    ap.add_argument("--phase", choices=["asm", "cpu", "invalid"], action="append")
    args = ap.parse_args()
    JOBS = args.jobs

    if not (TOOLS / "Assembler.sh").exists() or not (TOOLS / "CPUEmulator.sh").exists():
        print(f"skipping: no official tools in {TOOLS} (set N2T_COURSE_DIR to the course directory)")
        return 0
    if shutil.which("java") is None:
        print("skipping: java is not on the PATH")
        return 0
    if "." in str(SCRATCH):
        print(f"N2T_SCRATCH_DIR {SCRATCH} contains a '.'; the official assembler cannot write there")
        return 2
    SCRATCH.mkdir(parents=True, exist_ok=True)

    log("building n2t-hack examples ...")
    subprocess.run(["cargo", "build", "--release", "-q", "-j", "3", "-p", "n2t-hack", "--examples"],
                   cwd=REPO, check=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR", REPO / "target"))
    OUR_ASM = target / "release" / "examples" / "asm"
    OUR_TRACE = target / "release" / "examples" / "trace"

    phases = args.phase or ["asm", "cpu", "invalid"]
    total_bad = 0
    if "asm" in phases:
        total_bad += phase_assembler(random.Random(args.seed), args.quick)[1]
    if "cpu" in phases:
        total_bad += phase_cpu(random.Random(args.seed + 1), args.quick)[2]
    if "invalid" in phases:
        total_bad += phase_invalid(random.Random(args.seed + 2), args.quick)[1]

    rep = SCRATCH / "report.txt"
    rep.write_text("\n".join(report_lines) + "\n")
    log(f"{'FAIL' if total_bad else 'OK'}: {total_bad} mismatches (details in {rep})")
    return 1 if total_bad else 0


if __name__ == "__main__":
    sys.exit(main())
