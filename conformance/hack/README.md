# Hack conformance

`run.py` compares `n2t-hack` with the official Java assembler and CPU emulator.
It is a local tool: it needs the course distribution (which is not in this
repository) and Java, so CI does not run it. Without them it prints a message
and exits 0.

```sh
N2T_COURSE_DIR=~/n2t-private/course python3 conformance/hack/run.py          # full run
python3 conformance/hack/run.py --quick                                       # fewer generated programs
python3 conformance/hack/run.py --phase cpu --seed 7                          # one phase, other seed
```

| Variable | Default | Meaning |
|---|---|---|
| `N2T_COURSE_DIR` | `~/n2t-private/course` | course tree with `tools/` and `projects/` |
| `N2T_SCRATCH_DIR` | `~/n2t-private/tmp/hack/conformance` | scratch files and `report.txt`; must not contain a `.` (the Java assembler then silently writes nothing) |

Phases (all inputs are generated or read at run time; nothing from the course
is stored here):

1. **asm**: every `.asm` in the course projects and in `crates/hack/tests/fixtures`,
   whole-file edge cases (BOM, CR-only line ends, invalid UTF-8, 32767/32768/32769
   instructions, 32767 variables, Unicode digits) and generated valid programs are
   assembled by both tools; the `.hack` output must be identical, and where both
   reject, the first error must be on the same line.
2. **cpu**: course programs, fixtures, an exhaustive sweep of all 128 × 8 × 8
   comp/dest/jump codes and random instruction streams (including illegal words
   and out-of-range addresses) are run by a generated `.tst` script in the Java
   CPU emulator and by `cargo run --example trace` on the same script. A, D, PC
   and up to 17 RAM cells are compared after every `ticktock`, as is whether
   and where an emulator error stops the script.
3. **invalid**: hand-written invalid lines and fuzzed lines are inserted into a
   valid program. Accept/reject must agree; accepted programs must produce the
   same `.hack`; for rejected ones the first error line must agree. Java's
   messages do not always carry a correct line number, so its first error line
   is found by bisection over prefixes of the file.

The script exits 1 if anything differs and writes the details to
`$N2T_SCRATCH_DIR/report.txt`.
