# Changelog

## 0.5.2

- Saving the open `.asm` file reloads it in the Hack computer while the machine is at reset.
  Saving preserves emulator state once you run, step, or change it manually.

## 0.5.1

- The Hack computer and VM emulator now follow the active `.asm`, `.hack`, `.vm` or `.jack`
  file in the same panel. Switching files stops the previous program and loads the new
  one paused. Turn off `nandwright.simulator.followEditor` to pin panels to individual files.
- Switching to the HDL tab of a chip you drilled into now opens that chip as the root of
  the circuit diagram. Clicking a part or wire in the diagram still preserves the drill path.

## 0.5.0

- **Show Circuit Diagram** draws a chip as a live schematic beside its code: gate shapes,
  wires coloured by their value, bus values in decimal, binary or hex. Click inputs to
  change them and watch the change travel through the wires (setting
  `nandwright.circuit.animate`). Double-click a part that is one of your own chips to see
  inside it. Clicking a part or a wire selects its part statements in the HDL, and the
  editor cursor marks the part it is on. Export as SVG or PNG. From the editor title, the
  editor and Explorer context menus and the Command Palette.
- The chip simulator and the circuit diagram of the same chip share one simulation.
- A new practice chip, `Veto`, built from `Majority`.

## 0.4.0

- **Nand2Tetris: Set Up…** asks for your course, your name and your grade sheets, and
  whether tests run when you save. Every step can be skipped. Nandwright offers it once,
  as an ordinary notification, when you install or update.
- Course presets: the setting `nandwright.course`. The default, no preset, turns nothing
  course-specific on. **KTH EP121U** puts each project's folders together, names the zip
  as the course asks, ticks only the files the course asks for and adds the grade sheet.
  A setting you set yourself wins over the preset.
- Zip Project for Hand-In (the new name of Zip Project Files):
  - `nandwright.zip.projects` lists the folders of each project by the course's numbers.
    A project of several folders gives one zip with their files side by side, and `{n}`
    and `{nn}` in the zip's name come from the course's number. Zipping a folder inside a
    listed folder zips the listed folder.
  - `{familyname}` and `{firstname}` in the zip's name, from the new settings
    `nandwright.student.familyName` and `nandwright.student.firstName` (user settings
    only). If one is needed and not set, you are asked when you zip.
  - The project's grade sheet (`Project1.xlsx`, `project_01.ods`, …) is found and added:
    in the project's folder, the folder in `nandwright.zip.gradeSheetFolder`, a folder
    named `grade-sheets`, or the folders above. It is ticked, shows when it last changed,
    and has a button that opens it. A `grade-sheets` folder is never zipped as part of a
    project.
  - Easier to find: **Hand In** in the status bar, an entry in the project progress list,
    the editor's `…` menu for `.hdl`, `.asm`, `.vm` and `.jack` files, and an offer to zip
    when the last failing test of a folder passes.
- The Get Started walkthrough has a step for handing in.

## 0.3.3

- VM emulator: with your own OS classes and no `Sys.vm`, the built-in `Sys.init` now runs
  your `Memory.init`, `Math.init`, `Screen.init`, `Output.init` and `Keyboard.init` before
  `Main.main`, in that order, as the official VM Emulator does. Before, your `Memory.init`
  was skipped, so a program using your own `Memory.vm` (project 12) could behave
  differently from the official tool.
- VM emulator: the built-in `Array.new`, `Array.dispose`, `String.new` and `String.dispose`
  now call your own `Memory.alloc` and `Memory.deAlloc` when the folder has them, as the
  official VM Emulator does. Before, they used the built-in heap alongside yours.
- Compare with Reference Output now ignores spaces and tabs inside a line, as the official
  TextComparer does. XML written without indentation now matches.
- Zip Project Files: a VM translator's output (`Prog.asm` next to `Prog.vm`, or `Dir/Dir.asm`
  in a folder of `.vm` files) is not ticked, like other generated files.

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
