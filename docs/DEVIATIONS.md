# Where Nandwright differs from the official tools

Nandwright aims to give the result the official Java tools give, because that is
what coursework is graded against. The conformance harness (`conformance/`) runs
the same inputs through both and fails on any difference that is not listed in
`conformance/expected-differences.toml`.

This page lists every place where Nandwright is known to behave differently, on
purpose. Each entry says what the Java tool does, what Nandwright does, and why.
Anything not listed here that differs is a bug: please report it.

Ground truth is the Java suite of the classic course distribution
(`nand2tetris/tools`, the version with `HardwareSimulator.sh` and friends).
Where that suite and the book or the web IDE disagree, the Java suite wins.

## Test scripts (all three dialects)

| | Java tools | Nandwright | Why |
|---|---|---|---|
| Which program runs a script | You open the script in the Hardware Simulator, the CPU Emulator or the VM Emulator. | The dialect is read from the script: what its first `load` names, or failing that the stepping command it uses (`eval`/`tick`/`tock`, `ticktock`, `vmstep`). | An editor should not make you pick. A script that mixes dialects is reported as an error. |
| The script produces more lines than the compare file has | The simulator crashes with a `NullPointerException`. | A comparison failure at the first extra line, saying the compare file ended. | A crash is not a result. |
| A script that waits for a key (`while out <> 75 { eval, }` in the project 5 `Memory.tst`) | Waits until a person holds the key down in the simulator window. | Presses the key the loop is waiting for, holds it until the next `output`, and says so in the test output. This happens only when the loop watches the keyboard: an output pin of a chip that contains a keyboard, or `RAM[24576]`. A loop that waits for anything else (`while A <> 75 { ticktock; }`) gets no key. Setting `nandwright.test.pressKeys` (or `n2t test --no-keys`) turns it off altogether; the run then waits at the same line as Java. If nothing in the loop can change the awaited value and no key is pressed for it, the Testing view and `n2t test` stop there and say so; Java waits until you stop it. | So that "run all tests" can finish. The 54 lines produced this way equal `Memory.cmp`. |
| A script with a `repeat` that has no count (`Fill.tst`) | Runs until you stop it. | In the Testing view it is skipped with an explanation; "Step Through the Script" runs it. `n2t test` stops it at the command limit. | It is a demonstration to watch, not a test with a result. |
| After the first comparison failure | Stops. | Reports that first failure as the result, and stops writing the `.out` file there, exactly as Java does; the editor then keeps executing the script in memory to show every other failing row too. | The grade is the same; the extra rows help find the bug. |
| Line number in a script error | Often the line after the mistake (the tokenizer has already moved on). | The line and column of the mistake. | |
| A compare file with lines the script never reaches, or a `compare-to` with no `output` at all | "Comparison ended successfully". | The same: the test passes. The test output adds a note saying how many lines were not reached. | |

## Hardware simulator

| | Java tools | Nandwright | Why |
|---|---|---|---|
| Things Java accepts without a word: an input pin that is never used, a part input left unconnected, an internal pin that nothing reads | Silent. The pin is 0. | The same simulation, plus a warning in the editor. | These are the usual cause of a failing test. |
| A chip whose `PARTS:` is empty | Loads; outputs are 0; the test fails on comparison. | The same, plus a hint (not a warning, not an error) on the chip. | It is where every project starts. |
| File names that differ only in case (`and.hdl` for `And`) | Whatever the file system does: found on Windows, not found on Linux (the built-in chip is used). | The same, because the lookup goes through the same file system. | |

Agreement with the Java Hardware Simulator on the classic course tree, as
measured by the harness: every test of projects 1, 2, 3 and 5 on solved chips,
every test on the untouched skeletons, 23 hand-written broken cases and 180
generated single-mutation chips. Outcome class, failing line and the bytes of
the `.out` file (line endings aside) are compared.

## Assembler and CPU emulator

