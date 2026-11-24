# Changelog

## 0.3.2

- Zip Project Files: a new setting, `nandwright.zip.fileName`, sets the name the zip gets.
  `{folder}` is the folder's name, `{n}` the number in it and `{nn}` that number with two
  digits. `Project{nn}-familyname-firstname` gives `Project01-familyname-firstname.zip` for
  a folder named `01`. The gear button in the file list opens the setting.
- Zip Project Files: the `+` button in the file list adds files from another folder, such as
  a grade sheet. They go at the top level of the zip.
- Zip Project Files no longer offers Office lock files (`~$Project1.xlsx`, there while the
  sheet is open in Excel), `Thumbs.db`, `desktop.ini` or the `__MACOSX` folder.

## 0.3.1

- Test results are never shown for an older version of your work: results from overlapping
  runs, from edits made while tests were running, and from before a reload are now
  discarded instead of displayed, and row marks in the simulator always line up with the
  compare file the script uses.

## 0.3.0

- Tests run when you save: saving a chip, a program, a test script or a compare file runs
  the test scripts in its folder that use it, quietly, in the Testing view. The setting
  `nandwright.test.runOnSave` turns this off.
- The chip simulator shows the last test run: each row of the expected output is marked
  as matching, differing (with what your chip gave) or not reached, and a Run test button
  runs the chip's test from the panel.
- Project progress in the status bar: how many of the folder's tests pass, with a list of
  what is passing, failing, not run or not started.
- Clock history in the chip simulator: the pins after each tick and tock.
- Zip Project Files: packs a project folder for handing in, with your own work ticked and
  generated files left out. You choose the final list.
- Above a chip, beside Run test, the editor shows the result of the last run: whether the
  test passes, or how many rows differ. No need to open the Test Results view.

## 0.2.0

- The chip simulator follows the editor: switch to another `.hdl` file and the simulator
  shows that chip, without closing and reopening it. The setting
  `nandwright.simulator.followEditor` turns this off.
- The chip simulator shows the expected output: the chip's compare file (`.cmp`) as a
  table, with the row for the current inputs marked and any output that differs from
  your chip pointed out. Click a row to set the inputs to it.
- A test that waits for a key press on a chip that can never see one (the project 5
  keyboard test on a chip you have not written yet) now stops at once with an
  explanation, instead of running to the command limit.

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
