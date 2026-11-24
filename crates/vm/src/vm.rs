//! The Nand2Tetris stack-machine VM emulator.

use std::collections::HashSet;

use crate::error::{RunStatus, StepStatus, VmError};
use crate::os::memory::{HEAP_END, HEAP_START, KEYBOARD_ADDR, RAM_LEN, SCREEN_END, SCREEN_START};
use crate::os::string::write_word;
use crate::os::{BuiltinOutcome, OsState};
use crate::parse::Segment;
use crate::program::{BuiltinOs, CallTarget, EntryPoint, Op, Program};

const STACK_MIN: i16 = 256;
const STACK_MAX: i16 = 2047;
const TEMP_BASE: u16 = 5;
/// Sentinel return address pushed by built-in `Sys.init` when calling `Main.main`.
const BUILTIN_SYS_RETURN_ADDR: i16 = -1;

/// A set of `(file_stem, 0-based line)` breakpoints for [`Vm::run`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Breakpoints {
    points: HashSet<(String, u32)>,
}

impl Breakpoints {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, file: impl Into<String>, line: u32) -> bool {
        self.points.insert((file.into(), line))
    }

    pub fn remove(&mut self, file: &str, line: u32) -> bool {
        self.points.remove(&(file.to_string(), line))
    }

    pub fn clear(&mut self) {
        self.points.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn contains(&self, file: &str, line: u32) -> bool {
        if self.points.is_empty() {
            return false;
        }
        self.points.iter().any(|(f, l)| *l == line && f == file)
    }
}

/// Snapshot of one frame on the VM call stack, for UI panels and debug adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallFrame {
    pub function: String,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub local_base: u16,
    pub argument_base: u16,
    pub locals: Vec<i16>,
    pub arguments: Vec<i16>,
}

/// A built-in OS function waiting for the program's own function it called
/// (the Java VM Emulator's built-in `Array.new`, `Array.dispose`, `String.new`
/// and `String.dispose` call `Memory.alloc` or `Memory.deAlloc` through the VM,
/// so a program's own `Memory.vm` serves them).
#[derive(Debug, Clone, Copy)]
struct BuiltinWait {
    /// What the built-in does with the returned value.
    then: AfterCall,
    /// The built-in's own frame, to return from.
    frame_lcl: u16,
    frame_arg: u16,
    return_pc: u32,
}

#[derive(Debug, Clone, Copy)]
enum AfterCall {
    /// Return the value (`Array.new`).
    ReturnValue,
    /// Set up a string of this capacity at the address and return it (`String.new`).
    NewString(i16),
    /// Return 0 (`Array.dispose`, `String.dispose`).
    ReturnZero,
}

/// Name shown for the frame of the built-in `Sys.init` that boots `Main.main`.
const BUILTIN_SYS_INIT_FRAME: &str = "Sys.init (built-in)";

/// Most shadow frames kept for [`Vm::call_stack`]. A real call needs at least
/// five stack words, so an honest program never gets deeper than ~360 frames;
/// a program that pops its own frame and calls again can recurse forever with
/// a flat `SP`, and without this cap the records would exhaust memory. When the
/// cap is hit the oldest half is dropped (the panel shows the newest frames).
const MAX_SHADOW_FRAMES: usize = 4096;

/// Internal shadow frame record used to inspect the call stack cleanly.
#[derive(Debug, Clone)]
struct FrameRecord {
    /// Index into [`Program::functions`]; `None` for the built-in `Sys.init`.
    function: Option<u32>,
    file_idx: Option<u32>,
    call_line: Option<u32>,
    local_base: u16,
    argument_base: u16,
    n_locals: u16,
    n_args: u16,
}

/// The Hack VM Emulator: RAM, stack-machine CPU, and built-in Jack OS.
#[derive(Debug, Clone)]
pub struct Vm {
    program: Program,
    ram: Box<[i16; RAM_LEN]>,
    pc: usize,
    steps: u64,
    halted: bool,
    boot_pending: bool,
    /// The built-in `Sys.init`'s next call: an index into the program's OS
    /// `init` functions, then `Main.main`.
    boot_next: usize,
    /// Built-in OS functions waiting for the program's own functions they called.
    builtin_waits: Vec<BuiltinWait>,
    screen_generation: u64,
    screen_dirty: bool,
    current_function: String,
    last_index_in_func: Option<i32>,
    call_frames: Vec<FrameRecord>,
    os: OsState,
}

impl Vm {
    /// Creates a new VM loaded with `program`, initialized to its starting state.
    pub fn new(program: Program) -> Self {
        let mut vm = Self {
            program,
            ram: Box::new([0; RAM_LEN]),
            pc: 0,
            steps: 0,
            halted: false,
            boot_pending: false,
            boot_next: 0,
            builtin_waits: Vec::new(),
            screen_generation: 0,
            screen_dirty: false,
            current_function: String::new(),
            last_index_in_func: None,
            call_frames: Vec::new(),
            os: OsState::default(),
        };
        vm.reset();
        vm
    }

    /// Resets RAM, pointers, OS state, and PC back to the program's entry point.
    pub fn reset(&mut self) {
        self.ram.fill(0);
        self.ram[0] = STACK_MIN; // SP = 256
        self.steps = 0;
        self.halted = false;
        self.screen_generation = self.screen_generation.wrapping_add(1);
        self.screen_dirty = true;
        self.current_function.clear();
        self.last_index_in_func = None;
        self.call_frames.clear();
        self.builtin_waits.clear();
        self.os.reset();

        match self.program.entry {
            EntryPoint::UserSysInit { pc } => {
                self.pc = pc as usize;
                self.boot_pending = false;
            }
            EntryPoint::BuiltinSysInit { main_pc, .. } => {
                self.pc = main_pc as usize;
                self.boot_pending = true;
            }
            EntryPoint::StartOfProgram { pc } => {
                self.pc = pc as usize;
                self.boot_pending = false;
            }
        }
    }

    /// Executes a single VM command (the unit of the script command `vmstep`).
    ///
    /// Built-in OS calls execute atomically in one step unless they wait on
    /// `Sys.wait` or `Keyboard.read*`, in which case they return
    /// [`StepStatus::Sleeping`] or [`StepStatus::WaitingForKey`] without blocking.
    pub fn step(&mut self, now_ms: u64) -> StepStatus {
        if self.halted {
            return StepStatus::Halted;
        }

        // Resume any suspended built-in OS call (`Sys.wait` / `Keyboard.read*`).
        if self.os.pending.is_some() {
            return self.resume_pending(now_ms);
        }

        // First step of a program booted via built-in `Sys.init`.
        if self.boot_pending {
            self.boot_builtin_sys_init();
            return StepStatus::Stepped;
        }

        if self.pc >= self.program.ops.len() {
            self.halted = true;
            return StepStatus::Halted;
        }

        self.execute_one(now_ms)
    }

