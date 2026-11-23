//! Native built-in implementation of the Jack OS (Project 12).
//!
//! Covers all 8 classes (`Math`, `String`, `Array`, `Output`, `Screen`,
//! `Keyboard`, `Memory`, `Sys`). Nothing ever blocks: `Sys.wait` and
//! `Keyboard.readChar`/`readLine`/`readInt` suspend via [`PendingBuiltin`] and
//! resume when the host advances time or updates the keyboard register.

pub mod font;
pub mod math;
pub mod memory;
pub mod output;
pub mod screen;
pub mod string;

use crate::error::VmError;
use crate::program::BuiltinOs;

/// Error codes defined by the Jack OS API (printed as `ERR<code>` before halting).
pub mod err_code {
    pub const SYS_WAIT_DURATION: i16 = 1;
    pub const ARRAY_SIZE: i16 = 2;
    pub const MATH_DIV_ZERO: i16 = 3;
    pub const MATH_SQRT_NEG: i16 = 4;
    pub const MEMORY_ALLOC_SIZE: i16 = 5;
    pub const MEMORY_HEAP_OVERFLOW: i16 = 6;
    pub const SCREEN_PIXEL: i16 = 7;
    pub const SCREEN_LINE: i16 = 8;
    pub const SCREEN_RECT: i16 = 9;
    pub const SCREEN_CIRCLE_CENTER: i16 = 12;
    pub const SCREEN_CIRCLE_RADIUS: i16 = 13;
    pub const STRING_MAX_LEN: i16 = 14;
    pub const STRING_CHAR_AT: i16 = 15;
    pub const STRING_SET_CHAR_AT: i16 = 16;
    pub const STRING_APPEND_FULL: i16 = 17;
    pub const STRING_ERASE_EMPTY: i16 = 18;
    pub const STRING_SET_INT_CAP: i16 = 19;
    pub const OUTPUT_CURSOR: i16 = 20;
}

/// Persistent state of the built-in Jack OS classes.
#[derive(Debug, Clone, Default)]
pub struct OsState {
    pub screen: screen::ScreenState,
    pub output: output::OutputState,
    pub pending: Option<PendingBuiltin>,
}

/// Suspended state for a non-blocking built-in call (`Sys.wait` or `Keyboard.read*`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingBuiltin {
    /// Waiting inside `Sys.wait` until `now_ms >= until_ms`.
    Sleep {
        until_ms: u64,
        return_pc: u32,
        frame_lcl: u16,
        frame_arg: u16,
    },
    /// Waiting inside `Keyboard.readChar`.
    ReadChar {
        phase: KeyPhase,
        return_pc: u32,
        frame_lcl: u16,
        frame_arg: u16,
    },
    /// Waiting inside `Keyboard.readLine` or `Keyboard.readInt`.
    ReadLine {
        phase: KeyPhase,
        chars: Vec<i16>,
        /// Extra characters that could never fit in a returned Jack String.
        excess: usize,
        /// `true` for `Keyboard.readInt`, `false` for `Keyboard.readLine`.
        as_int: bool,
        return_pc: u32,
        frame_lcl: u16,
        frame_arg: u16,
    },
}

/// Key press/release state machine for `Keyboard.readChar` and `Keyboard.readLine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPhase {
    /// Waiting for `RAM[24576]` to become non-zero.
    WaitPress,
    /// Key `code` was pressed; waiting for `RAM[24576]` to return to `0`.
    WaitRelease { code: i16 },
}

impl KeyPhase {
    fn released_key(&mut self, key: i16) -> Option<i16> {
        match *self {
            Self::WaitPress if key != 0 => {
                *self = Self::WaitRelease { code: key };
                None
            }
            Self::WaitRelease { code } if key == 0 => {
                *self = Self::WaitPress;
                Some(code)
            }
            _ => None,
        }
    }
}

