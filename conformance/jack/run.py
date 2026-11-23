#!/usr/bin/env python3
"""Differential test of n2t-jack against the official Java JackCompiler.

Compares, per directory of .jack files:
  * byte-identical .vm output (modulo line endings),
  * the same accept/reject decision per file (a file is rejected when the
    official compiler prints any message that is not a warning),
  * the same line for the first error.

Inputs:
  1. every directory with .jack files under $N2T_COURSE_DIR/projects,
  2. every directory under crates/jack/tests/fixtures,
  3. the generated corpus in cases.py (valid and invalid snippets).

Nothing is written inside the course tree or the repository: inputs are copied
to a scratch directory first. Exits 0 when there is no difference, 1 otherwise,
and 0 with a message when N2T_COURSE_DIR (or java) is unavailable.

Usage: python3 conformance/jack/run.py [-v] [--keep]
"""

import os
import re
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)
sys.dont_write_bytecode = True
import cases  # noqa: E402

VERBOSE = "-v" in sys.argv
ALL_LINES = []
XML_RESULTS = []
KEEP = "--keep" in sys.argv


def course_dir():
    d = os.environ.get("N2T_COURSE_DIR") or os.path.expanduser("~/n2t-private/course")
    tool = os.path.join(d, "tools", "JackCompiler.sh")
    if not os.path.isfile(tool):
        return None, None
    return d, tool


def run_java(tool, directory):
    p = subprocess.run(
        ["timeout", "300", "sh", tool, directory],
        capture_output=True,
        text=True,
        errors="replace",
    )
    msgs = {}
    for line in (p.stdout + p.stderr).splitlines():
        m = re.match(r"In (.+?)\.jack \(line (-?\d+)\): (.*)", line)
        if m:
            msgs.setdefault(m.group(1), []).append((int(m.group(2)), m.group(3)))
    return msgs


def run_ours(jackc, directory):
    p = subprocess.run(
        [jackc, "--lenient", "--report", directory],
        capture_output=True,
        text=True,
        errors="replace",
    )
    diags, status = {}, {}
    for line in p.stdout.splitlines():
        parts = line.split("\t")
        if parts[0] == "DIAG" and len(parts) >= 6:
            stem = os.path.splitext(os.path.basename(parts[1]))[0]
            diags.setdefault(stem, []).append((parts[2], int(parts[3]), int(parts[4]), parts[5]))
        elif parts[0] == "FILE" and len(parts) >= 3:
            stem = os.path.splitext(os.path.basename(parts[1]))[0]
            status[stem] = parts[2]
    if p.returncode not in (0, 1):
        print("jackc crashed on", directory, p.stderr[-2000:])
    return diags, status


def read_vm(path):
    if not os.path.exists(path):
        return None
    with open(path, "rb") as f:
        return f.read().replace(b"\r\n", b"\n")


