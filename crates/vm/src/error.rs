//! Runtime errors and execution status types for the VM emulator.

use std::fmt;

/// A runtime error raised during VM or built-in OS execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmError {
    /// Stack pointer moved outside `[256, 2047]` during a push, pop, call, or function entry.
    StackOverflow { sp: i16, location: String },
    /// Stack underflow (tried to pop from an empty stack at `sp <= 256`).
    StackUnderflow { sp: i16, location: String },
    /// Memory segment access landed outside the segment's legal RAM range.
    OutOfSegmentSpace {
        segment: &'static str,
        address: i32,
        location: String,
    },
    /// `pop pointer 0` (`this`) received a value outside `<= 0` and `2048..=16383`,
    /// or `pop pointer 1` (`that`) received a value outside `0` and `2048..=24576`.
    IllegalPointer {
        pointer: u16,
        value: i16,
        location: String,
    },
    /// `return` popped a return address outside the program's instruction range,
    /// or `LCL < 5`.
    IllegalReturnAddress { address: i16, location: String },
    /// `goto`, `if-goto`, or `call` targeted a label or function that could not be resolved.
    UnresolvedTarget {
        kind: &'static str,
        location: String,
    },
    /// Executed a `function` whose name is malformed or whose class prefix does not
    /// match its file name.
    InvalidFunctionName { message: String, location: String },
    /// A built-in Jack OS function raised a runtime error (division by zero,
    /// negative sqrt, heap overflow, string index out of bounds, etc.).
    BuiltinError {
        function: &'static str,
        message: String,
    },
    /// `Sys.error(code)` was called (after printing `ERR<code>` to the screen).
    SysError { code: i16 },
    /// Built-in `Memory.peek`/`poke`/`deAlloc` accessed an out-of-range RAM address.
    MemoryOutOfRange {
        function: &'static str,
        address: i16,
    },
}

impl fmt::Display for VmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VmError::StackOverflow { sp, location } => {
                write!(f, "stack overflow (SP={sp}) at {location}")
            }
            VmError::StackUnderflow { sp, location } => {
                write!(f, "stack underflow (SP={sp}) at {location}")
            }
            VmError::OutOfSegmentSpace {
                segment,
                address,
                location,
            } => write!(
                f,
                "out of segment space: `{segment}` accessed RAM[{address}] at {location}"
            ),
            VmError::IllegalPointer {
                pointer,
                value,
                location,
            } => {
                if *pointer == 0 {
                    write!(
                        f,
                        "illegal `this` pointer ({value}): must be in the heap (2048..=16383) or <= 0 at {location}"
                    )
                } else {
                    write!(
                        f,
                        "illegal `that` pointer ({value}): must be in the heap or screen (2048..=24576) or 0 at {location}"
                    )
                }
            }
            VmError::IllegalReturnAddress { address, location } => {
                write!(f, "illegal return address ({address}) at {location}")
            }
            VmError::UnresolvedTarget { kind, location } => {
                write!(f, "unresolved {kind} target at {location}")
            }
            VmError::InvalidFunctionName { message, location } => {
                write!(f, "{message} at {location}")
            }
            VmError::BuiltinError { function, message } => {
                write!(f, "{function}: {message}")
            }
            VmError::SysError { code } => {
                write!(f, "Sys.error({code})")
            }
            VmError::MemoryOutOfRange { function, address } => {
                write!(
                    f,
                    "{function}: address {address} is outside the allowed memory range"
                )
            }
        }
    }
}

impl std::error::Error for VmError {}

/// Outcome of executing a single VM command via [`crate::Vm::step`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepStatus {
    /// Executed one VM command (or one complete built-in OS call).
    Stepped,
    /// The program has halted (`Sys.halt`, `Main.main` returned to built-in `Sys.init`,
    /// or execution reached the end of a script snippet).
    Halted,
    /// Suspended inside a built-in `Keyboard` routine waiting for keyboard state to change.
    WaitingForKey,
    /// Suspended inside `Sys.wait` until `now_ms >= until_ms`.
    Sleeping { until_ms: u64 },
    /// Execution stopped due to a runtime error.
    Error(VmError),
}

/// Outcome of executing up to `budget` VM commands via [`crate::Vm::run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunStatus {
    /// Exhausted the requested step budget.
    Budget,
    /// Stopped before executing the command at a matching breakpoint.
    Breakpoint { file: String, line: u32 },
    /// The program has halted.
    Halted,
    /// Suspended waiting for keyboard input.
    WaitingForKey,
    /// Suspended inside `Sys.wait` until `now_ms >= until_ms`.
    Sleeping { until_ms: u64 },
    /// Execution stopped due to a runtime error.
    Error(VmError),
}
