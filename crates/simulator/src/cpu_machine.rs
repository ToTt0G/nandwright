//! The CPU Emulator: a Hack computer running a program.

use std::any::Any;

use n2t_core::host::path;
use n2t_core::source::LineIndex;
use n2t_core::tst::ast::*;
use n2t_hack::Cpu;

use crate::format::Value;
use crate::machine::{Files, LoadedInput, Machine, Observed, Role};
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
    /// Whether this program was assembled from `.asm` in memory for a `.hack` load.
    pub built_from_source: bool,
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

    /// Finds a `.asm` file in `dir` matching `stem`, returning the actual
    /// directory entry spelling so preference checks match on case-insensitive
    /// macOS/Linux volumes too. Exact case first, then a unique case-insensitive
    /// match (the classic course distribution ships `mult.asm` beside `Mult.tst`).
    fn find_sibling_asm(files: &Files<'_>, dir: &str, stem: &str) -> Option<String> {
        if let Ok(entries) = files.host.list(dir) {
            let candidates: Vec<String> = entries
                .into_iter()
                .filter(|e| !e.is_dir && path::has_extension(&e.name, "asm"))
                .map(|e| e.name)
                .collect();
            if let Some(exact) = candidates.iter().find(|n| path::stem(n) == stem) {
                return Some(path::join(dir, exact));
            }
            let folded: Vec<&String> = candidates
                .iter()
                .filter(|n| path::stem(n).eq_ignore_ascii_case(stem))
                .collect();
            return match folded.as_slice() {
                [one] => Some(path::join(dir, one)),
                _ => None,
            };
        }
        let exact = path::join(dir, &format!("{stem}.asm"));
        files.host.read(&exact).is_ok().then_some(exact)
    }

    fn assemble_source(name: &str, text: &str) -> Result<(Vec<u16>, Vec<u32>), String> {
        let assembly = n2t_hack::assemble(text);
        if let Some(d) = assembly.diagnostics.iter().find(|d| d.is_error()) {
            let index = LineIndex::new(text);
            let at = d.span.map_or(String::new(), |span| {
                let (line, column) = index.line_col_one_based(text, span.start);
                format!(":{line}:{column}")
            });
            return Err(format!(
                "`{name}` does not assemble:\n  {name}{at}: {}",
                d.message
            ));
        }
        let index = LineIndex::new(text);
        let lines = assembly
            .source_map
            .iter()
            .map(|span| index.position(span.start).line)
            .collect();
        Ok((assembly.words, lines))
    }

    /// Loads a `.asm` or `.hack` file. The error is a message for the user.
    pub fn load_file(&mut self, files: &Files<'_>, file: &str) -> Result<(), String> {
        let name = path::file_name(file);
        let dir = path::dir(file);
        let stem = path::stem(file);
        let is_hack = path::has_extension(file, "hack");
        let is_asm = path::has_extension(file, "asm");

        if is_asm {
            let (resolved, text) = match n2t_core::host::read_text(files.host.as_ref(), file) {
                Ok(found) => (file.to_string(), found.text),
                Err(n2t_core::host::HostError::NotFound) if files.build_from_source => {
                    if let Some(alt) = Self::find_sibling_asm(files, dir, stem) {
                        let found = n2t_core::host::read_text(files.host.as_ref(), &alt)
                            .map_err(|e| format!("cannot read `{}`: {e}", path::file_name(&alt)))?;
                        (alt, found.text)
                    } else {
                        return Err(format!(
                            "cannot read `{name}`: {}",
                            n2t_core::host::HostError::NotFound
                        ));
                    }
                }
                Err(e) => return Err(format!("cannot read `{name}`: {e}")),
            };
            let (words, lines) = Self::assemble_source(path::file_name(&resolved), &text)?;
            self.cpu.load_rom(&words);
            self.program = Some(LoadedProgram {
                file: resolved,
                lines,
                built_from_source: false,
            });
            return Ok(());
        }

        // Loading a `.hack` file: find any sibling `.asm` (case-insensitively
        // so `mult.asm` is found for `load Mult.hack` on Linux too).
        let sibling = if is_hack {
            Self::find_sibling_asm(files, dir, stem)
        } else {
            None
        };
        let hack_file = n2t_core::host::read_text(files.host.as_ref(), file);

        if is_hack
            && files.build_from_source
            && let Some(asm_path) = &sibling
        {
            let hack_missing = matches!(hack_file, Err(n2t_core::host::HostError::NotFound));
            if hack_missing || files.prefers(asm_path) {
                let asm_name = path::file_name(asm_path);
                let asm_text = n2t_core::host::read_text(files.host.as_ref(), asm_path)
                    .map_err(|e| format!("cannot read `{asm_name}`: {e}"))?
                    .text;
                let (words, lines) = Self::assemble_source(asm_name, &asm_text)?;
                if let Ok(existing) = &hack_file
                    && let Ok(existing_words) = n2t_hack::parse_hack(&existing.text)
                    && existing_words != words
                {
                    self.notes.push(format!(
                        "Assembled `{asm_name}` in memory (newer than `{name}` on disk)."
                    ));
                }
                self.cpu.load_rom(&words);
                self.program = Some(LoadedProgram {
                    file: asm_path.clone(),
                    lines,
                    built_from_source: true,
                });
                return Ok(());
            }
        }

        let text = hack_file
            .map_err(|e| match (&e, &sibling) {
                // The official emulator needs the `.hack` file too.
                (n2t_core::host::HostError::NotFound, Some(asm_path)) => format!(
                    "`{name}` does not exist yet. Assemble `{}` first (\"Assemble to .hack File\"), as you would with the official Assembler.",
                    path::file_name(asm_path)
                ),
                _ => format!("cannot read `{name}`: {e}"),
            })?
            .text;
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
        if let Some(asm_path) = &sibling
            && let Ok(source) = n2t_core::host::read_text(files.host.as_ref(), asm_path)
        {
            let assembly = n2t_hack::assemble(&source.text);
            if !assembly.diagnostics.iter().any(|d| d.is_error()) && assembly.words != words {
                self.notes.push(format!(
                    "`{name}` is not what `{}` assembles to. If you changed the assembly, assemble it again: the test runs the `.hack` file.",
                    path::file_name(asm_path)
                ));
            }
        }
        self.cpu.load_rom(&words);
        self.program = Some(LoadedProgram {
            file: file.to_string(),
            lines,
            built_from_source: false,
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

    fn loaded_inputs(&self) -> Vec<LoadedInput> {
        self.program
            .iter()
            .map(|p| LoadedInput {
                path: p.file.clone(),
                built_from_source: p.built_from_source,
            })
            .collect()
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
