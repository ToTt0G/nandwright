#!/usr/bin/env python3
"""Checks that the repository holds no Nand2Tetris course material.

The course asks students not to publish solutions, and its tools, project
files, tests and OS are not ours to redistribute. This script fails if a
tracked file (or, with --history, any file in any commit on any branch):

  * implements a course chip (a `CHIP <course name>` with a non-empty PARTS);
  * is named like a course program, test, compare file or OS class;
  * carries the header the course puts on its own files;
  * with --course DIR (repeatable): has the same content as a file under DIR
    (the course distribution or a folder of private solutions).

Usage:
  python3 scripts/check-no-course-material.py [--history [--ref REF]...] [--course DIR]...
"""

import argparse
import hashlib
import os
import re
import subprocess
import sys

COURSE_CHIPS = """Not And Or Xor Mux DMux Not16 And16 Or16 Mux16 Or8Way Mux4Way16 Mux8Way16
DMux4Way DMux8Way HalfAdder FullAdder Add16 Inc16 ALU Bit Register RAM8 RAM64 RAM512 RAM4K
RAM16K PC Memory CPU Computer""".split()

OS_CLASSES = "Math String Array Output Screen Keyboard Memory Sys".split()

# Programs the course supplies or assigns.
COURSE_PROGRAMS = "mult fill add max maxl rect rectl pong pongl".split()

COURSE_HEADER = re.compile(r"This file is part of (www\.)?nand2tetris\.org", re.I)

CHIP = re.compile(r"\bCHIP\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{(.*?)\}", re.S)

# This file names the things it looks for.
SELF = "scripts/check-no-course-material.py"

TEXT_EXTENSIONS = {
    ".hdl", ".tst", ".cmp", ".out", ".asm", ".hack", ".vm", ".jack", ".xml", ".md", ".rs", ".ts",
    ".js", ".mjs", ".json", ".py", ".toml", ".txt", ".yml", ".yaml", ".html", ".css", ".expected",
}


def strip_comments(text):
    text = re.sub(r"/\*.*?\*/", " ", text, flags=re.S)
    return re.sub(r"//[^\n]*", " ", text)


def problems_in(path, data):
    """Yields descriptions of course material in one file."""
    name = os.path.basename(path)
    stem, extension = os.path.splitext(name)
    extension = extension.lower()
    if path == SELF:
        return
    if extension in {".tst", ".cmp"} and stem in COURSE_CHIPS:
        yield f"is named like the official test of the course chip {stem}"
    if extension == ".asm" and stem.lower() in COURSE_PROGRAMS:
        yield "is named like a course assembly program"
    if extension in {".jack", ".vm"} and stem in OS_CLASSES:
        yield f"is named like the OS class {stem} (project 12)"
    if extension not in TEXT_EXTENSIONS:
        return
    text = data.decode("utf-8", errors="replace")
    if COURSE_HEADER.search(text):
        yield "carries the header of the course's own files"
    for match in CHIP.finditer(strip_comments(text)):
        chip, body = match.group(1), match.group(2)
        if chip not in COURSE_CHIPS:
            continue
        parts = re.search(r"\bPARTS\s*:(.*)", body, re.S)
        if parts and "(" in parts.group(1):
            yield f"implements the course chip {chip}"


def normalized_hash(data):
    """A hash that ignores line endings, trailing space and blank lines."""
    lines = [line.rstrip() for line in data.replace(b"\r\n", b"\n").replace(b"\r", b"\n").split(b"\n")]
    body = b"\n".join(line for line in lines if line)
    return hashlib.sha256(body).hexdigest() if len(body) >= 40 else None


def course_hashes(directories):
    hashes = {}
    for directory in directories:
        for root, _, files in os.walk(os.path.expanduser(directory)):
            for name in files:
                if os.path.splitext(name)[1].lower() not in TEXT_EXTENSIONS:
                    continue
                full = os.path.join(root, name)
                try:
                    with open(full, "rb") as handle:
                        digest = normalized_hash(handle.read())
                except OSError:
                    continue
                if digest:
                    hashes.setdefault(digest, full)
    return hashes


def git(*args):
    return subprocess.run(["git", *args], check=True, capture_output=True).stdout


def tracked():
    for path in git("ls-files", "-z").decode().split("\0"):
        if path and os.path.isfile(path):
            with open(path, "rb") as handle:
                yield path, "the working tree", handle.read()


def history(refs):
    """Every (path, blob) in every commit reachable from `refs` (or from any ref)."""
    seen = set()
    for commit in git("rev-list", *(refs or ["--all"])).decode().split():
        for line in git("ls-tree", "-r", "-z", commit).decode().split("\0"):
            if not line:
                continue
            meta, path = line.split("\t", 1)
            _, kind, blob = meta.split()
            if kind != "blob" or (path, blob) in seen:
                continue
            seen.add((path, blob))
            yield path, f"commit {commit[:9]}", git("cat-file", "blob", blob)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--history", action="store_true", help="check every commit on every branch, not only the working tree")
    parser.add_argument("--ref", action="append", default=[], metavar="REF", help="with --history: check only commits reachable from REF (repeatable); default: every ref")
    parser.add_argument("--course", action="append", default=[], metavar="DIR", help="a course tree or solutions folder to compare contents with")
    options = parser.parse_args()
    os.chdir(git("rev-parse", "--show-toplevel").decode().strip())

    known = course_hashes(options.course)
    found = []
    checked = 0
    for path, where, data in (history(options.ref) if options.history else tracked()):
        checked += 1
        for problem in problems_in(path, data):
            found.append(f"{path} ({where}) {problem}")
        digest = normalized_hash(data) if known else None
        if digest in known:
            found.append(f"{path} ({where}) has the same content as {known[digest]}")
    for line in sorted(set(found)):
        print(f"course material: {line}")
    scope = "the working tree"
    if options.history:
        scope = f"every commit reachable from {', '.join(options.ref)}" if options.ref else "every commit on every ref"
    compared = f", compared with {len(known)} course/solution files" if known else ""
    print(f"checked {checked} files in {scope}{compared}: {len(set(found))} problem(s)")
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
