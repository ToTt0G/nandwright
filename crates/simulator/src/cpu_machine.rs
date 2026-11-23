//! The CPU Emulator: a Hack computer running a program.

use std::any::Any;

use n2t_core::host::path;
use n2t_core::source::LineIndex;
use n2t_core::tst::ast::*;
use n2t_hack::Cpu;

use crate::format::Value;
use crate::machine::{Files, Machine, Observed, Role};
use crate::runner::{RunError, error};

/// The last data-memory address: the keyboard register.
const LAST_RAM: i64 = 24576;
const LAST_ROM: i64 = 32767;

pub struct CpuMachine {
    cpu: Box<Cpu>,
    /// The program's source, when it was loaded from a `.asm` file.
    program: Option<LoadedProgram>,
    notes: Vec<String>,
}

/// A loaded program and where each instruction came from.
pub struct LoadedProgram {
    /// Engine-form path of the `.asm` or `.hack` file.
    pub file: String,
    /// For each ROM address, the zero-based source line of its instruction
    /// (for a `.hack` file, the line of the word).
    pub lines: Vec<u32>,
}

impl Default for CpuMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl CpuMachine {
    pub fn new() -> Self {
        Self {
            cpu: Box::new(Cpu::new()),
            program: None,
            notes: Vec::new(),
        }
    }

    pub fn cpu(&self) -> &Cpu {
        &self.cpu
    }

    pub fn cpu_mut(&mut self) -> &mut Cpu {
        &mut self.cpu
    }

    pub fn program(&self) -> Option<&LoadedProgram> {
        self.program.as_ref()
    }

    /// Loads a `.asm` or `.hack` file. The error is a message for the user.
    pub fn load_file(&mut self, files: &Files<'_>, file: &str) -> Result<(), String> {
        let name = path::file_name(file);
        let is_hack = path::has_extension(file, "hack");
        // The assembly a `.hack` file was (presumably) made from.
        let sibling = path::join(path::dir(file), &format!("{}.asm", path::stem(file)));
        let assembly_source = if is_hack {
            n2t_core::host::read_text(files.host.as_ref(), &sibling).ok()
        } else {
            None
        };
        let text = n2t_core::host::read_text(files.host.as_ref(), file)
            .map_err(|e| match (&e, &assembly_source) {
                // The official emulator needs the `.hack` file too.
                (n2t_core::host::HostError::NotFound, Some(_)) => format!(
                    "`{name}` does not exist yet. Assemble `{}` first (\"Assemble to .hack File\"), as you would with the official Assembler.",
                    path::file_name(&sibling)
                ),
                _ => format!("cannot read `{name}`: {e}"),
            })?
            .text;
        let (words, lines) = if path::has_extension(file, "asm") {
            let assembly = n2t_hack::assemble(&text);
            if let Some(d) = assembly.diagnostics.iter().find(|d| d.is_error()) {
                let index = LineIndex::new(&text);
                let at = d.span.map_or(String::new(), |span| {
                    let (line, column) = index.line_col_one_based(&text, span.start);
                    format!(":{line}:{column}")
                });
                return Err(format!(
                    "`{name}` does not assemble:\n  {name}{at}: {}",
                    d.message
                ));
            }
            let index = LineIndex::new(&text);
            let lines = assembly
                .source_map
                .iter()
                .map(|span| index.position(span.start).line)
                .collect();
            (assembly.words, lines)
        } else {
            let words = n2t_hack::parse_hack(&text).map_err(|diagnostics| {
                let detail = diagnostics.first().map_or_else(
                    || "it is not a Hack program".to_string(),
                    |d| d.message.clone(),
                );
                format!("`{name}`: {detail}")
            })?;
            let lines = text
                .lines()
                .enumerate()
                .filter(|(_, line)| !line.trim().is_empty())
                .map(|(number, _)| number as u32)
                .collect();
            if let Some(source) = &assembly_source {
                let assembly = n2t_hack::assemble(&source.text);
                if !assembly.diagnostics.iter().any(|d| d.is_error()) && assembly.words != words {
                    self.notes.push(format!(
                        "`{name}` is not what `{}` assembles to. If you changed the assembly, assemble it again: the test runs the `.hack` file.",
                        path::file_name(&sibling)
                    ));
                }
            }
            (words, lines)
        };
        self.cpu.load_rom(&words);
        self.program = Some(LoadedProgram {
            file: file.to_string(),
            lines,
        });
        Ok(())
    }

    fn index(variable: &Variable, last: i64) -> Result<u16, RunError> {
        let name = &variable.name.name;
        match variable.index.and_then(|i| i.value) {
            Some(i) if (i as i64) <= last => Ok(i as u16),
            Some(i) => Err(error(
                format!("`{name}[{i}]` is out of range; the last address is {last}"),
                variable.span,
            )),
            None => Err(error(
                format!("`{name}` needs an address, e.g. `{name}[0]`"),
                variable.span,
            )),
        }
    }
}

