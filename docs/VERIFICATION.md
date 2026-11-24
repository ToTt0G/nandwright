# How agreement with the official tools is measured

Nandwright's claim is that it gives the results the official Java tools give. This page
says what has been compared, with what numbers, and how to repeat it. The deliberate
differences are in [DEVIATIONS.md](DEVIATIONS.md).

The comparisons need the course distribution (the official tools and project files) and,
for the hardware projects, solved chips. Neither is in this repository, and neither may be
added to it: the harness reads them from `N2T_COURSE_DIR` and `N2T_PRIVATE_DIR` on your own
machine. Without them every script below prints a message and exits successfully, which is
what happens in CI.

Ground truth: the classic course distribution, the one with `tools/HardwareSimulator.sh`,
`CPUEmulator.sh`, `VMEmulator.sh`, `Assembler.sh` and `JackCompiler.sh`, run with Java 17 on
Linux. The numbers below are from runs on 23 November 2026.

## 1. Test scripts, all dialects: `conformance/run.py`

Runs every script through the right Java tool and through `n2t test --json`, and compares
the outcome (passed, comparison failure, error), the failing compare line, the number of
lines compared, the script line of an error where Java reports one, and the bytes of the
`.out` file after normalising line endings.

| Scenario | Cases | Same as Java | Deliberately different |
|---|---|---|---|
| Solved chips of projects 1, 2, 3, 5 and the demo; CPU Emulator tests of project 4 | 42 | 41 | 1 (`Memory.tst`: Nandwright presses the keys) |
| The untouched course tree (unwritten chips) | 42 | 42 | |
| 23 hand-written broken chips and scripts | 23 | 22 | 1 (Java reports the line after the mistake) |
| 180 generated chips, each a solved chip with one mutation (swapped pins, wrong gate, missing part, wrong bus slice, wrong constant, undriven pin, feedback loop, width mismatch, misspelled name) | 180 | 180 | |
| VM Emulator scripts of projects 7, 8 and 12 that run on the files the course ships | 14 | 14 | |
| **Total** | **301** | **299** | **2** |

Fourteen more scripts (the CPU Emulator tests of projects 7 and 8) need the output of a
student's VM translator and are skipped; they use the same dialect and emulator as the
project 4 tests.

```sh
cargo build --release -p n2t-cli
python3 conformance/run.py            # exits non-zero on any unexplained difference
```

The two deliberate differences are recorded, with reasons, in
`conformance/expected-differences.toml`.

## 2. The untouched course tree has no false errors

`n2t check <course>/projects` over all 168 `.hdl`, `.tst`, `.asm`, `.vm` and `.jack` files
of the distribution as shipped reports 0 errors and 0 warnings. (Unwritten chips and
unwritten OS subroutines are hints.) The editor uses the same analysis.

## 3. Assembler and CPU emulator: `conformance/hack/run.py`

- **Assembler**: every `.asm` of the course, the repository's fixtures, generated valid
  programs and whole-file edge cases (byte order mark, lone carriage returns, invalid UTF-8,
  programs at and over the size limit) are assembled by both; the `.hack` files must be
  byte-identical, and when both reject a file the first error must be on the same line.
- **CPU emulator**: each program is run as a generated test script through the Java CPU
  Emulator and through the engine, comparing A, D, PC and memory cells after every
  instruction, including a sweep of every `comp`/`dest`/`jump` combination and random
  instruction streams with illegal words and out-of-range addresses.
- **Invalid assembly**: hand-written and fuzzed invalid lines; accept or reject, output and
  first error line must agree.

| Run | Assembler | CPU emulator | Invalid input | Mismatches |
|---|---|---|---|---|
| `--quick --seed 99` | 92 programs | 114 programs, 85,507 machine states | 167 programs | 0 |
| full, seed 2026 (run by the crate's author) | 332 programs | 454 programs, 480,040 machine states | 667 programs | 0 |

## 4. Jack compiler: `conformance/jack/run.py`

Every `.jack` file of the course (projects 9 to 12, including the OS skeletons), the
repository's fixtures and 1,140 generated valid and invalid programs are compiled by both.

- 1,199 files compared; behaviour identical in all (5 of them are the two documented
  deep-nesting differences, which the script checks separately).
- 465 `.vm` files byte-identical.
- 768 files rejected by the Java compiler; each is rejected here with the first error on
  the same line.
- The project 10 reference output: 14 of 14 `.xml` files identical.

## 5. VM emulator and built-in OS: `conformance/vm/diff.py`

The built-in Jack OS of the Java VM Emulator exists only in its graphical program, so the
script drives that program on a virtual display. Each program runs for the same number of
VM steps in the Java emulator and in the engine; then selected RAM cells, all 8,192 screen
words and, for the error probes, the way the program stopped are compared.

| Group | Programs | Same as Java |
|---|---|---|
| The VM programs of projects 7 and 8, set up as their test scripts set them up | 11 | 11 |
| Jack programs of projects 9, 11 and 12, compiled with the Java compiler and run on the built-in OS of each emulator, with scripted key presses where a program waits for input | 16 | 16 |
| Original probe programs for every function of the built-in OS and every run-time error | 25 | 25 |
| **Total** | **52** | **52** |

In all: 85,904 VM steps, 1,028 RAM cells and 114,688 screen words.

The captured Java results of the probe programs are also kept as tests of the `n2t-vm`
crate (`crates/vm/tests/java_probes`), so they run in CI without Java. Captured Java results for programs
with their own `Memory.init`, `Memory.alloc` and other OS `init` functions beside the
built-in OS are in `crates/vm/tests/user_os_classes.rs`: the order and VM steps of the
built-in `Sys.init`, and the built-in `Array` and `String` functions using the program's
own `Memory.alloc` and `Memory.deAlloc`, as the Java emulator does. A seeded fuzzer
(generated programs; the count is set with `N2T_VM_FUZZ_ITERS`, and the crate's reviewer
ran 20,000) and a sweep of every built-in function with hostile arguments check that
nothing a program does can make the emulator panic; they compare nothing with Java.

## 6. The extension itself

- `cargo test --workspace`: the engine's own tests (no Java or course files needed), run in
  CI natively on Linux, Windows and macOS.
- `npm run test:unit`: the WebAssembly engine through the extension's loader, against real
  files, on all three platforms.
- `npm run test:integration -- --vsix <file>`: the packaged VSIX installed into a real
  VS Code and driven through its API: editing features, the Testing view, the chip
  simulator, the CPU and VM emulator panel, Jack compilation, reference comparison and the
  script debugger (including pausing a step that never ends). CI builds one VSIX on Linux
  and runs this suite against that same file on Linux, Windows and macOS.

## What has not been checked

- Nothing has been run by hand on a physical Windows or macOS machine; those platforms are
  covered by CI only.
- CI installs the current stable VS Code. The declared minimum, 1.91, is not tested.
- The newer project files of the web IDE and the book's second edition have not been run
  through the harness.
- Error *messages* are not compared with Java's, only whether there is an error and where.
- VM emulator: keyboard lines longer than 14,332 characters, the exact length of `Sys.wait`
  in real time, key codes the comparison did not press, and programs that corrupt their
  own call frames or the heap's bookkeeping have not been compared with Java (the fuzzer
  only checks that they are handled safely).
- The "already exists with different content" question of Assemble and Compile is not
  clicked in the integration tests; the rule behind it (a file is replaced only as it was
  when you were asked) is tested through the engine.
