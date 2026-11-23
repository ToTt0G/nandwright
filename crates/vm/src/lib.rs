//! Nand to Tetris VM language front-end, VM emulator, and built-in Jack OS.
//!
//! - [`parse`] parses a single `.vm` file into commands with source spans and
//!   structured diagnostics, recovering per line and never panicking.
//! - [`Program::link`] links one or more parsed files into a flat, pre-resolved
//!   instruction stream (resolving `goto`/`if-goto`, cross-file `call`s,
//!   built-in Jack OS calls, and per-file `static` segments).
//! - [`Vm`] executes a linked [`Program`] on the Hack RAM model with a complete
//!   non-blocking native implementation of the Jack OS (`Math`, `String`,
//!   `Array`, `Output`, `Screen`, `Keyboard`, `Memory`, `Sys`).

pub mod error;
mod os;
pub mod parse;
pub mod program;
pub mod vm;

pub use error::{RunStatus, StepStatus, VmError};
pub use os::memory::{HEAP_END, HEAP_START, KEYBOARD_ADDR, RAM_LEN, SCREEN_END, SCREEN_START};
pub use parse::{ArithOp, Command, CommandKind, Name, ParsedVm, Segment, parse};
pub use program::{BuiltinOs, CallTarget, EntryPoint, FileInfo, FunctionInfo, Op, Program};
pub use vm::{Breakpoints, CallFrame, Vm};
