# Architecture

Nandwright is a VS Code extension with one engine, written in Rust, that is compiled
twice from the same source:

- to **WebAssembly** (`wasm32-unknown-unknown`), which is what ships in the VSIX. The
  same bytes run on Windows, macOS and Linux, so there is no per-platform binary, no
  Java and no toolchain on the user's machine.
- to a **native CLI** (`n2t`), used by the test suite and the conformance harness,
  which diffs the engine against the official Java tools.

```
extension/            VS Code extension (TypeScript)
  src/extension.ts      activation and commands
  src/engine/wasm.ts    loads the engine and gives it the file system
  src/server/           language-server process: LSP transport around the engine
  src/runtime/          worker thread: test runs, simulators, emulators
  src/testing.ts        the Testing view
  src/chipPanel.ts      the chip simulator panel        (webview: src/webview/chip.ts)
  src/circuitPanel.ts   the circuit diagram panel       (webview: src/webview/circuit*.ts)
  src/chipSessions.ts   one hub per chip file: the shared runtime session, its queue and events
  src/computerPanel.ts  the CPU / VM emulator panel     (webview: src/webview/computer.ts)
  src/debug.ts          debug adapter that steps through test scripts
  src/outputs.ts        assemble, compile, compare with the reference
crates/
  core/        n2t-core        HDL + TST front-ends, built-in chip interfaces, formatters,
                               diagnostics, the `Host` file-system abstraction
  simulator/   n2t-simulator   hardware circuit simulation and the test-script runner
  hack/        n2t-hack        Hack assembler, disassembler, CPU emulator
  vm/          n2t-vm          VM language front-end, VM emulator, built-in Jack OS
  jack/        n2t-jack        Jack compiler
  lsp/         n2t-lsp         language server (a message-in, messages-out state machine)
  cli/         n2t-cli         the native `n2t` command
  wasm/        n2t-wasm        the wasm ABI: one JSON `call`, plus host imports
conformance/          harness that runs the official Java tools and `n2t` on the same
                      inputs and diffs them (course files live outside the repository)
tests/fixtures/       original chips and programs (never course assignments)
examples/             practice chips, assembly and Jack programs to try the extension on
scripts/              the check that keeps course material out of the repository
```

## Rules every crate follows

1. **No I/O in library code.** Library crates never touch `std::fs`, `std::env`,
   `std::process`, `std::time` or threads: they must compile for
   `wasm32-unknown-unknown`. Files are reached only through `n2t_core::host::Host`,
   which the CLI implements with `std::fs` and the extension implements with Node's
   `fs`. Pure functions over text are preferred wherever a file is not needed.
2. **Paths are opaque strings in engine form**: `/` separators, a Windows drive kept
   as `C:/…`. Only `n2t_core::host::path` splits or joins them. `std::path` is not
   used in library code, because on wasm it has Unix semantics.
3. **Never panic on user input.** Every parser accepts arbitrary bytes and returns
   diagnostics. No `unwrap`/`expect`/indexing on anything derived from input, no
   unbounded recursion or allocation driven by a number in a source file.
4. **Long work is resumable.** Anything that can loop (a script, a CPU, a VM) exposes
   `run(budget)` that returns after at most `budget` steps with a status, so the host
   can stay responsive, cancel, and time-slice. Nothing blocks waiting for a key press
   or a timer; it returns a "waiting" status instead.
5. **Match the Java tools.** Where the official Java suite and the book or web IDE
   disagree, the Java behaviour wins, because that is what coursework is graded
   against. Each deliberate difference is listed in `docs/DEVIATIONS.md`.
6. **Diagnostics** use `n2t_core::diagnostic::Diagnostic` with byte-offset `Span`s.
   New crates use `DiagnosticCode::Other("asm-…" | "vm-…" | "jack-…")`.
7. **No course material in the repository**: no official tools, skeletons, `.tst`,
   `.cmp`, OS sources, and no solutions to course projects. Tests in the repository
   use original chips and programs. Conformance tests read the course tree from
   `N2T_COURSE_DIR` and private solutions from `N2T_PRIVATE_DIR`, both outside the repo.
8. **Clean-room.** The official Java tools are a black box to compare against. Their
   class files are not decompiled and their sources are not copied or translated.

## Engine interfaces between crates

### n2t-hack

```rust
pub mod asm {
    /// Never panics. `words` is empty if there are errors.
    pub fn assemble(source: &str) -> Assembly;
    pub struct Assembly {
        pub words: Vec<u16>,
        /// For each ROM address, the span of the instruction that produced it.
        pub source_map: Vec<Span>,
        pub symbols: Vec<Symbol>,         // labels and variables with addresses and spans
        pub diagnostics: Vec<Diagnostic>,
    }
    pub fn hack_text(words: &[u16]) -> String;          // "0000000000000010\n…"
    pub fn parse_hack(text: &str) -> Result<Vec<u16>, Vec<Diagnostic>>;
    pub fn disassemble(word: u16) -> String;            // "@2", "D=A", "0;JMP"
    /// Line-level syntax tree with spans, for the language server.
    pub fn parse(source: &str) -> ParsedAsm;
}
pub mod cpu {
    pub struct Cpu { /* a, d, pc, ram, rom */ }
    impl Cpu {
        pub fn new() -> Self;
        pub fn load_rom(&mut self, words: &[u16]);
        pub fn reset(&mut self);                         // pc = 0, like the reset button
        pub fn step(&mut self) -> Result<(), CpuError>;  // one ticktock
        pub fn run(&mut self, budget: u64, breakpoints: &[u16]) -> RunStatus;
        // a(), d(), pc(), ram(addr), set_*(…), ram_slice(), rom_slice(), set_key(code)
    }
}
```