    /// Executes up to `budget` VM commands, stopping early on a breakpoint,
    /// halt, keyboard wait, sleep, or runtime error.
    ///
    /// Breakpoints are checked before each command, including the first one.
    /// Use `step` to advance past a breakpoint before continuing.
    /// A runtime error is reported once. Later calls return `Halted` until
    /// reset or an explicit script assignment to `line`.
    pub fn run(&mut self, budget: u64, now_ms: u64, breakpoints: &Breakpoints) -> RunStatus {
        if self.halted {
            return RunStatus::Halted;
        }

        let has_bp = !breakpoints.is_empty();
        // Pre-resolve breakpoints for this program into a fast Vec of (file_idx, line)
        // only when breakpoints are non-empty, so the hot loop stays allocation-free
        // when `breakpoints.is_empty()`.
        let resolved_bp: Vec<(u32, u32)> = if has_bp {
            breakpoints
                .points
                .iter()
                .filter_map(|(file, line)| self.program.file_index(file).map(|fidx| (fidx, *line)))
                .collect()
        } else {
            Vec::new()
        };
        let check_bp = !resolved_bp.is_empty();

        for _ in 0..budget {
            if self.halted {
                return RunStatus::Halted;
            }
            if self.os.pending.is_some() {
                match self.resume_pending(now_ms) {
                    StepStatus::Stepped => continue,
                    StepStatus::Halted => return RunStatus::Halted,
                    StepStatus::WaitingForKey => return RunStatus::WaitingForKey,
                    StepStatus::Sleeping { until_ms } => return RunStatus::Sleeping { until_ms },
                    StepStatus::Error(e) => return RunStatus::Error(e),
                }
            }
            if self.boot_pending {
                self.boot_builtin_sys_init();
                continue;
            }
            if self.pc >= self.program.ops.len() {
                self.halted = true;
                return RunStatus::Halted;
            }
            if check_bp {
                let m = self.program.meta[self.pc];
                if resolved_bp
                    .iter()
                    .any(|&(f, l)| f == m.file_idx && l == m.line)
                {
                    let file = self.program.files[m.file_idx as usize].stem.clone();
                    return RunStatus::Breakpoint { file, line: m.line };
                }
            }
            match self.execute_one(now_ms) {
                StepStatus::Stepped => {}
                StepStatus::Halted => return RunStatus::Halted,
                StepStatus::WaitingForKey => return RunStatus::WaitingForKey,
                StepStatus::Sleeping { until_ms } => return RunStatus::Sleeping { until_ms },
                StepStatus::Error(e) => return RunStatus::Error(e),
            }
        }
        RunStatus::Budget
    }

    fn boot_builtin_sys_init(&mut self) {
        self.boot_pending = false;
        let memory_init = self.program.function_by_name("Memory.init").is_some();
        if memory_init {
            self.os.init_all_but_memory();
        } else {
            self.os.init_all(&mut *self.ram);
        }

        // Built-in Sys.init frame at 256..260; its calls get frames from 261.
        self.ram[256] = 10;
        self.ram[257] = 0;
        self.ram[258] = 0;
        self.ram[259] = 0;
        self.ram[260] = 0;
        self.ram[0] = 261; // SP
        self.ram[1] = 261; // LCL
        self.ram[2] = 256; // ARG
        self.ram[3] = 0; // THIS
        self.ram[4] = 0; // THAT

        self.current_function.clear();
        self.current_function.push_str(BUILTIN_SYS_INIT_FRAME);
        self.last_index_in_func = Some(2);
        self.call_frames.push(FrameRecord {
            function: None,
            file_idx: None,
            call_line: None,
            local_base: 261,
            argument_base: 256,
            n_locals: 0,
            n_args: 0,
        });
        self.boot_next = 0;
        self.boot_call_next();
        self.steps += 1;
    }

    /// The built-in `Sys.init` calls the next function of its sequence: the
    /// program's own OS `init` functions (as the Java VM Emulator does), then
    /// `Main.main`. Returns `false` when the sequence is over.
    fn boot_call_next(&mut self) -> bool {
        let EntryPoint::BuiltinSysInit {
            main_pc,
            main_func_id,
        } = self.program.entry
        else {
            return false;
        };
        let (pc, func_id) = match self.program.boot_inits.get(self.boot_next) {
            Some(&id) => (self.program.functions[id as usize].start_pc, id),
            None if self.boot_next == self.program.boot_inits.len() => (main_pc, main_func_id),
            None => return false,
        };
        self.boot_next += 1;
        debug_assert_eq!(self.program.functions[func_id as usize].start_pc, pc);
        self.call_from_builtin(func_id, 0);
        true
    }

    /// The program's own `Memory.alloc` or `Memory.deAlloc` that a built-in OS
    /// function calls (as the Java VM Emulator does), with its argument and what
    /// the built-in does with the result. `None` when the built-in does its own
    /// work: the program has no such function, or the argument is an error the
    /// built-in reports before calling.
    fn user_memory_target(&self, builtin: BuiltinOs, arg: i16) -> Option<(u32, i16, AfterCall)> {
        match builtin {
            BuiltinOs::ArrayNew if arg > 0 => self
                .program
                .user_alloc
                .map(|f| (f, arg, AfterCall::ReturnValue)),
            BuiltinOs::StringNew if arg >= 0 => self
                .program
                .user_alloc
                .map(|f| (f, arg.wrapping_add(2), AfterCall::NewString(arg))),
            BuiltinOs::ArrayDispose | BuiltinOs::StringDispose => self
                .program
                .user_dealloc
                .map(|f| (f, arg, AfterCall::ReturnZero)),
            _ => None,
        }
    }

