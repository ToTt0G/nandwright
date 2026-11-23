//! Static analysis of CPU Emulator and VM Emulator scripts.
//!
//! These dialects have a fixed set of variables (registers, memory, VM
//! segments), so a script can be checked without loading the program.

use crate::diagnostic::{Diagnostic, DiagnosticCode};
use crate::source::Span;
use crate::tst::ast::*;
use crate::tst::semantic::ScriptAnalysis;

/// A variable of an emulator dialect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmulatorVariable {
    pub name: &'static str,
    /// Whether it is written `name[i]`.
    pub indexed: bool,
    /// Whether it can also be written without an index (`local` is the
    /// pointer, `local[2]` a cell).
    pub plain: bool,
    pub writable: bool,
    pub text: bool,
    pub description: &'static str,
}

const fn var(
    name: &'static str,
    indexed: bool,
    plain: bool,
    writable: bool,
    text: bool,
    description: &'static str,
) -> EmulatorVariable {
    EmulatorVariable {
        name,
        indexed,
        plain,
        writable,
        text,
        description,
    }
}

pub const CPU_VARIABLES: &[EmulatorVariable] = &[
    var("A", false, true, true, false, "the A register"),
    var("D", false, true, true, false, "the D register"),
    var("PC", false, true, true, false, "the program counter"),
    var(
        "RAM",
        true,
        false,
        true,
        false,
        "a word of data memory, `RAM[address]`",
    ),
    var(
        "ROM",
        true,
        false,
        true,
        false,
        "a word of the loaded program, `ROM[address]`",
    ),
    var(
        "time",
        false,
        true,
        false,
        true,
        "the number of `ticktock`s so far",
    ),
];

pub const VM_VARIABLES: &[EmulatorVariable] = &[
    var(
        "sp",
        false,
        true,
        true,
        false,
        "the stack pointer, `RAM[0]`",
    ),
    var(
        "local",
        true,
        true,
        true,
        false,
        "the `local` segment: its base address, or `local[i]`",
    ),
    var(
        "argument",
        true,
        true,
        true,
        false,
        "the `argument` segment: its base address, or `argument[i]`",
    ),
    var(
        "this",
        true,
        true,
        true,
        false,
        "the `this` segment: its base address, or `this[i]`",
    ),
    var(
        "that",
        true,
        true,
        true,
        false,
        "the `that` segment: its base address, or `that[i]`",
    ),
    var(
        "temp",
        true,
        false,
        true,
        false,
        "the `temp` segment, `temp[i]`",
    ),
    var(
        "RAM",
        true,
        false,
        true,
        false,
        "a word of memory, `RAM[address]`",
    ),
    var(
        "currentFunction",
        false,
        true,
        false,
        true,
        "the name of the function being executed",
    ),
    var(
        "line",
        false,
        true,
        false,
        false,
        "the index of the next command in the current function",
    ),
];

pub fn variables(dialect: Dialect) -> &'static [EmulatorVariable] {
    match dialect {
        Dialect::Cpu => CPU_VARIABLES,
        Dialect::Vm => VM_VARIABLES,
        Dialect::Hardware => &[],
    }
}

/// Looks a variable up the way the emulators do: ignoring case.
pub fn lookup(dialect: Dialect, variable: &Variable) -> Option<&'static EmulatorVariable> {
    variables(dialect)
        .iter()
        .find(|v| v.name.eq_ignore_ascii_case(&variable.name.name))
}

struct Analyzer {
    dialect: Dialect,
    has_output_file: bool,
    has_output_list: bool,
    diagnostics: Vec<Diagnostic>,
}

pub fn analyze(script: &Script, dialect: Dialect) -> ScriptAnalysis {
    let mut analyzer = Analyzer {
        dialect,
        has_output_file: false,
        has_output_list: false,
        diagnostics: Vec::new(),
    };
    for statement in &script.statements {
        match statement {
            Statement::Simple(s) => analyzer.simple(s),
            Statement::Repeat(r) => r.body.iter().for_each(|s| analyzer.simple(s)),
            Statement::While(w) => {
                for operand in [&w.condition.left, &w.condition.right] {
                    if let Operand::Variable(v) = operand {
                        analyzer.variable(v, false);
                    }
                }
                w.body.iter().for_each(|s| analyzer.simple(s));
            }
        }
    }
    ScriptAnalysis {
        diagnostics: analyzer.diagnostics,
        ..Default::default()
    }
}