/// Result of dispatching or resuming a built-in OS call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuiltinOutcome {
    /// Completed immediately with return value `value` (and `screen_touched` flag).
    Return { value: i16, screen_touched: bool },
    /// `Sys.halt()` was executed.
    Halt,
    /// Suspended until `now_ms >= until_ms`.
    Sleep { until_ms: u64 },
    /// Suspended until the keyboard register changes (`screen_touched` if cursor was drawn/updated).
    WaitKey { screen_touched: bool },
    /// Failed with a runtime error (`screen_touched` if `ERR<code>` was printed).
    Error {
        error: VmError,
        screen_touched: bool,
    },
}

impl OsState {
    pub fn reset(&mut self) {
        self.screen.init();
        self.output.init();
        self.pending = None;
    }

    /// Initializes all built-in OS subsystems (as performed by built-in `Sys.init`).
    pub fn init_all(&mut self, ram: &mut [i16]) {
        memory::init(ram);
        self.screen.init();
        self.output.init();
        self.pending = None;
    }

    /// Prints `ERR<code>` to the screen using `Output`, matching the Java VM Emulator.
    fn print_sys_error(&mut self, ram: &mut [i16], code: i16) {
        self.output.print_str_ascii(ram, "ERR");
        self.output.print_int(ram, code);
    }

