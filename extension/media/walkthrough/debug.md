## When a test fails

The failure shows the compare file and your chip's output side by side, with
every row that differs, and for each of those rows the value of **every pin,
internal ones included**, at that step.

Then find out why:

- **Open in simulator** shows the chip beside its code. Flip the inputs and
  watch the outputs and internal pins. Small chips get a full truth table.
  The panel follows your edits as you type.
- **Step through** runs the test script one step at a time with the
  debugger: Step Over runs to the next `;`, the Variables view shows the pins,
  and you can set breakpoints in the `.tst` file.

For a clocked chip the simulator has **Tick** and **Tock** buttons, so you
can watch a register take its value one clock phase at a time.