    /// A built-in OS function calls the program's function `func_id`, whose
    /// `n_args` arguments are on the stack: a frame returning to the built-in.
    fn call_from_builtin(&mut self, func_id: u32, n_args: u16) {
        let base = self.ram[0].max(0) as usize;
        self.ram[base] = BUILTIN_SYS_RETURN_ADDR;
        self.ram[base + 1] = self.ram[1]; // saved LCL
        self.ram[base + 2] = self.ram[2]; // saved ARG
        self.ram[base + 3] = self.ram[3]; // saved THIS
        self.ram[base + 4] = self.ram[4]; // saved THAT
        let arg = base - usize::from(n_args);
        self.ram[2] = arg as i16; // ARG
        self.ram[1] = (base + 5) as i16; // LCL
        self.ram[0] = (base + 5) as i16; // SP
        let info = &self.program.functions[func_id as usize];
        self.call_frames.push(FrameRecord {
            function: Some(func_id),
            file_idx: Some(info.file_idx),
            call_line: Some(info.line),
            local_base: (base + 5) as u16,
            argument_base: arg as u16,
            n_locals: info.n_locals,
            n_args,
        });
        self.pc = info.start_pc as usize;
    }

    fn resume_pending(&mut self, now_ms: u64) -> StepStatus {
        let Some(pending) = &self.os.pending else {
            return StepStatus::Stepped;
        };
        let (frame_lcl, frame_arg, return_pc) = pending.frame();

        match self.os.poll_pending(&mut *self.ram, now_ms) {
            BuiltinOutcome::Return {
                value,
                screen_touched,
            } => {
                if screen_touched {
                    self.mark_screen_dirty();
                }
                self.finish_builtin_return(frame_lcl, frame_arg, return_pc, value);
                self.steps += 1;
                StepStatus::Stepped
            }
            BuiltinOutcome::Halt => {
                self.halted = true;
                StepStatus::Halted
            }
            BuiltinOutcome::Sleep { until_ms } => StepStatus::Sleeping { until_ms },
            BuiltinOutcome::WaitKey { screen_touched } => {
                if screen_touched {
                    self.mark_screen_dirty();
                }
                StepStatus::WaitingForKey
            }
            BuiltinOutcome::Error {
                error,
                screen_touched,
            } => {
                if screen_touched {
                    self.mark_screen_dirty();
                }
                self.halted = true;
                StepStatus::Error(error)
            }
        }
    }

    #[inline]
    fn finish_builtin_return(
        &mut self,
        frame_lcl: u16,
        frame_arg: u16,
        return_pc: u32,
        value: i16,
    ) {
        let lcl = frame_lcl as usize;
        let arg = frame_arg as usize;
        // Write return value at the top of the built-in frame (matches Java's RAM trace).
        if lcl < RAM_LEN {
            self.ram[lcl] = value;
        }
        // Standard VM return teardown.
        self.ram[arg] = value;
        self.ram[0] = (frame_arg as i16).wrapping_add(1);
        self.ram[4] = self.ram[lcl - 1]; // THAT
        self.ram[3] = self.ram[lcl - 2]; // THIS
        self.ram[2] = self.ram[lcl - 3]; // ARG
        self.ram[1] = self.ram[lcl - 4]; // LCL
        self.pc = return_pc as usize;
    }

    #[inline]
    fn mark_screen_dirty(&mut self) {
        self.screen_generation = self.screen_generation.wrapping_add(1);
        self.screen_dirty = true;
    }

    #[inline]
    fn format_location(&self, pc: usize) -> String {
        if let Some(m) = self.program.meta.get(pc) {
            let func = m
                .func_id
                .and_then(|id| self.program.functions.get(id as usize))
                .map(|f| f.name.as_str())
                .unwrap_or("");
            format!("{func}.{}", m.index_in_func)
        } else {
            format!(".{pc}")
        }
    }

    #[inline]
    fn push(&mut self, val: i16, pc: usize) -> Result<(), VmError> {
        let sp = self.ram[0];
        let next = sp.wrapping_add(1);
        if !(STACK_MIN..=STACK_MAX).contains(&next) {
            return Err(VmError::StackOverflow {
                sp,
                location: self.format_location(pc),
            });
        }
        self.ram[sp as usize] = val;
        self.ram[0] = next;
        Ok(())
    }

    #[inline]
    fn pop(&mut self, pc: usize) -> Result<i16, VmError> {
        let sp = self.ram[0];
        let next = sp.wrapping_sub(1);
        if next < STACK_MIN {
            return Err(VmError::StackUnderflow {
                sp,
                location: self.format_location(pc),
            });
        }
        if next > STACK_MAX {
            return Err(VmError::StackOverflow {
                sp,
                location: self.format_location(pc),
            });
        }
        self.ram[0] = next;
        Ok(self.ram[next as usize])
    }

