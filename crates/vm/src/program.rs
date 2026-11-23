//! Linked VM program ready for fast execution.
//!
//! [`Program::link`] flattens one or more parsed `.vm` files into a single
//! instruction stream with all jump targets (`goto`/`if-goto`), call targets
//! (user functions and built-in Jack OS routines), and per-file `static`
//! segment offsets resolved up front. The hot execution loop performs zero
//! string lookups and zero heap allocations.

use std::collections::HashMap;

use n2t_core::diagnostic::{Diagnostic, DiagnosticCode, Location};
use n2t_core::source::Span;

use crate::parse::{ArithOp, CommandKind, ParsedVm, Segment};

/// The first RAM address reserved for `static` variables.
pub const STATIC_BASE: u16 = 16;
/// The last RAM address usable by `static` variables (`RAM[16..=254]`, 239 slots,
/// matching the official Java VM Emulator's `Out of segment space` boundary).
pub const STATIC_MAX: u16 = 254;

/// A built-in Jack OS function implemented natively by the emulator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinOs {
    // Math
    MathInit,
    MathAbs,
    MathMultiply,
    MathDivide,
    MathMin,
    MathMax,
    MathSqrt,
    // String
    StringNew,
    StringDispose,
    StringLength,
    StringCharAt,
    StringSetCharAt,
    StringAppendChar,
    StringEraseLastChar,
    StringIntValue,
    StringSetInt,
    StringBackSpace,
    StringDoubleQuote,
    StringNewLine,
    // Array
    ArrayNew,
    ArrayDispose,
    // Output
    OutputInit,
    OutputMoveCursor,
    OutputPrintChar,
    OutputPrintString,
    OutputPrintInt,
    OutputPrintln,
    OutputBackSpace,
    // Screen
    ScreenInit,
    ScreenClearScreen,
    ScreenSetColor,
    ScreenDrawPixel,
    ScreenDrawLine,
    ScreenDrawRectangle,
    ScreenDrawCircle,
    // Keyboard
    KeyboardInit,
    KeyboardKeyPressed,
    KeyboardReadChar,
    KeyboardReadLine,
    KeyboardReadInt,
    // Memory
    MemoryInit,
    MemoryPeek,
    MemoryPoke,
    MemoryAlloc,
    MemoryDeAlloc,
    // Sys
    SysInit,
    SysHalt,
    SysError,
    SysWait,
}

impl BuiltinOs {
    /// Looks up a built-in Jack OS function by its fully-qualified VM name.
    pub fn from_name(name: &str) -> Option<Self> {
        use BuiltinOs::*;
        Some(match name {
            "Math.init" => MathInit,
            "Math.abs" => MathAbs,
            "Math.multiply" => MathMultiply,
            "Math.divide" => MathDivide,
            "Math.min" => MathMin,
            "Math.max" => MathMax,
            "Math.sqrt" => MathSqrt,

            "String.new" => StringNew,
            "String.dispose" => StringDispose,
            "String.length" => StringLength,
            "String.charAt" => StringCharAt,
            "String.setCharAt" => StringSetCharAt,
            "String.appendChar" => StringAppendChar,
            "String.eraseLastChar" => StringEraseLastChar,
            "String.intValue" => StringIntValue,
            "String.setInt" => StringSetInt,
            "String.backSpace" => StringBackSpace,
            "String.doubleQuote" => StringDoubleQuote,
            "String.newLine" => StringNewLine,

            "Array.new" => ArrayNew,
            "Array.dispose" => ArrayDispose,

            "Output.init" => OutputInit,
            "Output.moveCursor" => OutputMoveCursor,
            "Output.printChar" => OutputPrintChar,
            "Output.printString" => OutputPrintString,
            "Output.printInt" => OutputPrintInt,
            "Output.println" => OutputPrintln,
            "Output.backSpace" => OutputBackSpace,

            "Screen.init" => ScreenInit,
            "Screen.clearScreen" => ScreenClearScreen,
            "Screen.setColor" => ScreenSetColor,
            "Screen.drawPixel" => ScreenDrawPixel,
            "Screen.drawLine" => ScreenDrawLine,
            "Screen.drawRectangle" => ScreenDrawRectangle,
            "Screen.drawCircle" => ScreenDrawCircle,

            "Keyboard.init" => KeyboardInit,
            "Keyboard.keyPressed" => KeyboardKeyPressed,
            "Keyboard.readChar" => KeyboardReadChar,
            "Keyboard.readLine" => KeyboardReadLine,
            "Keyboard.readInt" => KeyboardReadInt,

            "Memory.init" => MemoryInit,
            "Memory.peek" => MemoryPeek,
            "Memory.poke" => MemoryPoke,
            "Memory.alloc" => MemoryAlloc,
            "Memory.deAlloc" => MemoryDeAlloc,

            "Sys.init" => SysInit,
            "Sys.halt" => SysHalt,
            "Sys.error" => SysError,
            "Sys.wait" => SysWait,

            _ => return None,
        })
    }