### n2t-vm

```rust
pub fn parse(file_stem: &str, source: &str) -> ParsedVm;        // commands + spans + diagnostics
pub struct Program;                                              // linked from 1..n parsed files
impl Program { pub fn link(files: Vec<ParsedVm>) -> (Program, Vec<Diagnostic>); }
pub struct Vm;                                                   // RAM, call stack, built-in OS
impl Vm {
    pub fn new(program: Program) -> Self;
    pub fn step(&mut self, now_ms: u64) -> StepStatus;           // one VM command
    pub fn run(&mut self, budget: u64, now_ms: u64, breakpoints: &Breakpoints) -> RunStatus;
    // segments, stack, call stack, current function and line, ram(), set_key(code)
}
```

Functions the loaded program does not define resolve to the engine's built-in OS
(`Math`, `String`, `Array`, `Output`, `Screen`, `Keyboard`, `Memory`, `Sys`), exactly as
the Java VM emulator does.

### n2t-jack

```rust
pub fn compile(file_stem: &str, source: &str) -> Compiled;      // one class per file
pub struct Compiled {
    pub vm: Option<String>,             // None if there are errors
    pub diagnostics: Vec<Diagnostic>,
    pub line_map: Vec<u32>,             // VM command index -> 0-based Jack line
}
pub fn parse(source: &str) -> ParsedJack;                        // tokens + tree, for the LSP
```

### n2t-simulator: the test-script runner

One runner executes all three script dialects. It owns control flow (`repeat`,
`while`, terminators), `output-list` formatting, `.out` writing, `.cmp` comparison,
`echo` and breakpoints, and drives a `Machine`:

- **Hardware** (`load X.hdl`; `eval`, `tick`, `tock`; pins and built-in chip state),
- **Cpu** (`load X.asm|X.hack`; `ticktock`; `A`, `D`, `PC`, `RAM[i]`, `ROM[i]`),
- **Vm** (`load X.vm|dir|` nothing; `vmstep`; `sp`, `local`, `argument`, `this`, `that`,
  `RAM[i]`, `local[i]`, …).

The dialect is chosen from the first `load` command (or, without one, from the
commands used), because the Java suite makes the user pick the right program and an
editor should not.

For interactive inspection (such as the circuit diagram panel), `Circuit::with_probes`
(and `HardwareMachine::with_probes`) opts in to retaining a flat `u32`-per-bit mapping
and compact instance tree after union-find flattening so any composite instance's nets
and any part's own pins can be read at arbitrary depth via `Circuit::instance(path)`.
Test runs (`Circuit::new` / `HardwareMachine::new`) never build probes.

## The wasm ABI

`n2t-wasm` exports `nw_alloc`, `nw_free` and `nw_call(ptr, len) -> ptr` (JSON in, JSON
out) and imports `read`, `list`, `write` and `log` from the module `host`. `Engine::call`
in `crates/wasm/src/lib.rs` is the whole API, and the crate's tests drive that same
function natively. Two instances run in the extension:

- the **language server** process feeds LSP JSON-RPC messages to `{"op":"lsp"}` and
  sends back whatever messages come out. If the engine traps, the process makes a new
  instance and replays the handshake and the open documents;
- the **runtime** worker thread owns *sessions*: a test run (`test.*`), an interactive
  chip (`chip.*`), a Hack computer (`cpu.*`) or a VM (`vm.*`). Sessions are stepped
  with a budget so a runaway program can always be paused or cancelled, and a trap
  tears down one instance, not the editor. The client (`runtime/client.ts`) fires
  `onDidStop` whenever every session is gone: the worker stopped, or it replaced a
  trapped engine (a reply with `crashed: true`, announced before that reply arrives).

Unsaved editor buffers are pushed to the runtime as overlays (`overlay.set`) before a
run, so tests and simulators use what is on screen. Output that a student's own tool
may have produced (`.hack`, `.vm`) is written only when asked (`asm.assemble`,
`jack.compile` with `"write": true`).

## The extension's surfaces

Native VS Code surfaces are used wherever one exists: the Testing view for test
scripts, the Problems panel and hovers for diagnostics, CodeLens and the editor title
for actions, the debugger for stepping through scripts, the diff editor for
comparisons. Three webview panels exist for what is inherently visual: a chip's pins,
a chip's circuit diagram, and a computer's screen, keyboard and memory. They are drawn
with VS Code's theme variables only.

The circuit diagram (`docs/CIRCUIT_DIAGRAM.md`) draws one level of a chip as a live
schematic, with drill-down into parts that are chips of the student's own. Its layout is
computed in the webview from the engine's `chip.diagram` reply; values come from
`chip.values` after each change. When the chip simulator and the circuit diagram show
the same file they share one runtime chip session through a hub (`chipSessions.ts`):
every runtime call for that session, from either panel, runs through the hub's one
queue, and the hub publishes each outcome to both panels in the order the engine ran
it. The hub also reopens the chip after `onDidStop`, at most three times a minute;
after that the panels offer Retry.