    #[inline]
    fn execute_one(&mut self, now_ms: u64) -> StepStatus {
        let pc = self.pc;
        let op = self.program.ops[pc];
        let meta = self.program.meta[pc];
        self.last_index_in_func = Some(meta.index_in_func);

        let res: Result<StepStatus, VmError> = (|| {
            match op {
                Op::PushConstant(val) => {
                    self.push(val, pc)?;
                    self.pc = meta.next_pc as usize;
                }
                Op::PushLocal(idx) => {
                    let addr = i32::from(self.ram[1]) + i32::from(idx);
                    if !(i32::from(STACK_MIN)..=i32::from(STACK_MAX)).contains(&addr) {
                        return Err(VmError::OutOfSegmentSpace {
                            segment: "local",
                            address: addr,
                            location: self.format_location(pc),
                        });
                    }
                    self.push(self.ram[addr as usize], pc)?;
                    self.pc = meta.next_pc as usize;
                }
                Op::PushArgument(idx) => {
                    let addr = i32::from(self.ram[2]) + i32::from(idx);
                    if !(i32::from(STACK_MIN)..=i32::from(STACK_MAX)).contains(&addr) {
                        return Err(VmError::OutOfSegmentSpace {
                            segment: "argument",
                            address: addr,
                            location: self.format_location(pc),
                        });
                    }
                    self.push(self.ram[addr as usize], pc)?;
                    self.pc = meta.next_pc as usize;
                }
                Op::PushThis(idx) => {
                    let addr = i32::from(self.ram[3]) + i32::from(idx);
                    if !(i32::from(HEAP_START)..=i32::from(HEAP_END)).contains(&addr) {
                        return Err(VmError::OutOfSegmentSpace {
                            segment: "this",
                            address: addr,
                            location: self.format_location(pc),
                        });
                    }
                    self.push(self.ram[addr as usize], pc)?;
                    self.pc = meta.next_pc as usize;
                }
                Op::PushThat(idx) => {
                    let addr = i32::from(self.ram[4]) + i32::from(idx);
                    if !(i32::from(HEAP_START)..=i32::from(KEYBOARD_ADDR)).contains(&addr) {
                        return Err(VmError::OutOfSegmentSpace {
                            segment: "that",
                            address: addr,
                            location: self.format_location(pc),
                        });
                    }
                    self.push(self.ram[addr as usize], pc)?;
                    self.pc = meta.next_pc as usize;
                }
                Op::PushTemp(idx) => {
                    if idx > 7 {
                        return Err(VmError::OutOfSegmentSpace {
                            segment: "temp",
                            address: i32::from(TEMP_BASE) + i32::from(idx),
                            location: self.format_location(pc),
                        });
                    }
                    self.push(self.ram[(TEMP_BASE + idx) as usize], pc)?;
                    self.pc = meta.next_pc as usize;
                }
                Op::PushPointer(idx) => {
                    if idx <= 1 {
                        self.push(self.ram[(3 + idx) as usize], pc)?;
                    }
                    // `pointer 2+` is silently ignored by the Java VM Emulator.
                    self.pc = meta.next_pc as usize;
                }
                Op::PushStatic(addr) => {
                    if addr > crate::program::STATIC_MAX {
                        return Err(VmError::OutOfSegmentSpace {
                            segment: "static",
                            address: i32::from(addr),
                            location: self.format_location(pc),
                        });
                    }
                    self.push(self.ram[addr as usize], pc)?;
                    self.pc = meta.next_pc as usize;
                }

                Op::PopLocal(idx) => {
                    let addr = i32::from(self.ram[1]) + i32::from(idx);
                    if !(i32::from(STACK_MIN)..=i32::from(STACK_MAX)).contains(&addr) {
                        return Err(VmError::OutOfSegmentSpace {
                            segment: "local",
                            address: addr,
                            location: self.format_location(pc),
                        });
                    }
                    let val = self.pop(pc)?;
                    self.ram[addr as usize] = val;
                    self.pc = meta.next_pc as usize;
                }
                Op::PopArgument(idx) => {
                    let addr = i32::from(self.ram[2]) + i32::from(idx);
                    if !(i32::from(STACK_MIN)..=i32::from(STACK_MAX)).contains(&addr) {
                        return Err(VmError::OutOfSegmentSpace {
                            segment: "argument",
                            address: addr,
                            location: self.format_location(pc),
                        });
                    }
                    let val = self.pop(pc)?;
                    self.ram[addr as usize] = val;
                    self.pc = meta.next_pc as usize;
                }
                Op::PopThis(idx) => {
                    let addr = i32::from(self.ram[3]) + i32::from(idx);
                    if !(i32::from(HEAP_START)..=i32::from(HEAP_END)).contains(&addr) {
                        return Err(VmError::OutOfSegmentSpace {
                            segment: "this",
                            address: addr,
                            location: self.format_location(pc),
                        });
                    }
                    let val = self.pop(pc)?;
                    self.ram[addr as usize] = val;
                    self.pc = meta.next_pc as usize;
                }
                Op::PopThat(idx) => {
                    let addr = i32::from(self.ram[4]) + i32::from(idx);
                    if !(i32::from(HEAP_START)..=i32::from(KEYBOARD_ADDR)).contains(&addr) {
                        return Err(VmError::OutOfSegmentSpace {
                            segment: "that",
                            address: addr,
                            location: self.format_location(pc),
                        });
                    }
                    let val = self.pop(pc)?;
                    let a = addr as usize;
                    self.ram[a] = val;
                    if (SCREEN_START as usize..=SCREEN_END as usize).contains(&a) {
                        self.mark_screen_dirty();
                    }
                    self.pc = meta.next_pc as usize;
                }
                Op::PopTemp(idx) => {
                    if idx > 7 {
                        return Err(VmError::OutOfSegmentSpace {
                            segment: "temp",
                            address: i32::from(TEMP_BASE) + i32::from(idx),
                            location: self.format_location(pc),
                        });
                    }
                    let val = self.pop(pc)?;
                    self.ram[(TEMP_BASE + idx) as usize] = val;
                    self.pc = meta.next_pc as usize;
                }
                Op::PopPointer(idx) => {
                    if idx == 0 {
                        let sp = self.ram[0];
                        let next = sp.wrapping_sub(1);
                        if next < STACK_MIN {
                            return Err(VmError::StackUnderflow {
                                sp,
                                location: self.format_location(pc),
                            });
                        }
                        if next > STACK_MAX {
                            return Err(VmError::StackOverflow {
                                sp,
                                location: self.format_location(pc),
                            });
                        }
                        let val = self.ram[next as usize];
                        if val > 0 && !(HEAP_START as i16..=HEAP_END as i16).contains(&val) {
                            return Err(VmError::IllegalPointer {
                                pointer: 0,
                                value: val,
                                location: self.format_location(pc),
                            });
                        }
                        self.ram[0] = next;
                        self.ram[3] = val;
                    } else if idx == 1 {
                        let sp = self.ram[0];
                        let next = sp.wrapping_sub(1);
                        if next < STACK_MIN {
                            return Err(VmError::StackUnderflow {
                                sp,
                                location: self.format_location(pc),
                            });
                        }
                        if next > STACK_MAX {
                            return Err(VmError::StackOverflow {
                                sp,
                                location: self.format_location(pc),
                            });
                        }
                        let val = self.ram[next as usize];
                        if val != 0 && !(HEAP_START as i16..=KEYBOARD_ADDR as i16).contains(&val) {
                            return Err(VmError::IllegalPointer {
                                pointer: 1,
                                value: val,
                                location: self.format_location(pc),
                            });
                        }
                        self.ram[0] = next;
                        self.ram[4] = val;
                    }
                    // `pointer 2+` is silently ignored without popping (matches Java).
                    self.pc = meta.next_pc as usize;
                }
                Op::PopStatic(addr) => {
                    if addr > crate::program::STATIC_MAX {
                        return Err(VmError::OutOfSegmentSpace {
                            segment: "static",
                            address: i32::from(addr),
                            location: self.format_location(pc),
                        });
                    }
                    let val = self.pop(pc)?;
                    self.ram[addr as usize] = val;
                    self.pc = meta.next_pc as usize;
                }

                // ---- Arithmetic & Logic ----
                Op::Add | Op::Sub | Op::Eq | Op::Gt | Op::Lt | Op::And | Op::Or => {
                    let sp = self.ram[0];
                    if sp < STACK_MIN + 2 {
                        return Err(VmError::StackUnderflow {
                            sp,
                            location: self.format_location(pc),
                        });
                    }
                    if sp > STACK_MAX + 1 {
                        return Err(VmError::StackOverflow {
                            sp,
                            location: self.format_location(pc),
                        });
                    }
                    let top = (sp - 1) as usize;
                    let x = self.ram[top - 1];
                    let y = self.ram[top];
                    let out = match op {
                        Op::Add => x.wrapping_add(y),
                        Op::Sub => x.wrapping_sub(y),
                        Op::Eq => {
                            if x == y {
                                -1
                            } else {
                                0
                            }
                        }
                        Op::Gt => {
                            if x > y {
                                -1
                            } else {
                                0
                            }
                        }
                        Op::Lt => {
                            if x < y {
                                -1
                            } else {
                                0
                            }
                        }
                        Op::And => x & y,
                        Op::Or => x | y,
                        _ => unreachable!(),
                    };
                    self.ram[top - 1] = out;
                    self.ram[0] = sp - 1;
                    self.pc = meta.next_pc as usize;
                }
                Op::Neg | Op::Not => {
                    let sp = self.ram[0];
                    if sp < STACK_MIN + 1 {
                        return Err(VmError::StackUnderflow {
                            sp,
                            location: self.format_location(pc),
                        });
                    }
                    if sp > STACK_MAX + 1 {
                        return Err(VmError::StackOverflow {
                            sp,
                            location: self.format_location(pc),
                        });
                    }
                    let top = (sp - 1) as usize;
                    self.ram[top] = match op {
                        Op::Neg => self.ram[top].wrapping_neg(),
                        Op::Not => !self.ram[top],
                        _ => unreachable!(),
                    };
                    self.pc = meta.next_pc as usize;
                }

                // ---- Control Flow ----
                Op::Label => {
                    self.pc = meta.next_pc as usize;
                }
                Op::Goto(target) => {
                    if target == u32::MAX {
                        return Err(VmError::UnresolvedTarget {
                            kind: "goto",
                            location: self.format_location(pc),
                        });
                    }
                    self.pc = target as usize;
                }
                Op::IfGoto(target) => {
                    if target == u32::MAX {
                        return Err(VmError::UnresolvedTarget {
                            kind: "if-goto",
                            location: self.format_location(pc),
                        });
                    }
                    let cond = self.pop(pc)?;
                    if cond != 0 {
                        self.pc = target as usize;
                    } else {
                        self.pc = meta.next_pc as usize;
                    }
                }

                // ---- Functions ----
                Op::Function { func_id, n_locals } => {
                    let info = &self.program.functions[func_id as usize];
                    if let Some(msg) = &info.invalid_name_error {
                        return Err(VmError::InvalidFunctionName {
                            message: msg.clone(),
                            location: self.format_location(pc),
                        });
                    }
                    if n_locals > 0 {
                        let sp = i32::from(self.ram[0]);
                        let new_sp = sp + i32::from(n_locals);
                        if sp < i32::from(STACK_MIN) - 1 || new_sp > i32::from(STACK_MAX) {
                            return Err(VmError::StackOverflow {
                                sp: self.ram[0],
                                location: self.format_location(pc),
                            });
                        }
                        let start = sp as usize;
                        let end = new_sp as usize;
                        self.ram[start..end].fill(0);
                        self.ram[0] = new_sp as i16;
                    }
                    self.current_function.clear();
                    self.current_function.push_str(&info.name);
                    // Ensure call_frames has an entry when entering a function directly
                    // (e.g., Sys.init or SimpleFunction without an explicit `call`).
                    if self.call_frames.is_empty() {
                        self.call_frames.push(FrameRecord {
                            function: Some(func_id),
                            file_idx: Some(info.file_idx),
                            call_line: Some(info.line),
                            local_base: self.ram[1].max(0) as u16,
                            argument_base: self.ram[2].max(0) as u16,
                            n_locals,
                            n_args: 0,
                        });
                    } else if let Some(top) = self.call_frames.last_mut() {
                        top.n_locals = n_locals;
                    }
                    self.pc = meta.next_pc as usize;
                }
                Op::Call {
                    target,
                    n_args,
                    return_pc,
                } => {
                    if matches!(target, CallTarget::Unresolved) {
                        return Err(VmError::UnresolvedTarget {
                            kind: "call",
                            location: self.format_location(pc),
                        });
                    }
                    let sp = i32::from(self.ram[0]);
                    let new_sp = sp + 5;
                    if sp < i32::from(STACK_MIN) - 1 || new_sp > i32::from(STACK_MAX) {
                        return Err(VmError::StackOverflow {
                            sp: self.ram[0],
                            location: self.format_location(pc),
                        });
                    }
                    let new_arg = sp - i32::from(n_args);
                    if new_arg < 0 || new_arg >= RAM_LEN as i32 {
                        return Err(VmError::StackUnderflow {
                            sp: self.ram[0],
                            location: self.format_location(pc),
                        });
                    }
                    let base = sp as usize;
                    self.ram[base] = return_pc as i16;
                    self.ram[base + 1] = self.ram[1]; // saved LCL
                    self.ram[base + 2] = self.ram[2]; // saved ARG
                    self.ram[base + 3] = self.ram[3]; // saved THIS
                    self.ram[base + 4] = self.ram[4]; // saved THAT
                    self.ram[2] = new_arg as i16; // ARG
                    self.ram[1] = new_sp as i16; // LCL
                    self.ram[0] = new_sp as i16; // SP

                    match target {
                        CallTarget::User {
                            pc: target_pc,
                            func_id,
                        } => {
                            let info = &self.program.functions[func_id as usize];
                            if let Some(caller) = self.call_frames.last_mut() {
                                caller.call_line = Some(meta.line);
                            }
                            if self.call_frames.len() >= MAX_SHADOW_FRAMES {
                                self.call_frames.drain(..MAX_SHADOW_FRAMES / 2);
                            }
                            self.call_frames.push(FrameRecord {
                                function: Some(func_id),
                                file_idx: Some(info.file_idx),
                                call_line: Some(info.line),
                                local_base: new_sp as u16,
                                argument_base: new_arg as u16,
                                n_locals: info.n_locals,
                                n_args,
                            });
                            self.pc = target_pc as usize;
                        }
                        CallTarget::Builtin(builtin) => {
                            let mut args_buf = [0i16; 8];
                            let count = usize::from(n_args).min(args_buf.len());
                            for (i, slot) in args_buf[..count].iter_mut().enumerate() {
                                let a = new_arg as usize + i;
                                if a < RAM_LEN {
                                    *slot = self.ram[a];
                                }
                            }
                            if let Some((func_id, value, then)) =
                                self.user_memory_target(builtin, args_buf[0])
                            {
                                // The built-in passes its argument at its own LCL and
                                // calls the program's function, as Java does.
                                let lcl = new_sp as usize;
                                if lcl + 6 > STACK_MAX as usize {
                                    return Err(VmError::StackOverflow {
                                        sp: self.ram[0],
                                        location: self.format_location(pc),
                                    });
                                }
                                self.ram[lcl] = value;
                                self.ram[0] = (lcl + 1) as i16;
                                self.builtin_waits.push(BuiltinWait {
                                    then,
                                    frame_lcl: new_sp as u16,
                                    frame_arg: new_arg as u16,
                                    return_pc,
                                });
                                self.call_from_builtin(func_id, 1);
                                self.steps += 1;
                                return Ok(StepStatus::Stepped);
                            }
                            let outcome = self.os.call(
                                builtin,
                                &args_buf[..count],
                                &mut *self.ram,
                                now_ms,
                                new_sp as u16,
                                new_arg as u16,
                                return_pc,
                            );
                            match outcome {
                                BuiltinOutcome::Return {
                                    value,
                                    screen_touched,
                                } => {
                                    if screen_touched {
                                        self.mark_screen_dirty();
                                    }
                                    self.finish_builtin_return(
                                        new_sp as u16,
                                        new_arg as u16,
                                        return_pc,
                                        value,
                                    );
                                }
                                BuiltinOutcome::Halt => {
                                    self.halted = true;
                                    self.steps += 1;
                                    return Ok(StepStatus::Halted);
                                }
                                BuiltinOutcome::Sleep { until_ms } => {
                                    return Ok(StepStatus::Sleeping { until_ms });
                                }
                                BuiltinOutcome::WaitKey { screen_touched } => {
                                    if screen_touched {
                                        self.mark_screen_dirty();
                                    }
                                    return Ok(StepStatus::WaitingForKey);
                                }
                                BuiltinOutcome::Error {
                                    error,
                                    screen_touched,
                                } => {
                                    if screen_touched {
                                        self.mark_screen_dirty();
                                    }
                                    self.halted = true;
                                    return Err(error);
                                }
                            }
                        }
                        CallTarget::Unresolved => unreachable!(),
                    }
                }
                Op::Return => {
                    let lcl = i32::from(self.ram[1]);
                    if lcl < 5 || lcl >= RAM_LEN as i32 {
                        return Err(VmError::IllegalReturnAddress {
                            address: self.ram[1],
                            location: self.format_location(pc),
                        });
                    }
                    let lcl_u = lcl as usize;
                    let ret_addr = self.ram[lcl_u - 5];
                    let saved_lcl = self.ram[lcl_u - 4];
                    let saved_arg = self.ram[lcl_u - 3];
                    let saved_this = self.ram[lcl_u - 2];
                    let saved_that = self.ram[lcl_u - 1];

                    let retval = self.pop(pc)?;
                    let arg = i32::from(self.ram[2]);
                    if arg < 0 || arg >= RAM_LEN as i32 {
                        return Err(VmError::OutOfSegmentSpace {
                            segment: "argument",
                            address: arg,
                            location: self.format_location(pc),
                        });
                    }
                    self.set_ram(arg as u16, retval);
                    self.ram[4] = saved_that;
                    self.ram[3] = saved_this;
                    self.ram[2] = saved_arg;
                    self.ram[1] = saved_lcl;

                    self.call_frames.pop();

                    if ret_addr == BUILTIN_SYS_RETURN_ADDR
                        && let Some(wait) = self.builtin_waits.pop()
                    {
                        // Returned to a built-in OS function: it finishes and
                        // returns to its caller in the same step.
                        let value = match wait.then {
                            AfterCall::ReturnValue => retval,
                            AfterCall::ReturnZero => 0,
                            AfterCall::NewString(capacity) => {
                                let base = i32::from(retval);
                                write_word(&mut *self.ram, base, capacity, "String.new")?;
                                write_word(&mut *self.ram, base + 1, 0, "String.new")?;
                                retval
                            }
                        };
                        self.finish_builtin_return(
                            wait.frame_lcl,
                            wait.frame_arg,
                            wait.return_pc,
                            value,
                        );
                        self.current_function.clear();
                        if let Some(caller) = self.program.meta.get(self.pc).and_then(|m| m.func_id)
                        {
                            self.current_function
                                .push_str(&self.program.functions[caller as usize].name);
                        }
                        self.steps += 1;
                        return Ok(StepStatus::Stepped);
                    }

                    if ret_addr == BUILTIN_SYS_RETURN_ADDR {
                        // Returned to the built-in `Sys.init`: it drops the
                        // value and, in the same step, calls the next function
                        // of its sequence, or halts after `Main.main`.
                        self.ram[0] = arg as i16;
                        let more = self.boot_next <= self.program.boot_inits.len()
                            && matches!(self.program.entry, EntryPoint::BuiltinSysInit { .. });
                        if more && !(i32::from(STACK_MIN)..=i32::from(STACK_MAX) - 5).contains(&arg)
                        {
                            return Err(VmError::StackOverflow {
                                sp: self.ram[0],
                                location: self.format_location(pc),
                            });
                        }
                        if self.boot_call_next() {
                            self.current_function.clear();
                            self.current_function.push_str(BUILTIN_SYS_INIT_FRAME);
                            self.steps += 1;
                            return Ok(StepStatus::Stepped);
                        }
                        self.current_function.clear();
                        self.current_function.push_str(BUILTIN_SYS_INIT_FRAME);
                        self.halted = true;
                        self.steps += 1;
                        return Ok(StepStatus::Halted);
                    }

                    if ret_addr < 0 || (ret_addr as usize) >= self.program.ops.len() {
                        return Err(VmError::IllegalReturnAddress {
                            address: ret_addr,
                            location: self.format_location(pc),
                        });
                    }

                    self.ram[0] = (arg + 1) as i16;
                    self.pc = ret_addr as usize;
                    // Update `currentFunction` immediately to the caller (matches Java).
                    if let Some(caller_func) = self.program.meta[self.pc].func_id {
                        self.current_function.clear();
                        self.current_function
                            .push_str(&self.program.functions[caller_func as usize].name);
                    } else {
                        self.current_function.clear();
                    }
                }
            }
            self.steps += 1;
            Ok(StepStatus::Stepped)
        })();

        match res {
            Ok(status) => status,
            Err(e) => {
                self.halted = true;
                StepStatus::Error(e)
            }
        }
    }