    pub const fn name(self) -> &'static str {
        use BuiltinOs::*;
        match self {
            MathInit => "Math.init",
            MathAbs => "Math.abs",
            MathMultiply => "Math.multiply",
            MathDivide => "Math.divide",
            MathMin => "Math.min",
            MathMax => "Math.max",
            MathSqrt => "Math.sqrt",

            StringNew => "String.new",
            StringDispose => "String.dispose",
            StringLength => "String.length",
            StringCharAt => "String.charAt",
            StringSetCharAt => "String.setCharAt",
            StringAppendChar => "String.appendChar",
            StringEraseLastChar => "String.eraseLastChar",
            StringIntValue => "String.intValue",
            StringSetInt => "String.setInt",
            StringBackSpace => "String.backSpace",
            StringDoubleQuote => "String.doubleQuote",
            StringNewLine => "String.newLine",

            ArrayNew => "Array.new",
            ArrayDispose => "Array.dispose",

            OutputInit => "Output.init",
            OutputMoveCursor => "Output.moveCursor",
            OutputPrintChar => "Output.printChar",
            OutputPrintString => "Output.printString",
            OutputPrintInt => "Output.printInt",
            OutputPrintln => "Output.println",
            OutputBackSpace => "Output.backSpace",

            ScreenInit => "Screen.init",
            ScreenClearScreen => "Screen.clearScreen",
            ScreenSetColor => "Screen.setColor",
            ScreenDrawPixel => "Screen.drawPixel",
            ScreenDrawLine => "Screen.drawLine",
            ScreenDrawRectangle => "Screen.drawRectangle",
            ScreenDrawCircle => "Screen.drawCircle",

            KeyboardInit => "Keyboard.init",
            KeyboardKeyPressed => "Keyboard.keyPressed",
            KeyboardReadChar => "Keyboard.readChar",
            KeyboardReadLine => "Keyboard.readLine",
            KeyboardReadInt => "Keyboard.readInt",

            MemoryInit => "Memory.init",
            MemoryPeek => "Memory.peek",
            MemoryPoke => "Memory.poke",
            MemoryAlloc => "Memory.alloc",
            MemoryDeAlloc => "Memory.deAlloc",

            SysInit => "Sys.init",
            SysHalt => "Sys.halt",
            SysError => "Sys.error",
            SysWait => "Sys.wait",
        }
    }

    /// Expected argument count of the Jack OS function (including `this` for methods).
    pub const fn arity(self) -> u16 {
        use BuiltinOs::*;
        match self {
            MathInit | StringBackSpace | StringDoubleQuote | StringNewLine | OutputInit
            | OutputPrintln | OutputBackSpace | ScreenInit | ScreenClearScreen | KeyboardInit
            | KeyboardKeyPressed | KeyboardReadChar | MemoryInit | SysInit | SysHalt => 0,

            MathAbs | MathSqrt | StringNew | StringDispose | StringLength | StringEraseLastChar
            | StringIntValue | ArrayNew | ArrayDispose | OutputPrintChar | OutputPrintString
            | OutputPrintInt | ScreenSetColor | KeyboardReadLine | KeyboardReadInt | MemoryPeek
            | MemoryAlloc | MemoryDeAlloc | SysError | SysWait => 1,

            MathMultiply | MathDivide | MathMin | MathMax | StringCharAt | StringAppendChar
            | StringSetInt | OutputMoveCursor | ScreenDrawPixel | MemoryPoke => 2,

            StringSetCharAt | ScreenDrawCircle => 3,

            ScreenDrawLine | ScreenDrawRectangle => 4,
        }
    }
}