def compare_dir(label, java_dir, ours_dir, stems, tool, jackc, results):
    java = run_java(tool, java_dir)
    ours, status = run_ours(jackc, ours_dir)
    for stem in sorted(stems):
        jmsgs = java.get(stem, [])
        jerrors = [m for m in jmsgs if not m[1].split(": ")[-1].startswith("Warning")
                   and "Warning:" not in m[1]]
        java_rejects = bool(jerrors)
        odiags = ours.get(stem, [])
        oerrors = [d for d in odiags if d[0] == "error"]
        ours_rejects = bool(oerrors)
        jvm = read_vm(os.path.join(java_dir, stem + ".vm"))
        ovm = read_vm(os.path.join(ours_dir, stem + ".vm"))
        problems = []
        known = None
        if (not java_rejects and ours_rejects and jvm is not None
                and all("silently writes an incomplete" in d[3] for d in oerrors)
                and not jvm.rstrip().endswith(b"return")):
            # Deliberate deviation: beyond 100 nested expressions the official
            # compiler prints nothing and writes a truncated .vm file.
            known = "java writes a truncated .vm without a message (expressions nested > 100)"
        if (not known and not java_rejects and ours_rejects and jvm is not None
                and any("which this compiler does not support" in d[3] for d in oerrors)):
            # Deliberate deviation: a fixed nesting limit protects the stack.
            known = "nested more than %d levels (this compiler's stack guard)" % 128
        if known:
            pass
        elif java_rejects != ours_rejects:
            problems.append(
                "accept/reject differs: java %s, ours %s"
                % ("rejects" if java_rejects else "accepts", "rejects" if ours_rejects else "accepts")
            )
        if known:
            pass
        elif (jvm is None) != (ovm is None):
            problems.append("vm written by %s only" % ("java" if jvm is not None else "ours"))
        elif jvm is not None and jvm != ovm and not known:
            jl, ol = jvm.split(b"\n"), ovm.split(b"\n")
            i = next((k for k in range(min(len(jl), len(ol))) if jl[k] != ol[k]), min(len(jl), len(ol)))
            problems.append(
                "vm differs at line %d: java %r, ours %r"
                % (i + 1, jl[i] if i < len(jl) else b"<eof>", ol[i] if i < len(ol) else b"<eof>")
            )
        if java_rejects and ours_rejects:
            jfirst = jerrors[0][0]
            ofirst = next((d[1] for d in oerrors if d[1] > 0), None)
            if jfirst != ofirst:
                problems.append(
                    "first error line differs: java %d (%s), ours %s (%s)"
                    % (jfirst, jerrors[0][1], ofirst, oerrors[0][3][:100])
                )
        all_lines = [l for l, _ in jerrors] == [d[1] for d in oerrors if d[1] > 0]
        if not all_lines and VERBOSE and java_rejects:
            print("  all-lines differ %s/%s: java %s ours %s" % (label, stem, [l for l, _ in jerrors],
                  [d[1] for d in oerrors if d[1] > 0]))
        ALL_LINES.append(all_lines)
        results.append((label, stem, java_rejects, jvm is not None, problems, jerrors, oerrors, known))


def collect_dirs(root):
    out = []
    for d, _, files in os.walk(root):
        if any(f.endswith(".jack") for f in files):
            out.append(d)
    return sorted(out)


