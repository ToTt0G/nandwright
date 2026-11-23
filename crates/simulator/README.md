# n2t-simulator

The hardware simulator and the test-script runner.

- `Circuit` instantiates a compiled chip and evaluates it with the two-phase `tick`/`tock` clock.
- `primitives` implements the built-in chips natively.
- `ScriptRunner` runs a `.tst` script of any dialect against a `Machine`
  (`HardwareMachine`, `CpuMachine`, `VmMachine`), writes the `.out` file and compares it with
  the `.cmp` file. It is resumable: it can run for a budget of commands, step to the next
  `;`, or step one command, which is what the editor's test runs and debugger are built on.

Its results follow the official Java tools; see `docs/DEVIATIONS.md` for the exceptions.
