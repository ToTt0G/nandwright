# VM Emulator Differential Conformance Harness

Compares `n2t-vm` (via `crates/vm/examples/vmrun.rs`) against the official Java
VM Emulator across:

1. **Projects 07 & 08** (`--only p0708`): every official `*VME.tst` test under
   `$N2T_COURSE_DIR/projects/{07,08}`, reproducing each script's `set`s,
   `vmstep`s, and compared RAM cells row by row.
2. **Projects 09, 11 & 12** (`--only jack`): Jack programs compiled with the
   Java `JackCompiler` into a temporary scratch directory (without OS `.vm`
   files, so both emulators run their built-in Jack OS), comparing RAM samples
   and all 8192 screen words (with scripted `RAM[24576]` keyboard events for
   interactive programs like `Square`, `Pong`, and `SysTest`).
3. **Built-in OS & Error Probes** (`--only probes`): original VM programs
   covering every built-in OS class (`Math`, `Memory`, `Array`, `String`,
   `Screen`, `Output`, `Keyboard`, `Sys`) and all 19 OS error conditions
   (`ERR1`–`ERR20`), comparing RAM, screen output, and error/halt status.

## Usage

```sh
N2T_COURSE_DIR=~/n2t-private/course python3 conformance/vm/diff.py
```

When `N2T_COURSE_DIR` is unset or does not exist (as in CI), the script skips
cleanly and exits `0`. Any mismatch exits non-zero.