/// Resolved destination of a `call` instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallTarget {
    /// A user-defined function starting at the given PC in [`Program::ops`].
    User { pc: u32, func_id: u32 },
    /// A built-in Jack OS routine.
    Builtin(BuiltinOs),
    /// Unresolved call target (link error was reported; fails if executed).
    Unresolved,
}

/// A compiled VM instruction in the flat program array.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    PushConstant(i16),
    PushLocal(u16),
    PushArgument(u16),
    PushThis(u16),
    PushThat(u16),
    PushTemp(u16),
    PushPointer(u16),
    /// Absolute RAM address for `static` (`static_base + index`), or `u16::MAX` on overflow.
    PushStatic(u16),

    PopLocal(u16),
    PopArgument(u16),
    PopThis(u16),
    PopThat(u16),
    PopTemp(u16),
    PopPointer(u16),
    /// Absolute RAM address for `static` (`static_base + index`), or `u16::MAX` on overflow.
    PopStatic(u16),

    Add,
    Sub,
    Neg,
    Eq,
    Gt,
    Lt,
    And,
    Or,
    Not,

    /// Labels occupy a PC slot in the flat array (so return addresses match the
    /// Java VM Emulator), and act as a no-op if jumped onto directly.
    Label,
    /// Jumps to `target_pc` (or `u32::MAX` if unresolved).
    Goto(u32),
    /// Pops top of stack and jumps to `target_pc` if non-zero (or `u32::MAX` if unresolved).
    IfGoto(u32),
    Function {
        func_id: u32,
        n_locals: u16,
    },
    Call {
        target: CallTarget,
        n_args: u16,
        /// Next executable PC after the `call` (skipping immediately following labels).
        return_pc: u32,
    },
    Return,
}

/// Source-level metadata for one compiled instruction, kept parallel to [`Program::ops`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpMeta {
    pub file_idx: u32,
    /// Zero-based source line in the originating file.
    pub line: u32,
    pub span: Span,
    /// Enclosing function index in [`Program::functions`], or `None` for top-level code.
    pub func_id: Option<u32>,
    /// Zero-based command index within the enclosing function, excluding `label`
    /// commands (`-1` for `label` commands themselves). Matches the Java VM
    /// Emulator's `line` script variable (`"<func>.<index_in_func>"`).
    pub index_in_func: i32,
    /// Next PC on sequential fall-through (skips any `label` instructions).
    pub next_pc: u32,
}

/// Metadata for a linked source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    pub stem: String,
    pub static_base: u16,
    pub static_count: u16,
}

/// Metadata for a user-defined function in the linked program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionInfo {
    pub name: String,
    pub file_idx: u32,
    pub start_pc: u32,
    pub n_locals: u16,
    pub line: u32,
    pub span: Span,
    /// Set if the function name lacks `Class.name` or its class prefix does not
    /// match its file stem (Java loads the file but stops when executing it).
    pub invalid_name_error: Option<String>,
}

