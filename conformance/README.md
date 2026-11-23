# Nandwright Conformance Harness

Compares the native `n2t` CLI against the official Nand2Tetris Java tools
(`HardwareSimulator.sh`, `CPUEmulator.sh`, `VMEmulator.sh`) on identical staged
workspaces.

No course files, skeletons, official tests, or solutions live in this directory.
All course inputs, private solutions, staged workspaces, cached Java ground
truth, and generated reports stay outside the repository under
`N2T_COURSE_DIR` and `N2T_PRIVATE_DIR`.

## Requirements

- Python 3.12+ (standard library only; no third-party packages)
- Java 17+ (only needed when capturing or refreshing Java ground truth)
- Built `n2t` binary (`cargo build --release -p n2t-cli -j 3`)

## Environment variables

| Variable | Default | Purpose |
|---|---|---|
| `N2T_COURSE_DIR` | `~/n2t-private/course` | Official course distribution (`projects/` and `tools/`). If absent, `run.py` prints `skipped: N2T_COURSE_DIR not found` and exits `0`. |
| `N2T_PRIVATE_DIR` | `~/n2t-private` | Holds `solutions/`, `evidence/exp2/`, `stage/`, `truth/`, and `reports/`. |
| `N2T_BIN` | `target/release/n2t` | Optional override for the `n2t` CLI path. |

## Usage

```bash
# Run all scenarios (captures Java truth on first run, then uses cache)
python3 conformance/run.py

# Run one scenario
python3 conformance/run.py --scenario solved
python3 conformance/run.py --scenario untouched
python3 conformance/run.py --scenario broken
python3 conformance/run.py --scenario dialects

# Filter by project or file
python3 conformance/run.py --scenario solved --project 05
python3 conformance/run.py --scenario broken --file c05_subbus_internal

# Re-run Java tools and refresh the cached truth for selected cases
python3 conformance/run.py --scenario solved --refresh-truth

# Capture Java truth only (without running n2t)
python3 conformance/run.py --capture-only
```

Parallelism is capped at 3 worker processes (`--jobs`, default 3, max 3).

## Scenarios

- **`solved`**: Course tree overlaid with private solutions (`01`, `02`, `03/a`,
  `03/b`, `04`, `05`, `demo`), with Project 04 `.asm` files assembled via
  `Assembler.sh`.
- **`untouched`**: Course tree exactly as shipped (unimplemented skeletons).
- **`broken`**: The 23 `evidence/exp2` HDL/TST cases plus a deterministic,
  seeded single-mutation corpus (default 180 mutations across 9 mutation
  families) generated at runtime from private solutions.
- **`dialects`**: CPU-emulator and VM-emulator `.tst` scripts in projects `04`,
  `07`, `08`, and `12` runnable from shipped course files (including `*VME.tst`
  and Project 12 tests staged with shipped `tools/OS/*.vm` + `JackCompiler.sh`).
  Scripts that require student-produced artifacts (such as a VM translator's
  `.asm`) are recorded as skipped.

## Comparison rules

For each selected script, `n2t` runs on a fresh isolated copy of the staged
directory and is compared against the cached Java truth record:

1. **Outcome class** must match (`passed`, `unchecked`, `comparison-failure`,
   `error`, `timeout`).
2. **Passed scripts**: `compared_lines` must match when Java's is derivable.
3. **Comparison failures**: failing line number must match.
4. **Output files (`.out`)**: byte-identical to Java's after normalizing line
   endings (`CRLF`/`CR` -> `LF`) only.
5. **Errors**: script line number must match when Java reports one (`In script
   ..., Line N`); messages may differ and are recorded side by side.
6. **Unsupported dialects**: CPU and VM scripts are reported as `not-supported`
   without failing the run while `n2t` only implements the hardware dialect;
   they automatically activate as the engine adds those dialects.

Any unexplained difference causes `run.py` to exit with status `1`. Deliberate,
documented deviations can be listed in `conformance/expected-differences.toml`.

## Adapter contract (`conformance/n2t_adapter.py`)

All invocations of `n2t` go through `run_n2t(...)`. It checks once whether
`n2t test --json` is available:

- **JSON mode (preferred)**: parses one JSON object per script from stdout
  (`script`, `dialect`, `outcome`, `compared_lines`, `mismatch`, `error`,
  `out_file`).
- **Human mode (fallback)**: parses human-readable output and exit code from
  `n2t test <script.tst>`.

## The other comparisons

`run.py` covers test scripts of every dialect. Three more scripts compare one tool each
with its Java counterpart; they take the same `N2T_COURSE_DIR` and skip in the same way
when it is missing:

| Script | Compares |
|---|---|
| `hack/run.py` | the assembler (`.hack` bytes, error lines) and the CPU emulator (state after every instruction) |
| `jack/run.py` | the Jack compiler (`.vm` bytes, accept or reject, error lines) and the project 10 XML |
| `vm/diff.py` | the VM emulator and its built-in OS (memory and all screen words); it drives the Java VM Emulator's window under Xvfb, because only there does Java offer its built-in OS |

And one check needs no Java: the course tree as shipped must have no false errors.

```sh
target/release/n2t check "$N2T_COURSE_DIR/projects"    # expect: 0 errors, 0 warnings
```

`docs/VERIFICATION.md` has the numbers from the last full run.