    // ---- State inspection & mutation APIs ----

    pub fn program(&self) -> &Program {
        &self.program
    }

    pub fn pc(&self) -> usize {
        self.pc
    }

    pub fn steps(&self) -> u64 {
        self.steps
    }

    pub fn is_halted(&self) -> bool {
        self.halted
    }

    #[inline]
    pub fn ram(&self, addr: u16) -> i16 {
        self.ram.get(addr as usize).copied().unwrap_or(0)
    }

    #[inline]
    pub fn set_ram(&mut self, addr: u16, value: i16) {
        let a = addr as usize;
        if a < RAM_LEN {
            self.ram[a] = value;
            if (SCREEN_START as usize..=SCREEN_END as usize).contains(&a) {
                self.mark_screen_dirty();
            }
        }
    }

    pub fn ram_slice(&self) -> &[i16] {
        &self.ram[..]
    }

    /// The 8192-word screen memory (`RAM[16384..24576]`).
    pub fn screen_slice(&self) -> &[i16] {
        &self.ram[SCREEN_START as usize..=SCREEN_END as usize]
    }

    /// Monotonically incremented whenever any screen word is modified.
    pub fn screen_generation(&self) -> u64 {
        self.screen_generation
    }

    pub fn screen_dirty(&self) -> bool {
        self.screen_dirty
    }