| | Java tools | Nandwright | Why |
|---|---|---|---|
| A `.hack` file with a line of 15 or 17 digits, or a blank line in the middle | Loaded; the blank line is read as 0. | Rejected, naming the line. | Such a file is damaged; running it hides the damage. |
| A program of exactly 32768 instructions | The assembler crashes and leaves a truncated `.hack`. | An error: a program can have at most 32767 instructions, which is the most Java assembles. | |
| Exit status and missing output | The assembler exits with status 0 even on an error, and writes nothing if any folder in the path contains a `.` | Errors are errors; paths are paths. | |
| Surprising assembly that Java accepts (`@-1`, a label defined twice, `D<tab>JGT` losing its jump, …) | Accepted silently. | Accepted with the same machine code, plus a warning. | Same output, but you are told. |
| A test loads `Prog.hack` while `Prog.asm` next to it assembles to something else | Runs the `.hack`. | Runs the `.hack` too, and adds a note to the test output. | The usual cause is forgetting to assemble again. |
| State after a run-time error (an address out of range) | Cannot be observed: the script stops. | The failing instruction changes nothing. | |

## VM emulator

| | Java tools | Nandwright | Why |
|---|---|---|---|
| A program that calls OS functions without containing the OS | The GUI asks whether to use its built-in OS; run from the command line it refuses to start without `Sys.vm`. | Uses the built-in OS, as the GUI does when you answer yes. | |
| `Sys.wait` in a test script | Really waits. | The script's clock jumps to the end of the wait. | A test should not sleep. In the interactive panel the wait is real. |
| An OS error in a test script (for example a heap overflow in the built-in `Memory.alloc`) | The GUI shows "Program Halted", then the script runs on to its end and writes its output. | The test fails at that step with the error. | The screen shows the same `ERR` code; a test should not pass after the program stopped. |
| Built-in OS functions that use other OS functions, when the folder has its own version of those | The built-in `Output.printString`, `Keyboard.readLine`, `Keyboard.readInt` and `Sys.error` call your own `String`, `Output` or `Memory` functions if the folder has them. | Only `Array.new`, `Array.dispose`, `String.new` and `String.dispose` do: they call your own `Memory.alloc` and `Memory.deAlloc`, as Java does. The others use the built-in versions. | Covers the usual case, your own `Memory.vm` with the rest of the OS built in. |
| `RAM[256]`, the first word of the built-in `Sys.init`'s frame | The program's length plus a few words. | 10. | No program reads it. |

## Jack compiler

| | Java tools | Nandwright | Why |
|---|---|---|---|
| Expressions nested more than 100 deep | Prints nothing and writes a `.vm` file that stops after the `function` line. | An error. | Silent wrong output. |
| Expressions, unary operators and blocks nested more than 128 deep in total | Accepts deeper nesting of some kinds. | An error. | The compiler runs in WebAssembly with a fixed stack; a limit is better than a crash. |
| Errors in calls between classes (unknown subroutine, wrong argument count, method called as function) | Printed as errors, but the `.vm` file is written anyway. | "Compile Jack to VM Files" writes nothing if there is any error. "Compare with Reference Output" still shows the code Java would have written. | An error that produces a file invites running broken code. |
| The same class name in two files of a folder | "Subroutine … redeclared". | "Duplicate class". | |
| Surprising Jack that Java accepts (`1.5` read as `1`, a string that never closes ending at the end of the line, a field used in a function becoming a call on a class of that name, …) | Accepted silently. | Accepted with the same code, plus a warning. | |

In the editor (not in the compiler), two things the compiler rejects are shown
as hints instead of errors, because they are how the course ships its files:
a subroutine whose body is still empty (the project 12 OS skeletons), and
everything in the project 10 `ExpressionLessSquare` folder, whose code is
deliberately meaningless.

## Not differences, but worth knowing

- **Output files have your platform's line endings**, as with the Java tools
  (CRLF on Windows, LF elsewhere).
- **Running Jack does not write `.vm` files.** The official flow is to compile
  to `.vm` files and then load the folder in the VM Emulator. Nandwright
  compiles in memory when you run a `.jack` file, because in projects 10 and 11
  the `.vm` files in the folder are your own compiler's output. "Compile Jack to
  VM Files" writes them when you ask, and asks before replacing a file whose
  content differs.
- **The formatter** has no counterpart in the official tools. It changes only
  whitespace (indentation, spacing, runs of blank lines), never joins two
  lines or splits one, and checks that the formatted text has the same tokens
  before it touches the file.
