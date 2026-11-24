## When a test fails

The failure shows the compare file and your chip's output side by side, with
every row that differs, and for each of those rows the value of **every pin,
internal ones included**, at that step.

Then find out why:

- **Open in simulator** shows the chip beside its code. Flip the inputs and
  watch the outputs and internal pins. Small chips get a full truth table.
  The panel follows your edits as you type. It follows you to whichever chip you open next.
- **Show Circuit Diagram** draws the chip as a live schematic beside its code:
  flip inputs, watch signals travel through the wires, double-click your own
  chips to look inside them, and click a part or wire to find it in the HDL.
- **Step through** runs the test script one step at a time with the
  debugger: Step Over runs to the next `;`, the Variables view shows the pins,
  and you can set breakpoints in the `.tst` file.

For a clocked chip both the simulator and the circuit diagram have clock
controls (**Tick** and **Tock**), so you can watch a register take its value
one clock phase at a time.