    pub fn clear_screen_dirty(&mut self) {
        self.screen_dirty = false;
    }

    /// Sets the keyboard register `RAM[24576]` (`0` = no key pressed).
    pub fn set_key(&mut self, code: u16) {
        self.ram[KEYBOARD_ADDR as usize] = code as i16;
    }

    pub fn key(&self) -> u16 {
        self.ram[KEYBOARD_ADDR as usize] as u16
    }

    pub fn sp(&self) -> i16 {
        self.ram[0]
    }

    pub fn local(&self) -> i16 {
        self.ram[1]
    }

    pub fn argument(&self) -> i16 {
        self.ram[2]
    }

    pub fn this(&self) -> i16 {
        self.ram[3]
    }

    pub fn that(&self) -> i16 {
        self.ram[4]
    }

    /// Active portion of the working stack (`RAM[256..SP]`).
    pub fn stack(&self) -> &[i16] {
        let sp = self.ram[0];
        if (STACK_MIN..=STACK_MAX + 1).contains(&sp) {
            &self.ram[STACK_MIN as usize..sp as usize]
        } else {
            &[]
        }
    }

    pub fn temp_slice(&self) -> &[i16] {
        &self.ram[TEMP_BASE as usize..(TEMP_BASE + 8) as usize]
    }

