# Changelog

## 0.1.1

- The chip simulator follows the editor: switch to another `.hdl` file and the simulator
  shows that chip, without closing and reopening it. The setting
  `nandwright.simulator.followEditor` turns this off.

## 0.1.0

The first version.

- Hardware simulator for HDL chips, with every built-in chip of the course.
- Test scripts (`.tst`) of all three kinds run in the Testing view: Hardware Simulator,
  CPU Emulator and VM Emulator scripts. Failures show every differing row with the pin
  values at that step.
- Interactive chip simulator panel with a clock for sequential chips and truth tables for
  small chips.
- A debugger that steps through test scripts.
- Hack assembler and CPU emulator, VM emulator with a built-in Jack OS, and Jack compiler,
  with a panel for the screen, keyboard, registers, stack and memory, and breakpoints in
  assembly, VM and Jack code.
- Reference comparison for the output of your own assembler, syntax analyzer and compiler.
- Language features for HDL, test scripts, Hack assembly, VM code and Jack; a formatter
  for HDL and test scripts that changes whitespace only.