    /// Executes a built-in OS function call with arguments `args`.
    ///
    /// For suspending calls (`Sys.wait`, `Keyboard.read*`), `frame_lcl`, `frame_arg`,
    /// and `return_pc` identify the active built-in call frame to tear down upon completion.
    #[allow(clippy::too_many_arguments)]
    pub fn call(
        &mut self,
        builtin: BuiltinOs,
        args: &[i16],
        ram: &mut [i16],
        now_ms: u64,
        frame_lcl: u16,
        frame_arg: u16,
        return_pc: u32,
    ) -> BuiltinOutcome {
        let a0 = args.first().copied().unwrap_or(0);
        let a1 = args.get(1).copied().unwrap_or(0);
        let a2 = args.get(2).copied().unwrap_or(0);
        let a3 = args.get(3).copied().unwrap_or(0);

        use BuiltinOs::*;
        match builtin {
            // ---- Math ----
            MathInit => BuiltinOutcome::Return {
                value: 0,
                screen_touched: false,
            },
            MathAbs => BuiltinOutcome::Return {
                value: math::abs(a0),
                screen_touched: false,
            },
            MathMultiply => BuiltinOutcome::Return {
                value: math::multiply(a0, a1),
                screen_touched: false,
            },
            MathDivide => match math::divide(a0, a1) {
                Ok(v) => BuiltinOutcome::Return {
                    value: v,
                    screen_touched: false,
                },
                Err(error) => {
                    self.print_sys_error(ram, err_code::MATH_DIV_ZERO);
                    BuiltinOutcome::Error {
                        error,
                        screen_touched: true,
                    }
                }
            },
            MathMin => BuiltinOutcome::Return {
                value: math::min(a0, a1),
                screen_touched: false,
            },
            MathMax => BuiltinOutcome::Return {
                value: math::max(a0, a1),
                screen_touched: false,
            },
            MathSqrt => match math::sqrt(a0) {
                Ok(v) => BuiltinOutcome::Return {
                    value: v,
                    screen_touched: false,
                },
                Err(error) => {
                    self.print_sys_error(ram, err_code::MATH_SQRT_NEG);
                    BuiltinOutcome::Error {
                        error,
                        screen_touched: true,
                    }
                }
            },

            // ---- Memory & Array ----
            MemoryInit => {
                memory::init(ram);
                BuiltinOutcome::Return {
                    value: 0,
                    screen_touched: false,
                }
            }
            MemoryPeek => match memory::peek(ram, a0) {
                Ok(v) => BuiltinOutcome::Return {
                    value: v,
                    screen_touched: false,
                },
                Err(error) => BuiltinOutcome::Error {
                    error,
                    screen_touched: false,
                },
            },
            MemoryPoke => match memory::poke(ram, a0, a1) {
                Ok(screen_touched) => BuiltinOutcome::Return {
                    value: 0,
                    screen_touched,
                },
                Err(error) => BuiltinOutcome::Error {
                    error,
                    screen_touched: false,
                },
            },
            MemoryAlloc => match memory::alloc(ram, a0) {
                Ok(ptr) => BuiltinOutcome::Return {
                    value: ptr,
                    screen_touched: false,
                },
                Err(error) => {
                    let code = if a0 <= 0 {
                        err_code::MEMORY_ALLOC_SIZE
                    } else {
                        err_code::MEMORY_HEAP_OVERFLOW
                    };
                    self.print_sys_error(ram, code);
                    BuiltinOutcome::Error {
                        error,
                        screen_touched: true,
                    }
                }
            },
            MemoryDeAlloc | ArrayDispose => match memory::de_alloc(ram, a0) {
                Ok(()) => BuiltinOutcome::Return {
                    value: 0,
                    screen_touched: false,
                },
                Err(error) => BuiltinOutcome::Error {
                    error,
                    screen_touched: false,
                },
            },
            ArrayNew => match memory::array_new(ram, a0) {
                Ok(ptr) => BuiltinOutcome::Return {
                    value: ptr,
                    screen_touched: false,
                },
                Err(error) => {
                    let code = if a0 <= 0 {
                        err_code::ARRAY_SIZE
                    } else {
                        err_code::MEMORY_HEAP_OVERFLOW
                    };
                    self.print_sys_error(ram, code);
                    BuiltinOutcome::Error {
                        error,
                        screen_touched: true,
                    }
                }
            },

            // ---- String ----
            StringNew => match string::new(ram, a0) {
                Ok(ptr) => BuiltinOutcome::Return {
                    value: ptr,
                    screen_touched: false,
                },
                Err(error) => {
                    let code = if a0 < 0 {
                        err_code::STRING_MAX_LEN
                    } else {
                        err_code::MEMORY_HEAP_OVERFLOW
                    };
                    self.print_sys_error(ram, code);
                    BuiltinOutcome::Error {
                        error,
                        screen_touched: true,
                    }
                }
            },
            StringDispose => match string::dispose(ram, a0) {
                Ok(v) => BuiltinOutcome::Return {
                    value: v,
                    screen_touched: false,
                },
                Err(error) => BuiltinOutcome::Error {
                    error,
                    screen_touched: false,
                },
            },
            StringLength => match string::length(ram, a0) {
                Ok(v) => BuiltinOutcome::Return {
                    value: v,
                    screen_touched: false,
                },
                Err(error) => BuiltinOutcome::Error {
                    error,
                    screen_touched: false,
                },
            },
            StringCharAt => match string::char_at(ram, a0, a1) {
                Ok(v) => BuiltinOutcome::Return {
                    value: v,
                    screen_touched: false,
                },
                Err(error) => {
                    let screen_touched = matches!(error, VmError::BuiltinError { .. });
                    if screen_touched {
                        self.print_sys_error(ram, err_code::STRING_CHAR_AT);
                    }
                    BuiltinOutcome::Error {
                        error,
                        screen_touched,
                    }
                }
            },
            StringSetCharAt => match string::set_char_at(ram, a0, a1, a2) {
                Ok(v) => BuiltinOutcome::Return {
                    value: v,
                    screen_touched: false,
                },
                Err(error) => {
                    let screen_touched = matches!(error, VmError::BuiltinError { .. });
                    if screen_touched {
                        self.print_sys_error(ram, err_code::STRING_SET_CHAR_AT);
                    }
                    BuiltinOutcome::Error {
                        error,
                        screen_touched,
                    }
                }
            },
            StringAppendChar => match string::append_char(ram, a0, a1) {
                Ok(v) => BuiltinOutcome::Return {
                    value: v,
                    screen_touched: false,
                },
                Err(error) => {
                    let screen_touched = matches!(error, VmError::BuiltinError { .. });
                    if screen_touched {
                        self.print_sys_error(ram, err_code::STRING_APPEND_FULL);
                    }
                    BuiltinOutcome::Error {
                        error,
                        screen_touched,
                    }
                }
            },
            StringEraseLastChar => match string::erase_last_char(ram, a0) {
                Ok(v) => BuiltinOutcome::Return {
                    value: v,
                    screen_touched: false,
                },
                Err(error) => {
                    let screen_touched = matches!(error, VmError::BuiltinError { .. });
                    if screen_touched {
                        self.print_sys_error(ram, err_code::STRING_ERASE_EMPTY);
                    }
                    BuiltinOutcome::Error {
                        error,
                        screen_touched,
                    }
                }
            },
            StringIntValue => match string::int_value(ram, a0) {
                Ok(v) => BuiltinOutcome::Return {
                    value: v,
                    screen_touched: false,
                },
                Err(error) => BuiltinOutcome::Error {
                    error,
                    screen_touched: false,
                },
            },
            StringSetInt => match string::set_int(ram, a0, a1) {
                Ok(v) => BuiltinOutcome::Return {
                    value: v,
                    screen_touched: false,
                },
                Err(error) => {
                    let screen_touched = matches!(error, VmError::BuiltinError { .. });
                    if screen_touched {
                        self.print_sys_error(ram, err_code::STRING_SET_INT_CAP);
                    }
                    BuiltinOutcome::Error {
                        error,
                        screen_touched,
                    }
                }
            },
            StringBackSpace => BuiltinOutcome::Return {
                value: string::back_space(),
                screen_touched: false,
            },
            StringDoubleQuote => BuiltinOutcome::Return {
                value: string::double_quote(),
                screen_touched: false,
            },
            StringNewLine => BuiltinOutcome::Return {
                value: string::new_line(),
                screen_touched: false,
            },

            // ---- Screen ----
            ScreenInit => {
                self.screen.init();
                BuiltinOutcome::Return {
                    value: 0,
                    screen_touched: false,
                }
            }
            ScreenClearScreen => {
                self.screen.clear_screen(ram);
                BuiltinOutcome::Return {
                    value: 0,
                    screen_touched: true,
                }
            }
            ScreenSetColor => {
                self.screen.set_color(a0);
                BuiltinOutcome::Return {
                    value: 0,
                    screen_touched: false,
                }
            }
            ScreenDrawPixel => match self.screen.draw_pixel(ram, a0, a1) {
                Ok(()) => BuiltinOutcome::Return {
                    value: 0,
                    screen_touched: true,
                },
                Err(error) => {
                    self.print_sys_error(ram, err_code::SCREEN_PIXEL);
                    BuiltinOutcome::Error {
                        error,
                        screen_touched: true,
                    }
                }
            },
            ScreenDrawLine => match self.screen.draw_line(ram, a0, a1, a2, a3) {
                Ok(()) => BuiltinOutcome::Return {
                    value: 0,
                    screen_touched: true,
                },
                Err(error) => {
                    self.print_sys_error(ram, err_code::SCREEN_LINE);
                    BuiltinOutcome::Error {
                        error,
                        screen_touched: true,
                    }
                }
            },
            ScreenDrawRectangle => match self.screen.draw_rectangle(ram, a0, a1, a2, a3) {
                Ok(()) => BuiltinOutcome::Return {
                    value: 0,
                    screen_touched: true,
                },
                Err(error) => {
                    self.print_sys_error(ram, err_code::SCREEN_RECT);
                    BuiltinOutcome::Error {
                        error,
                        screen_touched: true,
                    }
                }
            },
            ScreenDrawCircle => match self.screen.draw_circle(ram, a0, a1, a2) {
                Ok(()) => BuiltinOutcome::Return {
                    value: 0,
                    screen_touched: true,
                },
                Err(error @ VmError::MemoryOutOfRange { .. }) => BuiltinOutcome::Error {
                    error,
                    screen_touched: true,
                },
                Err(error) => {
                    let code = if !(0..screen::SCREEN_WIDTH).contains(&a0)
                        || !(0..screen::SCREEN_HEIGHT).contains(&a1)
                    {
                        err_code::SCREEN_CIRCLE_CENTER
                    } else {
                        err_code::SCREEN_CIRCLE_RADIUS
                    };
                    self.print_sys_error(ram, code);
                    BuiltinOutcome::Error {
                        error,
                        screen_touched: true,
                    }
                }
            },

            // ---- Output ----
            OutputInit => {
                self.output.init();
                BuiltinOutcome::Return {
                    value: 0,
                    screen_touched: false,
                }
            }
            OutputMoveCursor => match self.output.move_cursor(a0, a1) {
                Ok(()) => BuiltinOutcome::Return {
                    value: 0,
                    screen_touched: false,
                },
                Err(error) => {
                    self.print_sys_error(ram, err_code::OUTPUT_CURSOR);
                    BuiltinOutcome::Error {
                        error,
                        screen_touched: true,
                    }
                }
            },
            OutputPrintChar => {
                self.output.print_char(ram, a0);
                BuiltinOutcome::Return {
                    value: 0,
                    screen_touched: true,
                }
            }
            OutputPrintString => match self.output.print_string(ram, a0) {
                Ok(()) => BuiltinOutcome::Return {
                    value: 0,
                    screen_touched: true,
                },
                Err(error) => BuiltinOutcome::Error {
                    error,
                    screen_touched: true,
                },
            },
            OutputPrintInt => {
                self.output.print_int(ram, a0);
                BuiltinOutcome::Return {
                    value: 0,
                    screen_touched: true,
                }
            }
            OutputPrintln => {
                self.output.println();
                BuiltinOutcome::Return {
                    value: 0,
                    screen_touched: false,
                }
            }
            OutputBackSpace => {
                self.output.back_space(ram);
                BuiltinOutcome::Return {
                    value: 0,
                    screen_touched: true,
                }
            }

            // ---- Keyboard ----
            KeyboardInit => BuiltinOutcome::Return {
                value: 0,
                screen_touched: false,
            },
            KeyboardKeyPressed => BuiltinOutcome::Return {
                value: ram[memory::KEYBOARD_ADDR as usize],
                screen_touched: false,
            },
            KeyboardReadChar => {
                self.output.draw_cursor_box(ram);
                self.pending = Some(PendingBuiltin::ReadChar {
                    phase: KeyPhase::WaitPress,
                    return_pc,
                    frame_lcl,
                    frame_arg,
                });
                let outcome = self.poll_pending(ram, now_ms);
                match outcome {
                    BuiltinOutcome::WaitKey { .. } => BuiltinOutcome::WaitKey {
                        screen_touched: true,
                    },
                    BuiltinOutcome::Return { value, .. } => BuiltinOutcome::Return {
                        value,
                        screen_touched: true,
                    },
                    other => other,
                }
            }
            KeyboardReadLine | KeyboardReadInt => {
                if let Err(error) = self.output.print_string(ram, a0) {
                    return BuiltinOutcome::Error {
                        error,
                        screen_touched: true,
                    };
                }
                self.output.draw_cursor_box(ram);
                self.pending = Some(PendingBuiltin::ReadLine {
                    phase: KeyPhase::WaitPress,
                    chars: Vec::new(),
                    excess: 0,
                    as_int: matches!(builtin, KeyboardReadInt),
                    return_pc,
                    frame_lcl,
                    frame_arg,
                });
                let outcome = self.poll_pending(ram, now_ms);
                match outcome {
                    BuiltinOutcome::WaitKey { .. } => BuiltinOutcome::WaitKey {
                        screen_touched: true,
                    },
                    BuiltinOutcome::Return { value, .. } => BuiltinOutcome::Return {
                        value,
                        screen_touched: true,
                    },
                    other => other,
                }
            }

            // ---- Sys ----
            SysInit => {
                self.init_all(ram);
                BuiltinOutcome::Return {
                    value: 0,
                    screen_touched: false,
                }
            }
            SysHalt => BuiltinOutcome::Halt,
            SysError => {
                self.print_sys_error(ram, a0);
                BuiltinOutcome::Error {
                    error: VmError::SysError { code: a0 },
                    screen_touched: true,
                }
            }
            SysWait => {
                if a0 < 0 {
                    self.print_sys_error(ram, err_code::SYS_WAIT_DURATION);
                    return BuiltinOutcome::Error {
                        error: VmError::BuiltinError {
                            function: "Sys.wait",
                            message: "duration must be non-negative".to_string(),
                        },
                        screen_touched: true,
                    };
                }
                if a0 == 0 {
                    return BuiltinOutcome::Return {
                        value: 0,
                        screen_touched: false,
                    };
                }
                let until_ms = now_ms.saturating_add(a0 as u64);
                self.pending = Some(PendingBuiltin::Sleep {
                    until_ms,
                    return_pc,
                    frame_lcl,
                    frame_arg,
                });
                BuiltinOutcome::Sleep { until_ms }
            }
        }
    }