    /// Returns the base RAM address of `segment` (for segments that have a single
    /// runtime base: `local`, `argument`, `this`, `that`, `temp`, `pointer`).
    pub fn segment_base(&self, segment: Segment) -> Option<u16> {
        let base: i16 = match segment {
            Segment::Local => self.ram[1],
            Segment::Argument => self.ram[2],
            Segment::This => self.ram[3],
            Segment::That => self.ram[4],
            Segment::Temp => TEMP_BASE as i16,
            Segment::Pointer => 3,
            Segment::Static => {
                let file_idx = self.current_file_index()?;
                self.program.files.get(file_idx as usize)?.static_base as i16
            }
            Segment::Constant => return None,
        };
        if base >= 0 && (base as usize) < RAM_LEN {
            Some(base as u16)
        } else {
            None
        }
    }

    /// Returns up to `len` words starting at `segment`'s base, safely clamped to RAM.
    pub fn segment_slice(&self, segment: Segment, len: usize) -> &[i16] {
        let Some(base) = self.segment_base(segment) else {
            return &[];
        };
        let start = base as usize;
        let end = start.saturating_add(len).min(RAM_LEN);
        &self.ram[start..end]
    }

    /// Returns the static segment slice allocated to `file_stem`, if any.
    pub fn static_slice(&self, file_stem: &str) -> Option<&[i16]> {
        let info = self.program.files.iter().find(|f| f.stem == file_stem)?;
        // A file after one with a huge `static` index can have its base past
        // the end of RAM (the linker reports the overflow; it must not panic here).
        let start = (info.static_base as usize).min(RAM_LEN);
        let end = start
            .saturating_add(info.static_count as usize)
            .min(RAM_LEN);
        Some(&self.ram[start..end])
    }

    pub fn current_function(&self) -> &str {
        &self.current_function
    }

    fn current_file_index(&self) -> Option<u32> {
        self.program.meta.get(self.pc).map(|m| m.file_idx)
    }

    pub fn current_file(&self) -> Option<&str> {
        let idx = self.current_file_index()?;
        self.program
            .files
            .get(idx as usize)
            .map(|f| f.stem.as_str())
    }

    /// Zero-based source line of the next instruction to execute.
    pub fn current_line(&self) -> Option<u32> {
        self.program.meta.get(self.pc).map(|m| m.line)
    }

    /// Current cursor position `(row, col)` of the built-in `Output` class.
    pub fn output_cursor(&self) -> (u8, u8) {
        (self.os.output.row, self.os.output.col)
    }

