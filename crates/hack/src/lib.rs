//! The Hack platform of Nand to Tetris: assembler, disassembler and CPU
//! emulator, each matching the observable behaviour of the official Java
//! tools (see the documentation of [`asm`] and [`cpu`] for the details that
//! differ from what the book suggests).
//!
//! The library performs no I/O and compiles for `wasm32-unknown-unknown`.

pub mod asm;
pub mod cpu;
pub mod isa;

pub use asm::{Assembly, Symbol, SymbolKind, assemble, disassemble, hack_text, parse, parse_hack};
pub use cpu::{Cpu, CpuError, RunStatus};
