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
   covering all eight built-in OS classes and 19 error cases (18 numbered
   library errors and one explicit `Sys.error` call), comparing RAM,
   screen output, and error/halt status.

## Usage

```sh
N2T_COURSE_DIR=~/n2t-private/course python3 conformance/vm/diff.py
```

When `N2T_COURSE_DIR` is unset or does not exist (as in CI), the script skips
cleanly and exits `0`. Any mismatch exits non-zero.

## Java golden probes

`cargo test -p n2t-vm --test java_probes` replays original VM programs against
committed Java RAM and screen captures. Java and the course tree are only
needed to refresh captures.

```sh
python3 conformance/vm/capture_java_probes.py [probe-name ...]
```

The capture script uses the private Java tool copy under
`~/n2t-private/tmp/vm`, a Java 17 compiler on PATH, and a temporary Xvfb
display. `KeyDrive.java` injects press and release events through Swing.
Probes with `observe` record the rendered screen before input, with each
key held, and after its release. `codes` supplies the Hack codes for
replay when Java's GUI translates an event, such as lowercase to uppercase.