impl Analyzer {
    fn error(&mut self, code: DiagnosticCode, message: impl Into<String>, span: Span) {
        self.diagnostics
            .push(Diagnostic::error(code, message, span));
    }

    fn simple(&mut self, statement: &SimpleStatement) {
        let keyword = statement.keyword.span;
        let tool = self.dialect.tool();
        match &statement.command {
            Command::Load(file) => self.load(file),
            Command::OutputFile(_) => self.has_output_file = true,
            Command::OutputList(items) => {
                if !self.has_output_file {
                    self.error(
                        DiagnosticCode::NoOutputFile,
                        "`output-list` requires an output file; add `output-file Xxx.out` first",
                        keyword,
                    );
                }
                for item in items {
                    self.variable(&item.variable, false);
                }
                self.has_output_list = true;
            }
            Command::Output => {
                if !self.has_output_list {
                    self.error(
                        DiagnosticCode::NoOutputList,
                        "`output` requires an output schema; add `output-list ...` first",
                        keyword,
                    );
                }
            }
            Command::Set { variable, .. } => self.variable(variable, true),
            Command::Breakpoint { variable, .. } => self.variable(variable, false),
            Command::TickTock if self.dialect == Dialect::Cpu => {}
            Command::VmStep if self.dialect == Dialect::Vm => {}
            Command::Eval | Command::Tick | Command::Tock | Command::TickTock | Command::VmStep => {
                let step = if self.dialect == Dialect::Cpu {
                    "ticktock"
                } else {
                    "vmstep"
                };
                self.error(
                    DiagnosticCode::UnsupportedCommand,
                    format!(
                        "`{}` is not a {tool} command; this script loads a program, so it runs in the {tool}, which steps with `{step}`",
                        statement.keyword.name
                    ),
                    keyword,
                );
            }
            Command::BuiltinMethod { target, .. } => {
                self.error(
                    DiagnosticCode::UnknownCommand,
                    format!("unknown {tool} command `{}`", target.name),
                    target.span,
                );
            }
            Command::CompareTo(_)
            | Command::Echo(_)
            | Command::ClearEcho
            | Command::ClearBreakpoints => {}
        }
    }

    fn load(&mut self, file: &FileName) {
        let extension = file.extension().map(str::to_ascii_lowercase);
        let ok = matches!(
            (self.dialect, extension.as_deref()),
            (Dialect::Cpu, Some("asm" | "hack")) | (Dialect::Vm, Some("vm") | None)
        );
        if !ok && !file.name.is_empty() {
            let expected = if self.dialect == Dialect::Cpu {
                "a `.asm` or `.hack` program"
            } else {
                "a `.vm` file or a folder of `.vm` files"
            };
            self.error(
                DiagnosticCode::InvalidFileName,
                format!(
                    "this script runs in the {}, which loads {expected}; `{}` is neither",
                    self.dialect.tool(),
                    file.name
                ),
                file.span,
            );
        }
    }

    fn variable(&mut self, variable: &Variable, write: bool) {
        let Some(known) = lookup(self.dialect, variable) else {
            let names: Vec<String> = variables(self.dialect)
                .iter()
                .map(|v| {
                    if v.indexed && !v.plain {
                        format!("`{}[i]`", v.name)
                    } else {
                        format!("`{}`", v.name)
                    }
                })
                .collect();
            self.error(
                DiagnosticCode::UnknownVariable,
                format!(
                    "the {} has no variable `{}`; it has {}",
                    self.dialect.tool(),
                    variable.name.name,
                    names.join(", ")
                ),
                variable.name.span,
            );
            return;
        };
        match &variable.index {
            Some(index) if !known.indexed => self.error(
                DiagnosticCode::InvalidIndex,
                format!("`{}` is not indexed; write `{}`", known.name, known.name),
                index.span,
            ),
            Some(index) if index.value.is_none() => self.error(
                DiagnosticCode::InvalidIndex,
                format!(
                    "`{}[]` needs an index, e.g. `{}[0]`",
                    known.name, known.name
                ),
                index.span,
            ),
            None if !known.plain => self.error(
                DiagnosticCode::InvalidIndex,
                format!("`{}` needs an index, e.g. `{}[0]`", known.name, known.name),
                variable.span,
            ),
            _ => {}
        }
        if write && !known.writable {
            self.error(
                DiagnosticCode::ReadOnlyVariable,
                format!("`{}` is read-only", known.name),
                variable.span,
            );
        }
    }
}