fn word(value: u16) -> Value {
    Value::Int {
        bits: u64::from(value),
        width: 16,
    }
}

fn unknown(variable: &Variable) -> RunError {
    error(
        format!(
            "the CPU Emulator has no variable `{}`; it has `A`, `D`, `PC`, `RAM[i]` and `ROM[i]`",
            variable.name.name
        ),
        variable.name.span,
    )
}

impl Machine for CpuMachine {
    fn dialect(&self) -> Dialect {
        Dialect::Cpu
    }

    fn load(&mut self, files: &Files<'_>, file: &FileName) -> Result<(), RunError> {
        let extension = file.extension().map(str::to_ascii_lowercase);
        if !matches!(extension.as_deref(), Some("asm" | "hack")) {
            return Err(error(
                "the CPU Emulator loads programs: `load` needs a `.asm` or `.hack` file",
                file.span,
            ));
        }
        let target = path::join(files.dir, &file.name);
        self.load_file(files, &target)
            .map_err(|message| error(message, file.span))
    }

    fn execute(&mut self, _files: &Files<'_>, statement: &SimpleStatement) -> Result<(), RunError> {
        match &statement.command {
            Command::TickTock => self
                .cpu
                .step()
                .map_err(|e| error(e.to_string(), statement.command_span)),
            other => Err(error(
                format!(
                    "`{}` is not a CPU Emulator command; a program is stepped with `ticktock`",
                    other.name()
                ),
                statement.keyword.span,
            )),
        }
    }

    fn read(&self, variable: &Variable) -> Result<Value, RunError> {
        let name = variable.name.name.as_str();
        if name.eq_ignore_ascii_case("RAM") {
            return Ok(word(self.cpu.ram(Self::index(variable, LAST_RAM)?)));
        }
        if name.eq_ignore_ascii_case("ROM") {
            return Ok(word(self.cpu.rom(Self::index(variable, LAST_ROM)?)));
        }
        if variable.index.is_some() {
            return Err(unknown(variable));
        }
        if name.eq_ignore_ascii_case("A") {
            Ok(word(self.cpu.a()))
        } else if name.eq_ignore_ascii_case("D") {
            Ok(word(self.cpu.d()))
        } else if name.eq_ignore_ascii_case("PC") {
            Ok(word(self.cpu.pc()))
        } else if name.eq_ignore_ascii_case("time") {
            Ok(Value::Text(self.cpu.cycles().to_string()))
        } else {
            Err(unknown(variable))
        }
    }

    fn write(&mut self, variable: &Variable, value: &IntegerLiteral) -> Result<(), RunError> {
        let name = variable.name.name.as_str();
        let number = value.constant_value().map_err(|m| error(m, value.span))?;
        let check = |low: i64, high: i64| -> Result<u16, RunError> {
            if (low..=high).contains(&number) {
                Ok(number as u16)
            } else {
                Err(error(
                    format!("{number} is not a legal value for `{variable}` ({low} to {high})"),
                    value.span,
                ))
            }
        };
        if name.eq_ignore_ascii_case("RAM") {
            let address = Self::index(variable, LAST_RAM)?;
            let word = check(-32768, 32767)?;
            self.cpu.set_ram(address, word);
            return Ok(());
        }
        if name.eq_ignore_ascii_case("ROM") {
            let address = Self::index(variable, LAST_ROM)?;
            let word = check(-32768, 32767)?;
            self.cpu.set_rom(address, word);
            return Ok(());
        }
        if variable.index.is_some() {
            return Err(unknown(variable));
        }
        if name.eq_ignore_ascii_case("A") {
            // The official emulator only lets a script point A at memory.
            self.cpu.set_a(check(0, LAST_RAM)?);
        } else if name.eq_ignore_ascii_case("D") {
            self.cpu.set_d(check(-32768, 32767)?);
        } else if name.eq_ignore_ascii_case("PC") {
            self.cpu.set_pc(check(0, LAST_ROM)?);
        } else if name.eq_ignore_ascii_case("time") {
            return Err(error("`time` is read-only", variable.span));
        } else {
            return Err(unknown(variable));
        }
        Ok(())
    }

    fn observe(&self) -> Vec<Observed> {
        let register = |name: &str, value: u16| Observed {
            name: name.to_string(),
            role: Role::Register,
            width: 16,
            bits: u64::from(value),
        };
        let mut values = vec![
            register("A", self.cpu.a()),
            register("D", self.cpu.d()),
            register("PC", self.cpu.pc()),
        ];
        for address in 0..16u16 {
            values.push(Observed {
                name: format!("RAM[{address}]"),
                role: Role::Memory,
                width: 16,
                bits: u64::from(self.cpu.ram(address)),
            });
        }
        values
    }

    fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }

    fn set_key(&mut self, code: u16) -> bool {
        self.cpu.set_key(code);
        true
    }

    fn reads_keyboard(&self, variable: &Variable) -> bool {
        crate::machine::is_keyboard_cell(variable)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