    /// Snapshot of the current call stack from bottom (oldest caller) to top (active frame).
    /// A suspended built-in is represented by the VM frame and source line
    /// that called it.
    pub fn call_stack(&self) -> Vec<CallFrame> {
        let last_idx = self.call_frames.len().saturating_sub(1);
        let (active_lcl, active_arg) =
            self.os
                .pending
                .as_ref()
                .map_or((self.ram[1], self.ram[2]), |pending| {
                    let (builtin_lcl, _, _) = pending.frame();
                    let lcl = builtin_lcl as usize;
                    (self.ram[lcl - 4], self.ram[lcl - 3])
                });
        self.call_frames
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let (local_base, argument_base, line) = if i == last_idx {
                    (
                        active_lcl.max(0) as u16,
                        active_arg.max(0) as u16,
                        self.current_line().or(f.call_line),
                    )
                } else {
                    (f.local_base, f.argument_base, f.call_line)
                };
                let locals_start = (local_base as usize).min(RAM_LEN);
                let locals_end = locals_start
                    .saturating_add(f.n_locals as usize)
                    .min(RAM_LEN);
                let args_start = (argument_base as usize).min(RAM_LEN);
                let args_end = args_start.saturating_add(f.n_args as usize).min(RAM_LEN);
                CallFrame {
                    function: match f.function {
                        Some(id) => self
                            .program
                            .functions
                            .get(id as usize)
                            .map_or_else(String::new, |info| info.name.clone()),
                        None => BUILTIN_SYS_INIT_FRAME.to_string(),
                    },
                    file: f
                        .file_idx
                        .and_then(|fi| self.program.files.get(fi as usize))
                        .map(|fi| fi.stem.clone()),
                    line,
                    local_base,
                    argument_base,
                    locals: self.ram[locals_start..locals_end].to_vec(),
                    arguments: self.ram[args_start..args_end].to_vec(),
                }
            })
            .collect()
    }

    // ---- VM-emulator test-script variable namespace ----

    /// Reads an integer variable from the VM-emulator test-script namespace.
    ///
    /// Supported case-sensitive names (matching the official Java VM Emulator):
    /// - Scalars (`index == None`): `sp`, `local`, `argument`, `this`, `that`, `line`
    /// - Indexed (`index == Some(i)`): `RAM[i]`, `local[i]`, `argument[i]`,
    ///   `this[i]`, `that[i]`, `temp[i]`
    ///
    /// For text variables (`currentFunction`, `line` formatted as `<func>.<idx>`),
    /// use [`Vm::get_var_text`].
    pub fn get_var(&self, name: &str, index: Option<i64>) -> Result<i64, String> {
        match (name, index) {
            ("sp", None) => Ok(i64::from(self.ram[0])),
            ("local", None) => Ok(i64::from(self.ram[1])),
            ("argument", None) => Ok(i64::from(self.ram[2])),
            ("this", None) => Ok(i64::from(self.ram[3])),
            ("that", None) => Ok(i64::from(self.ram[4])),
            ("line", None) => Ok(i64::from(self.last_index_in_func.unwrap_or(0))),

            ("RAM", Some(i)) => self.read_script_ram(name, i, i),
            ("local", Some(i)) => {
                self.read_script_ram(name, i, i64::from(self.ram[1]).saturating_add(i))
            }
            ("argument", Some(i)) => {
                self.read_script_ram(name, i, i64::from(self.ram[2]).saturating_add(i))
            }
            ("this", Some(i)) => {
                self.read_script_ram(name, i, i64::from(self.ram[3]).saturating_add(i))
            }
            ("that", Some(i)) => {
                self.read_script_ram(name, i, i64::from(self.ram[4]).saturating_add(i))
            }
            ("temp", Some(i)) => {
                self.read_script_ram(name, i, i64::from(TEMP_BASE).saturating_add(i))
            }

            ("currentFunction", None) => {
                Err("`currentFunction` is a text variable; use `get_var_text`".to_string())
            }
            (_, Some(i)) => Err(format!("Unknown variable: {name}[{i}]")),
            (_, None) => Err(format!("Unknown variable: {name}")),
        }
    }

    /// Reads a text variable from the VM-emulator test-script namespace:
    /// `currentFunction` or `line` (`"<currentFunction>.<indexInFunction>"`).
    pub fn get_var_text(&self, name: &str) -> Result<String, String> {
        match name {
            "currentFunction" => Ok(self.current_function.clone()),
            "line" => match self.last_index_in_func {
                Some(idx) => Ok(format!("{}.{idx}", self.current_function)),
                None => Ok(format!("{}.0", self.current_function)),
            },
            _ => Err(format!("Unknown variable: {name}")),
        }
    }

    /// Writes a variable in the VM-emulator test-script namespace.
    pub fn set_var(&mut self, name: &str, index: Option<i64>, value: i64) -> Result<(), String> {
        match (name, index) {
            ("sp", None) => self.write_script_ptr(name, 0, value),
            ("local", None) => self.write_script_ptr(name, 1, value),
            ("argument", None) => self.write_script_ptr(name, 2, value),
            ("this", None) => self.write_script_ptr(name, 3, value),
            ("that", None) => self.write_script_ptr(name, 4, value),
            ("line", None) => {
                if self.os.pending.is_some() {
                    return Err("Cannot set line while a built-in call is waiting".to_string());
                }
                let Ok(pc) = usize::try_from(value) else {
                    return Err(format!("{value} is an illegal value for: line"));
                };
                if pc > self.program.ops.len() {
                    return Err(format!("{value} is an illegal value for: line"));
                }
                self.pc = pc;
                self.boot_pending = false;
                self.builtin_waits.clear();
                self.halted = false;
                Ok(())
            }
            ("currentFunction", None) => Err("Read Only variable: currentFunction".to_string()),

            ("RAM", Some(i)) => self.write_script_ram(name, i, i, value),
            ("local", Some(i)) => {
                self.write_script_ram(name, i, i64::from(self.ram[1]).saturating_add(i), value)
            }
            ("argument", Some(i)) => {
                self.write_script_ram(name, i, i64::from(self.ram[2]).saturating_add(i), value)
            }
            ("this", Some(i)) => {
                self.write_script_ram(name, i, i64::from(self.ram[3]).saturating_add(i), value)
            }
            ("that", Some(i)) => {
                self.write_script_ram(name, i, i64::from(self.ram[4]).saturating_add(i), value)
            }
            ("temp", Some(i)) => {
                self.write_script_ram(name, i, i64::from(TEMP_BASE).saturating_add(i), value)
            }

            (_, Some(i)) => Err(format!("Unknown variable: {name}[{i}]")),
            (_, None) => Err(format!("Unknown variable: {name}")),
        }
    }

    fn read_script_ram(&self, name: &str, idx: i64, addr: i64) -> Result<i64, String> {
        if idx < 0 || addr < 0 || addr >= RAM_LEN as i64 {
            return Err(format!("Illegal variable index: {name}[{idx}]"));
        }
        Ok(i64::from(self.ram[addr as usize]))
    }

    fn write_script_ram(
        &mut self,
        name: &str,
        idx: i64,
        addr: i64,
        value: i64,
    ) -> Result<(), String> {
        if idx < 0 || addr < 0 || addr >= RAM_LEN as i64 {
            return Err(format!("Illegal variable index: {name}[{idx}]"));
        }
        let a = addr as usize;
        self.ram[a] = (value as u16) as i16;
        if (SCREEN_START as usize..=SCREEN_END as usize).contains(&a) {
            self.mark_screen_dirty();
        }
        Ok(())
    }

    fn write_script_ptr(&mut self, name: &str, reg: usize, value: i64) -> Result<(), String> {
        if value < 0 || value >= RAM_LEN as i64 {
            return Err(format!("{value} is an illegal value for: {name}"));
        }
        self.ram[reg] = value as i16;
        Ok(())
    }
}