/// How execution begins when the VM starts or resets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryPoint {
    /// Start at the user-defined `Sys.init` function at `pc`.
    UserSysInit { pc: u32 },
    /// No user `Sys.init`, but user `Main.main` exists: start via built-in
    /// `Sys.init` (which initializes the built-in OS, calls `Main.main`, and
    /// halts when `Main.main` returns).
    BuiltinSysInit { main_pc: u32, main_func_id: u32 },
    /// Script-driven snippet (e.g. Project 7/8 tests without `Sys.init` or
    /// `Main.main`): start at PC 0 (skipping any leading labels).
    StartOfProgram { pc: u32 },
}

/// A linked VM program ready to be loaded into [`crate::Vm`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub(crate) ops: Vec<Op>,
    pub(crate) meta: Vec<OpMeta>,
    pub(crate) files: Vec<FileInfo>,
    pub(crate) functions: Vec<FunctionInfo>,
    pub(crate) func_by_name: HashMap<String, u32>,
    pub(crate) entry: EntryPoint,
    pub(crate) has_builtin_calls: bool,
}

impl Program {
    /// Links one or more parsed `.vm` files into a single executable [`Program`].
    ///
    /// Resolves `goto`/`if-goto` targets within each function, resolves `call`
    /// targets across files and against the built-in Jack OS, allocates each
    /// file's `static` segment starting at `RAM[16]`, and returns any link-time
    /// diagnostics (duplicate functions, calls to unknown functions, static
    /// segment overflow).
    pub fn link(mut files: Vec<ParsedVm>) -> (Self, Vec<Diagnostic>) {
        let mut diagnostics = Vec::new();

        // Place the file defining `Sys.init` (or named `Sys`) first, matching
        // the official Java VM Emulator's load order.
        if let Some(sys_pos) = files.iter().position(|f| {
            f.commands.iter().any(|c| {
                matches!(
                    &c.kind,
                    CommandKind::Function { name, .. } if name.text == "Sys.init"
                )
            })
        }) {
            if sys_pos != 0 {
                let sys_file = files.remove(sys_pos);
                files.insert(0, sys_file);
            }
        } else if let Some(sys_pos) = files.iter().position(|f| f.file_stem == "Sys")
            && sys_pos != 0
        {
            let sys_file = files.remove(sys_pos);
            files.insert(0, sys_file);
        }

        let mut file_infos: Vec<FileInfo> = Vec::with_capacity(files.len());
        let mut functions: Vec<FunctionInfo> = Vec::new();
        let mut func_by_name: HashMap<String, u32> = HashMap::new();
        let mut next_static: u32 = STATIC_BASE as u32;

        // First pass: assign file PC bases, static bases, and collect all user functions.
        let mut file_pc_bases = Vec::with_capacity(files.len());
        let mut total_ops: usize = 0;

        for (file_idx, parsed) in files.iter().enumerate() {
            let file_idx_u32 = file_idx as u32;
            file_pc_bases.push(total_ops);

            // Compute static slots needed by this file (max static index + 1 if any).
            let max_static = parsed
                .commands
                .iter()
                .filter_map(|c| match c.kind {
                    CommandKind::Push {
                        segment: Segment::Static,
                        index,
                        ..
                    }
                    | CommandKind::Pop {
                        segment: Segment::Static,
                        index,
                        ..
                    } => Some(index),
                    _ => None,
                })
                .max();

            let static_base = next_static.min(u16::MAX as u32) as u16;
            let static_count = match max_static {
                Some(m) => m.saturating_add(1),
                None => 0,
            };
            next_static = next_static.saturating_add(static_count as u32);
            if static_count > 0
                && u32::from(static_base) + u32::from(static_count) - 1 > u32::from(STATIC_MAX)
            {
                diagnostics.push(Diagnostic::new(
                    n2t_core::Severity::Warning,
                    DiagnosticCode::Other("vm-static-overflow"),
                    format!(
                        "static variables of `{}.vm` extend past RAM[{STATIC_MAX}]; accesses above RAM[{STATIC_MAX}] will fail at runtime",
                        parsed.file_stem
                    ),
                    None,
                ));
            }

            file_infos.push(FileInfo {
                stem: parsed.file_stem.clone(),
                static_base,
                static_count,
            });

            for (cmd_idx, cmd) in parsed.commands.iter().enumerate() {
                if let CommandKind::Function { name, n_locals, .. } = &cmd.kind {
                    let start_pc = (total_ops + cmd_idx) as u32;
                    let invalid_name_error = match name.text.split_once('.') {
                        None => Some(format!(
                            "function `{}` has no class prefix (expected `Class.name`)",
                            name.text
                        )),
                        Some((class, _)) if class != parsed.file_stem => Some(format!(
                            "function `{}` is defined in `{}.vm` (expected class `{}`)",
                            name.text, parsed.file_stem, parsed.file_stem
                        )),
                        _ => None,
                    };
                    let func_id = functions.len() as u32;
                    if let Some(&prev_id) = func_by_name.get(&name.text) {
                        let prev = &functions[prev_id as usize];
                        let prev_file = &file_infos[prev.file_idx as usize].stem;
                        diagnostics.push(
                            Diagnostic::error(
                                DiagnosticCode::Other("vm-duplicate-function"),
                                format!(
                                    "function `{}` is defined more than once (in `{}.vm`)",
                                    name.text, parsed.file_stem
                                ),
                                name.span,
                            )
                            .with_related(
                                format!("first defined in `{prev_file}.vm`"),
                                Some(Location {
                                    source: Some(prev_file.clone()),
                                    span: prev.span,
                                }),
                            ),
                        );
                    } else {
                        func_by_name.insert(name.text.clone(), func_id);
                    }
                    functions.push(FunctionInfo {
                        name: name.text.clone(),
                        file_idx: file_idx_u32,
                        start_pc,
                        n_locals: *n_locals,
                        line: cmd.line,
                        span: name.span,
                        invalid_name_error,
                    });
                }
            }

            total_ops += parsed.commands.len();
        }

        // Build lookup from (file_idx, local_cmd_idx) -> global func_id for enclosing functions.
        let mut func_id_by_start_pc: HashMap<u32, u32> = HashMap::with_capacity(functions.len());
        for (id, f) in functions.iter().enumerate() {
            func_id_by_start_pc.insert(f.start_pc, id as u32);
        }

        // Second pass: emit flat ops and metadata.
        let mut ops = Vec::with_capacity(total_ops);
        let mut meta = Vec::with_capacity(total_ops);
        let mut has_builtin_calls = false;

        for (file_idx, parsed) in files.iter().enumerate() {
            let file_idx_u32 = file_idx as u32;
            let pc_base = file_pc_bases[file_idx];
            let static_base = file_infos[file_idx].static_base;

            // Collect last label PC per (enclosing_local_func, label_name) within this file.
            let mut label_targets: HashMap<(Option<u32>, &str), u32> = HashMap::new();
            for (cmd_idx, cmd) in parsed.commands.iter().enumerate() {
                if let CommandKind::Label(name) = &cmd.kind {
                    label_targets.insert(
                        (cmd.function, name.text.as_str()),
                        (pc_base + cmd_idx) as u32,
                    );
                }
            }

            // Precompute non-label index within each function (and top-level) and
            // next non-label PC for sequential fallthrough / return addresses.
            let n = parsed.commands.len();
            let mut next_non_label_in_file = vec![(pc_base + n) as u32; n];
            let mut next_exec = (pc_base + n) as u32;
            for i in (0..n).rev() {
                next_non_label_in_file[i] = next_exec;
                if !matches!(parsed.commands[i].kind, CommandKind::Label(_)) {
                    next_exec = (pc_base + i) as u32;
                }
            }

            let mut idx_in_func: i32 = 0;
            let mut prev_scope: Option<u32> = None;

            for (cmd_idx, cmd) in parsed.commands.iter().enumerate() {
                if cmd.function != prev_scope {
                    idx_in_func = 0;
                    prev_scope = cmd.function;
                }
                let global_pc = (pc_base + cmd_idx) as u32;
                let enclosing_func_id = cmd.function.and_then(|local_f| {
                    func_id_by_start_pc
                        .get(&(pc_base as u32 + local_f))
                        .copied()
                });

                let op_index_in_func = if matches!(cmd.kind, CommandKind::Label(_)) {
                    -1
                } else {
                    let cur = idx_in_func;
                    idx_in_func += 1;
                    cur
                };

                // Sequential fallthrough skips labels, even across file boundaries
                // (though normal programs end functions with `return` or a loop).
                let next_pc = next_non_label_in_file[cmd_idx];

                let op = match &cmd.kind {
                    CommandKind::Push { segment, index, .. } => match segment {
                        Segment::Constant => Op::PushConstant(*index as i16),
                        Segment::Local => Op::PushLocal(*index),
                        Segment::Argument => Op::PushArgument(*index),
                        Segment::This => Op::PushThis(*index),
                        Segment::That => Op::PushThat(*index),
                        Segment::Temp => Op::PushTemp(*index),
                        Segment::Pointer => Op::PushPointer(*index),
                        Segment::Static => {
                            let addr = u32::from(static_base) + u32::from(*index);
                            if addr <= u32::from(STATIC_MAX) {
                                Op::PushStatic(addr as u16)
                            } else {
                                Op::PushStatic(u16::MAX)
                            }
                        }
                    },
                    CommandKind::Pop { segment, index, .. } => match segment {
                        Segment::Constant => Op::PopTemp(u16::MAX), // unreachable for valid parses
                        Segment::Local => Op::PopLocal(*index),
                        Segment::Argument => Op::PopArgument(*index),
                        Segment::This => Op::PopThis(*index),
                        Segment::That => Op::PopThat(*index),
                        Segment::Temp => Op::PopTemp(*index),
                        Segment::Pointer => Op::PopPointer(*index),
                        Segment::Static => {
                            let addr = u32::from(static_base) + u32::from(*index);
                            if addr <= u32::from(STATIC_MAX) {
                                Op::PopStatic(addr as u16)
                            } else {
                                Op::PopStatic(u16::MAX)
                            }
                        }
                    },
                    CommandKind::Arithmetic(a) => match a {
                        ArithOp::Add => Op::Add,
                        ArithOp::Sub => Op::Sub,
                        ArithOp::Neg => Op::Neg,
                        ArithOp::Eq => Op::Eq,
                        ArithOp::Gt => Op::Gt,
                        ArithOp::Lt => Op::Lt,
                        ArithOp::And => Op::And,
                        ArithOp::Or => Op::Or,
                        ArithOp::Not => Op::Not,
                    },
                    CommandKind::Label(_) => Op::Label,
                    CommandKind::Goto(name) => {
                        // In Java, goto jumps to `label_pc + 1` (skipping the target
                        // label itself, though landing on a second consecutive label
                        // if two labels appear in a row).
                        let target = label_targets
                            .get(&(cmd.function, name.text.as_str()))
                            .map(|&lpc| lpc + 1)
                            .unwrap_or(u32::MAX);
                        Op::Goto(target)
                    }
                    CommandKind::IfGoto(name) => {
                        let target = label_targets
                            .get(&(cmd.function, name.text.as_str()))
                            .map(|&lpc| lpc + 1)
                            .unwrap_or(u32::MAX);
                        Op::IfGoto(target)
                    }
                    CommandKind::Function { n_locals, .. } => {
                        let func_id = func_id_by_start_pc.get(&global_pc).copied().unwrap_or(0);
                        Op::Function {
                            func_id,
                            n_locals: *n_locals,
                        }
                    }
                    CommandKind::Call { name, n_args, .. } => {
                        let target = if let Some(&func_id) = func_by_name.get(&name.text) {
                            CallTarget::User {
                                pc: functions[func_id as usize].start_pc,
                                func_id,
                            }
                        } else if let Some(builtin) = BuiltinOs::from_name(&name.text) {
                            has_builtin_calls = true;
                            CallTarget::Builtin(builtin)
                        } else {
                            diagnostics.push(
                                Diagnostic::error(
                                    DiagnosticCode::Other("vm-unknown-function"),
                                    format!(
                                        "unknown function `{}` called from `{}.vm`",
                                        name.text, parsed.file_stem
                                    ),
                                    name.span,
                                )
                                .with_related(
                                    format!("called in `{}.vm`", parsed.file_stem),
                                    Some(Location {
                                        source: Some(parsed.file_stem.clone()),
                                        span: name.span,
                                    }),
                                ),
                            );
                            CallTarget::Unresolved
                        };
                        Op::Call {
                            target,
                            n_args: *n_args,
                            return_pc: next_pc,
                        }
                    }
                    CommandKind::Return => Op::Return,
                };

                ops.push(op);
                meta.push(OpMeta {
                    file_idx: file_idx_u32,
                    line: cmd.line,
                    span: cmd.span,
                    func_id: enclosing_func_id,
                    index_in_func: op_index_in_func,
                    next_pc,
                });
            }
        }

        // Fix up cross-file sequential fallthroughs (if a file ends and the next
        // file starts with a label, skip leading labels of the next file).
        for m in &mut meta {
            let mut pc = m.next_pc as usize;
            while pc < ops.len() && matches!(ops[pc], Op::Label) {
                pc += 1;
            }
            m.next_pc = pc as u32;
        }
        for (i, op) in ops.iter_mut().enumerate() {
            if let Op::Call { return_pc, .. } = op {
                *return_pc = meta[i].next_pc;
            }
        }

        // Determine entry point.
        let entry = if let Some(&sys_id) = func_by_name.get("Sys.init") {
            EntryPoint::UserSysInit {
                pc: functions[sys_id as usize].start_pc,
            }
        } else if let Some(&main_id) = func_by_name.get("Main.main") {
            has_builtin_calls = true;
            EntryPoint::BuiltinSysInit {
                main_pc: functions[main_id as usize].start_pc,
                main_func_id: main_id,
            }
        } else {
            let mut pc = 0usize;
            while pc < ops.len() && matches!(ops[pc], Op::Label) {
                pc += 1;
            }
            EntryPoint::StartOfProgram { pc: pc as u32 }
        };

        (
            Program {
                ops,
                meta,
                files: file_infos,
                functions,
                func_by_name,
                entry,
                has_builtin_calls,
            },
            diagnostics,
        )
    }

    pub fn files(&self) -> &[FileInfo] {
        &self.files
    }

    pub fn functions(&self) -> &[FunctionInfo] {
        &self.functions
    }

    pub fn function_by_name(&self, name: &str) -> Option<&FunctionInfo> {
        let &id = self.func_by_name.get(name)?;
        self.functions.get(id as usize)
    }

    pub fn entry_point(&self) -> EntryPoint {
        self.entry
    }

    pub fn len(&self) -> usize {
        self.ops.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn has_builtin_calls(&self) -> bool {
        self.has_builtin_calls
    }

    /// Returns the file stem and 0-based source line for instruction `pc`.
    pub fn location(&self, pc: usize) -> Option<(&str, u32)> {
        let m = self.meta.get(pc)?;
        let file = self.files.get(m.file_idx as usize)?;
        Some((file.stem.as_str(), m.line))
    }

    /// Looks up a file index by its `file_stem`.
    pub fn file_index(&self, file_stem: &str) -> Option<u32> {
        self.files
            .iter()
            .position(|f| f.stem == file_stem)
            .map(|i| i as u32)
    }
}