    /// Advances an active [`PendingBuiltin`] without blocking.
    ///
    /// Returns `BuiltinOutcome::Return` once the wait completes (and clears `self.pending`).
    pub fn poll_pending(&mut self, ram: &mut [i16], now_ms: u64) -> BuiltinOutcome {
        let Some(mut pending) = self.pending.take() else {
            return BuiltinOutcome::Return {
                value: 0,
                screen_touched: false,
            };
        };

        let key = ram[memory::KEYBOARD_ADDR as usize];
        let outcome = match &mut pending {
            PendingBuiltin::Sleep { until_ms, .. } if now_ms < *until_ms => BuiltinOutcome::Sleep {
                until_ms: *until_ms,
            },
            PendingBuiltin::Sleep { .. } => BuiltinOutcome::Return {
                value: 0,
                screen_touched: false,
            },
            PendingBuiltin::ReadChar { phase, .. } => {
                if let Some(code) = phase.released_key(key) {
                    self.output.draw_char_at_cursor(ram, 32);
                    self.output.print_char(ram, code);
                    BuiltinOutcome::Return {
                        value: code,
                        screen_touched: true,
                    }
                } else {
                    BuiltinOutcome::WaitKey {
                        screen_touched: false,
                    }
                }
            }
            PendingBuiltin::ReadLine {
                phase,
                chars,
                excess,
                as_int,
                ..
            } => match phase.released_key(key) {
                Some(128) => self.finish_read_line(ram, chars, *excess, *as_int),
                Some(129) if *excess != 0 || !chars.is_empty() => {
                    if *excess != 0 {
                        *excess -= 1;
                    } else {
                        chars.pop();
                    }
                    self.output.back_space(ram);
                    self.output.draw_cursor_box(ram);
                    BuiltinOutcome::WaitKey {
                        screen_touched: true,
                    }
                }
                Some(code) if code != 129 => {
                    if chars.len() < string::MAX_CAPACITY {
                        chars.push(code);
                    } else {
                        *excess = excess.saturating_add(1);
                    }
                    self.output.print_char(ram, code);
                    self.output.draw_cursor_box(ram);
                    BuiltinOutcome::WaitKey {
                        screen_touched: true,
                    }
                }
                _ => BuiltinOutcome::WaitKey {
                    screen_touched: false,
                },
            },
        };
        if matches!(
            outcome,
            BuiltinOutcome::Sleep { .. } | BuiltinOutcome::WaitKey { .. }
        ) {
            self.pending = Some(pending);
        }
        outcome
    }

    fn finish_read_line(
        &mut self,
        ram: &mut [i16],
        chars: &[i16],
        excess: usize,
        as_int: bool,
    ) -> BuiltinOutcome {
        self.output.draw_char_at_cursor(ram, 32);
        self.output.println();
        // Java buffers input outside Hack RAM. Only readLine allocates a Jack
        // String, sized to the final line after editing.
        let result = if excess != 0 {
            Err(VmError::BuiltinError {
                function: if as_int {
                    "Keyboard.readInt"
                } else {
                    "Keyboard.readLine"
                },
                message: "input exceeds the maximum Jack string size".to_string(),
            })
        } else if as_int {
            string::parse_int(chars.iter().copied().map(Ok))
        } else {
            string::from_chars(ram, chars)
        };
        match result {
            Ok(value) => BuiltinOutcome::Return {
                value,
                screen_touched: true,
            },
            Err(error) => {
                self.print_sys_error(ram, err_code::MEMORY_HEAP_OVERFLOW);
                BuiltinOutcome::Error {
                    error,
                    screen_touched: true,
                }
            }
        }
    }
}