def main():
    course, tool = course_dir()
    if course is None:
        print("SKIP: N2T_COURSE_DIR is not set or has no tools/JackCompiler.sh; "
              "the Jack conformance test needs the official tools.")
        return 0
    if shutil.which("java") is None:
        print("SKIP: java is not installed.")
        return 0
    print("building jackc ...", flush=True)
    b = subprocess.run(
        ["cargo", "build", "-q", "-j", "3", "-p", "n2t-jack", "--example", "jackc"],
        cwd=REPO,
    )
    if b.returncode != 0:
        return 1
    jackc = os.path.join(REPO, "target", "debug", "examples", "jackc")
    scratch_base = os.environ.get("N2T_SCRATCH") or os.path.expanduser("~/n2t-private/tmp/jack")
    os.makedirs(scratch_base, exist_ok=True)
    scratch = tempfile.mkdtemp(prefix="conf.", dir=scratch_base)
    results = []
    groups = []
    try:
        dirs = [("course", d) for d in collect_dirs(os.path.join(course, "projects"))]
        dirs += [("fixtures", d) for d in collect_dirs(os.path.join(REPO, "crates", "jack", "tests", "fixtures"))]
        for n, (label, d) in enumerate(dirs):
            stems = [f[:-5] for f in os.listdir(d) if f.endswith(".jack")]
            jd = os.path.join(scratch, "j%d" % n)
            od = os.path.join(scratch, "o%d" % n)
            os.makedirs(jd)
            os.makedirs(od)
            for s in stems:
                shutil.copy(os.path.join(d, s + ".jack"), jd)
                shutil.copy(os.path.join(d, s + ".jack"), od)
            rel = os.path.relpath(d, course if label == "course" else REPO)
            before = len(results)
            compare_dir(label + ":" + rel, jd, od, stems, tool, jackc, results)
            groups.append((label, rel, results[before:]))
        # Project-10 analyzer XML against the course's reference files.
        xml_dirs = [d for d in collect_dirs(os.path.join(course, "projects", "10"))
                    if any(f.endswith("T.xml") for f in os.listdir(d))]
        for n, d in enumerate(xml_dirs):
            xd = os.path.join(scratch, "x%d" % n)
            os.makedirs(xd)
            for f in os.listdir(d):
                if f.endswith(".jack"):
                    shutil.copy(os.path.join(d, f), xd)
            subprocess.run([jackc, "--check", "--xml", "--out", xd, xd], capture_output=True)
            for f in sorted(os.listdir(d)):
                if not f.endswith(".xml"):
                    continue
                ref = read_vm(os.path.join(d, f))
                ours = read_vm(os.path.join(xd, f))
                XML_RESULTS.append((os.path.relpath(os.path.join(d, f), course), ref == ours))
        # Generated corpus: each snippet is its own class in one directory.
        jd = os.path.join(scratch, "jcases")
        od = os.path.join(scratch, "ocases")
        os.makedirs(jd)
        os.makedirs(od)
        stems = []
        for i, src in enumerate(cases.CASES):
            stem = "Case%03d" % i
            text = src.replace("CLS", stem)
            for d in (jd, od):
                with open(os.path.join(d, stem + ".jack"), "w", newline="", encoding="utf-8") as f:
                    f.write(text)
            stems.append(stem)
        before = len(results)
        compare_dir("cases", jd, od, stems, tool, jackc, results)
        groups.append(("cases", "cases.py", results[before:]))
    finally:
        if not KEEP:
            shutil.rmtree(scratch, ignore_errors=True)
        else:
            print("kept", scratch)

    # Summary.
    print()
    print("%-44s %6s %6s %8s %9s %6s" % ("group", "files", "same", "differ", "rejected", "vm"))
    total = same = differ = rejected = vms = 0
    for label, rel, rs in groups:
        n = len(rs)
        bad = sum(1 for r in rs if r[4])
        rej = sum(1 for r in rs if r[2])
        vm = sum(1 for r in rs if r[3])
        total += n
        differ += bad
        same += n - bad
        rejected += rej
        vms += vm
        if VERBOSE or bad or label != "course" or True:
            print("%-44s %6d %6d %8d %9d %6d" % ((label + ":" + rel)[-44:], n, n - bad, bad, rej, vm))
    print("%-44s %6d %6d %8d %9d %6d" % ("TOTAL", total, same, differ, rejected, vms))
    for label, stem, jrej, jvm, problems, jerr, oerr, known in results:
        if known:
            print("KNOWN %s %s: %s" % (label, stem, known))
        if problems:
            print("DIFF %s %s:" % (label, stem))
            for p in problems:
                print("    " + p)
            if VERBOSE:
                print("    java:", jerr[:3])
                print("    ours:", [(d[1], d[3][:80]) for d in oerr[:3]])
    vm_compared = sum(1 for r in results if r[3] and not r[7])
    known_n = sum(1 for r in results if r[7])
    invalid = sum(1 for r in results if r[2])
    print()
    print("files compared: %d, identical behaviour: %d (of which %d documented deviations), different: %d"
          % (total, same, known_n, differ))
    print(".vm files byte-compared: %d; files rejected by the official compiler: %d" % (vm_compared, invalid))
    print("files where every reported error line also matches (informational): %d of %d"
          % (sum(ALL_LINES), len(ALL_LINES)))
    xml_bad = [p for p, ok in XML_RESULTS if not ok]
    print("project-10 XML files compared: %d, identical: %d, different: %d"
          % (len(XML_RESULTS), len(XML_RESULTS) - len(xml_bad), len(xml_bad)))
    for p in xml_bad:
        print("XML DIFF", p)
    return 1 if differ or xml_bad else 0


if __name__ == "__main__":
    sys.exit(main())
