//! Simulators and the test-script runner for Nand to Tetris.
//!
//! - [`circuit::Circuit`] instantiates a validated
//!   [`n2t_core::hdl::ChipDefinition`] with fresh state and evaluates it with
//!   the two-phase `tick`/`tock` clock.
//! - [`primitives`] holds native implementations of the built-in chips.
//! - [`runner::ScriptRunner`] executes `.tst` scripts of every dialect against
//!   a [`machine::Machine`], writing `.out` files and comparing against `.cmp`
//!   files ([`format`]).

pub mod circuit;
pub mod cpu_machine;
pub mod format;
pub mod machine;
pub mod primitives;
pub mod program;
pub mod runner;
pub mod vm_machine;

pub use circuit::{Circuit, SimError, Time};
pub use cpu_machine::CpuMachine;
pub use machine::{HardwareMachine, Machine, Observed, Role};
pub use primitives::{BuiltinRegistry, Primitive};
pub use runner::{
    Event, Failure, Newline, Outcome, RunError, RunnerOptions, ScriptRunner, StartError,
    SuspendReason,
};
pub use vm_machine::VmMachine;
