# Nandwright: Nand2Tetris IDE for VS Code

Nandwright is a VS Code extension for doing the whole [Nand2Tetris](https://www.nand2tetris.org/)
course ("Nand to Tetris", *The Elements of Computing Systems*) inside the editor. It runs
your HDL chips in a hardware simulator, runs every `.tst` test script, assembles and runs
Hack assembly on a CPU emulator, runs VM code and Jack programs on a VM emulator with the
Hack screen and keyboard, and shows you why a test failed. It needs no Java and no
`tools` folder: the simulators are built into the extension.

It is for students whose course uses the classic project folders (`projects/01` …
`projects/12`) and who would rather stay in VS Code than switch between the editor and the
official Java programs. Results are checked against those Java programs.

![Writing a chip, running its test, reading the failure, fixing it and passing, all in VS Code](docs/images/demo.gif)

> Unofficial. Nandwright is not affiliated with or endorsed by the authors of Nand to
> Tetris. It contains no course material and no solutions.

## Install

1. Download `nandwright-<version>.vsix` from the [Releases](https://github.com/ToTt0G/nandwright/releases) page.
2. In VS Code open the Extensions view, choose **…** then **Install from VSIX…**, and pick the file.
   (Or run `code --install-extension nandwright-<version>.vsix`.)

That is all. It works on Windows, macOS and Linux with VS Code 1.91 or newer. There is one
file to install and it is the same file on every platform.

## Quick start

1. **Open your course folder**: the one that contains `projects`. Nothing needs to be configured.
2. **Open a chip**, for example `projects/01/Not.hdl`, and write its `PARTS:` section.
   Completion offers chips and their pins; mistakes are underlined as you type.
3. **Press the play button** at the top right of the editor (or `Ctrl+Alt+T`, `Cmd+Alt+T` on a Mac).
   The chip's test script runs and the result appears: passed, or the first line that
   differs from the compare file.
4. **If it fails**, the failing row shows expected against actual, which columns differ,
   and the value of every pin, internal ones included, at that step.

No course folder at hand? Run **Nand2Tetris: Get Started** from the Command Palette and
choose "Try the example chip": a small practice chip (not a course project) to write and test.
The [`examples`](examples) folder of this repository has more: chips, assembly programs and a Jack program.

## What you can do

### Run and test

- **Every `.tst` script is a test** in VS Code's Testing view: run one, a folder, or the
  whole course. Hardware Simulator, CPU Emulator and VM Emulator scripts all work; the
  extension reads from the script which kind it is.
- The `.out` file is written next to the script, as the official tools do.
- Scripts that wait for a key (project 5's `Memory.tst`) get the key pressed for them, and
  the output says so.
- Tests run when you save a file, quietly, in the Testing view.

![The Testing view listing test scripts with their results](docs/images/testing-view.png)

### See why a test failed

- The failing row with **expected and actual**, the **columns that differ**, and **every pin
  at that step**, shown at the script line that produced it. All failing rows are listed,
  not only the first.
- **Step through the test script** with the debugger: breakpoints in the `.tst` file, Step
  Over to the next `;`, pins in the Variables view.

![A failing test: expected and actual rows, the differing column, and the internal pins](docs/images/failing-test.png)

![Stepping through a test script with the debugger, pins in the Variables view](docs/images/step-through.png)

### Try a chip by hand

- **Open in simulator** shows a chip next to its code: flip inputs, type bus values in
  decimal, binary or hex, and watch outputs and internal pins. Small chips get a full
  truth table.
- Clocked chips have **Tick**, **Tock** and the clock time, so you can watch a register load.
- The panel follows the editor as you type. It follows you from chip to chip as you switch files. Below the pins it shows the expected output from the chip's `.cmp` file, and marks where your chip disagrees.
- The chip simulator marks each row of the expected output as matching, differing or not reached after a test run, and has a Run test button.

![The chip simulator beside the HDL source, with inputs, outputs, internal pins and a truth table](docs/images/chip-simulator.png)

![A clocked chip in the simulator with Tick, Tock and the clock time](docs/images/clocked-chip.png)

### Run programs: assembly, VM code, Jack

- **Run Program** runs a `.asm` or `.hack` file on the Hack computer, and a `.vm` file or a
  Jack program on the VM emulator, with the **screen**, the **keyboard**, registers, the
  stack, the call stack and memory. Run, pause, step, choose the speed.
- **Breakpoints** set in the editor pause the program; the line about to run is highlighted.
  Jack programs step by Jack line.
- Jack runs with a built-in OS (`Math`, `String`, `Array`, `Output`, `Screen`, `Keyboard`,
  `Memory`, `Sys`). Classes you write yourself, including OS classes for project 12,
  replace the built-in ones.

![A Hack assembly program on the CPU emulator: screen, registers and memory](docs/images/hack-computer.png)

![A Jack program running on the VM emulator with the Hack screen, call stack and registers](docs/images/jack-program.png)

### Check your own assembler, translator and compiler

In projects 6 to 11 you write the tools yourself. Nandwright does not overwrite their output.

- **Compare with Reference Output** on a `.hack`, `.vm` or `.xml` file your tool wrote shows
  a diff against what the built-in assembler, compiler or syntax analyzer produces.
- Your VM translator's `.asm` output runs under the course's CPU Emulator test scripts in
  the Testing view.
- **Assemble to .hack File** and **Compile Jack to VM Files** write the official output when
  you ask, and ask before replacing a file whose content differs.

### Write code

- HDL: completion, hover, go to definition (also into the built-in chips), find references,
  rename, signature help in part lists, quick fixes for misspelled chips and pins,
  semantic highlighting.
- Hack assembly: hover shows the machine code and address of an instruction; go to
  definition, references and rename for labels and variables.
- Jack: errors as the official compiler reports them (checked across the classes of a
  folder), hover with types and VM segments, completion including the OS API, rename.
- Test scripts and VM files: highlighting and diagnostics; `.out` files show which cells
  differ from the `.cmp` file.
- **Format Document** for HDL and test scripts changes whitespace only: indentation, spacing
  and runs of blank lines. It never joins two lines or splits one, and it checks its own
  output before touching your file.
- A chip or subroutine that is not written yet is not an error.

![Diagnostics and hover while editing HDL](docs/images/editing.png)

The panels use your colour theme, including light and high-contrast themes.

![The chip simulator in a light theme](docs/images/chip-simulator-light.png)

## Course projects

| Project | You write | What Nandwright does for it |
|---|---|---|
| 1 Boolean logic, 2 Arithmetic, 3 Memory, 5 Computer | Chips in HDL | Simulates them, runs the test scripts, shows failing rows with internal pins, interactive simulator with clock |
| 4 Machine language | Hack assembly | Runs the CPU Emulator test scripts; runs programs with screen and keyboard; assembles to `.hack` |
| 6 Assembler | An assembler | Compares your `.hack` with the reference; runs it on the Hack computer |
| 7, 8 VM translator | A VM-to-Hack translator | Runs the course's CPU Emulator scripts on your `.asm`; runs the `…VME.tst` scripts and any `.vm` program on the VM emulator |
| 9 High-level language | Jack programs | Compiles and runs them with the built-in OS; breakpoints and stepping by Jack line |
| 10 Syntax analyzer | A tokenizer and parser | Compares your `.xml` with the reference token list and parse tree |
| 11 Compiler | A Jack compiler | Compares your `.vm` with the reference; runs your `.vm` on the VM emulator |
| 12 Operating system | The OS classes in Jack | Runs the project's test scripts and test programs with your classes in place of the built-in ones |

## Does it match the official tools?

That is the point of the project, so it is measured. A conformance harness runs the same
inputs through the official Java tools and through Nandwright's engine and compares the
outcome, the failing line and the bytes of the output file. On the classic course
distribution:

- **Test scripts**: 301 cases (every hardware test of projects 1, 2, 3 and 5 on solved
  chips and on the untouched skeletons, 23 hand-written broken cases, 180 generated broken
  chips, the CPU Emulator tests of project 4 and the VM Emulator tests of projects 7, 8 and
  12). 299 give the same result as Java. The other two differ on purpose.
- **Assembler and CPU emulator**: identical `.hack` files, and identical registers and
  memory after every instruction, over the course's programs and generated ones.
- **Jack compiler**: byte-identical `.vm` output on every Jack file of the course and on
  generated programs, and the same accept-or-reject decision with the error on the same line.
- **VM emulator and built-in OS**: memory and all 8,192 screen words compared with Java on
  the course's test programs and on probes of each OS function.

Every deliberate difference is listed, with the reason, in [docs/DEVIATIONS.md](docs/DEVIATIONS.md).
The exact numbers and how to reproduce them are in [docs/VERIFICATION.md](docs/VERIFICATION.md).

## How it compares

| | Needs Java and the `tools` folder | Runs tests in the editor | Failing row with internal pins | CPU and VM emulators with screen | Works on plain course folders |
|---|---|---|---|---|---|
| **Nandwright** | No | Yes | Yes | Yes | Yes |
| Official Java tools | Yes (they are the tools) | Separate windows | Compare by eye | Yes | Yes |
| [Official web IDE](https://nand2tetris.github.io/web-ide) | No | In the browser | Diff of the output table | Yes | It keeps its own copy of the projects |
| VS Code extensions that launch the Java tools (for example Nand2Tetris Tools, NAND IDE, Mastro NANDo) | Yes | Result in a terminal | No | In the Java windows | Yes |
| VS Code extensions for highlighting and snippets | Not applicable | No | No | No | Yes |

The official tools and the official web IDE are the reference; if your course tells you to
use one of them for grading, do that for your final check. The descriptions of other
extensions are from their own Marketplace pages at the time of writing.

## Questions

### How do I run a Nand2Tetris .tst file in VS Code?

Install Nandwright, open the `.tst` file (or the `.hdl` it tests) and press the play button
at the top right of the editor, or run it from the Testing view. No setup is needed.

### Do I need Java?

No. Nandwright does not use the official Java programs. Its simulators, assembler,
emulators and Jack compiler are compiled to WebAssembly and shipped inside the extension.

### Does it work with the course's project folders?

Yes. Open the folder you unzipped from the course (the one with `projects` in it). The
untouched course files open without errors: an unwritten chip is shown as a hint, not a problem.

### Will my chip get the same result as in the official Hardware Simulator?

It has on every case measured so far: see "Does it match the official tools?". If you find
a case where it does not, that is a bug; please open an issue with the chip's interface and
the test (not your solution).

### How do I see the internal pins of an HDL chip?

Three ways: a failing test lists them for each failing row; "Open in simulator" shows them
live as you change inputs; "Step through the script" shows them in the debugger's Variables view.

### How do I run a Hack assembly or Jack program and see the screen?

Open the `.asm`, `.hack`, `.vm` or `.jack` file and choose **Run Program** from the play
button's menu. Click the screen to send keys to the Hack keyboard.

### I am writing my own assembler, VM translator or compiler. How do I check it?

Open the file your tool produced (`Prog.hack`, `Main.vm`, `MainT.xml`, `Main.xml`) and run
**Nand2Tetris: Compare with Reference Output**. For the VM translator, run the course's
test scripts on the `.asm` your translator wrote.

### Why does the output say a key was pressed for me?

Some official scripts wait until you hold a key down in the simulator window. Nandwright
presses the key the script is waiting for so that the test can finish. Turn off the setting
**Nandwright › Test: Press Keys** to do it yourself.

### Does it work in the browser (vscode.dev) or with the web IDE's files?

It needs desktop VS Code. It is tested with the classic course distribution; the newer
project files use the same languages, but they have not been run through the conformance
harness.

### Does it include solutions to the projects?

No, and it will not. The repository's tests use chips and programs that are not course
assignments, and a check in CI enforces that.

## The command line

The engine also builds as a native command, `n2t`, used by the test suite:

```sh
cargo build --release -p n2t-cli
n2t test projects/01            # run every test script in a folder
n2t check projects              # report problems in .hdl, .tst, .asm, .vm and .jack files
n2t eval Majority.hdl a=1 c=1   # evaluate a chip and print every pin
n2t fmt --check projects/01     # check formatting
```

## Building and testing

You need Rust (with the `wasm32-unknown-unknown` target) and Node 20 or newer.

```sh
cargo test --workspace                 # the engine
cd extension && npm ci
npm run build:wasm && npm run build    # the extension
npm run test:unit                      # the engine through its WebAssembly loader
xvfb-run -a npm run test:integration   # the extension in a real VS Code (plain `npm run …` on Windows and macOS)
npm run package                        # dist/nandwright-<version>.vsix
```

On Windows, run them from a short path such as `C:\src\nandwright`: the copy of VS Code that the tests download lives inside the checkout, and it stops working when its path grows beyond 260 characters.

[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) explains how the pieces fit;
[conformance/README.md](conformance/README.md) explains how to run the comparison with the
Java tools (you need your own copy of the course files; they are not in this repository).

## License and credits

Nandwright is licensed under the [Mozilla Public License 2.0](LICENSE).

It builds on [isXander/nand2tetris](https://github.com/isXander/nand2tetris) (MPL-2.0),
whose HDL and test-script front-ends, hardware simulator and language server are the
starting point of the engine, and uses the Hack font table from the official
[web IDE](https://github.com/nand2tetris/web-ide) (MIT). [NOTICE.md](NOTICE.md) has the details.

Nand to Tetris and *The Elements of Computing Systems* are the work of Noam Nisan and
Shimon Schocken. This project is independent of them.
