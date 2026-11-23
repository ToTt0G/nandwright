//! The VM Emulator: a VM program on the Hack memory model, with the built-in
//! Jack OS for whatever the program does not define.

use std::any::Any;

use n2t_core::diagnostic::Diagnostic;
use n2t_core::host::{HostError, path, read_text};
use n2t_core::source::LineIndex;
use n2t_core::tst::ast::*;
use n2t_vm::{ParsedVm, Program, StepStatus, Vm};

use crate::format::Value;
use crate::machine::{Files, Machine, Observed, Role};
use crate::runner::{RunError, error};

pub struct VmMachine {
    vm: Option<Box<Vm>>,
    /// Engine-form paths of the loaded `.vm` files, by file stem.
    files: Vec<(String, String)>,
    /// Scripts have no clock. Time moves only when the program sleeps, and
    /// then jumps straight to the moment it wakes: a test never waits.
    now_ms: u64,
}

impl Default for VmMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl VmMachine {
    pub fn new() -> Self {
        Self {
            vm: None,
            files: Vec::new(),
            now_ms: 0,
        }
    }

    pub fn vm(&self) -> Option<&Vm> {
        self.vm.as_deref()
    }

    pub fn vm_mut(&mut self) -> Option<&mut Vm> {
        self.vm.as_deref_mut()
    }

    /// The path of a loaded file, by its stem (the class name).
    pub fn file_path(&self, stem: &str) -> Option<&str> {
        self.files
            .iter()
            .find(|(s, _)| s == stem)
            .map(|(_, p)| p.as_str())
    }

    /// Loads one `.vm` file, or every `.vm` file of a folder. The error is a
    /// message for the user.
    pub fn load_path(&mut self, files: &Files<'_>, target: &str) -> Result<(), String> {
        let mut sources: Vec<(String, String)> = Vec::new();
        if path::has_extension(target, "vm") {
            sources.push((path::stem(target).to_string(), target.to_string()));
        } else {
            let entries = files.host.list(target).map_err(|e| match e {
                HostError::NotFound => {
                    format!("the folder `{}` does not exist", path::file_name(target))
                }
                other => format!(
                    "cannot read the folder `{}`: {other}",
                    path::file_name(target)
                ),
            })?;
            for entry in entries {
                if !entry.is_dir && path::has_extension(&entry.name, "vm") {
                    sources.push((
                        path::stem(&entry.name).to_string(),
                        path::join(target, &entry.name),
                    ));
                }
            }
            sources.sort();
            if sources.is_empty() {
                return Err(format!(
                    "there are no `.vm` files in `{}`; compile the Jack program or write the VM code first",
                    path::file_name(target)
                ));
            }
        }
        let mut parsed: Vec<ParsedVm> = Vec::new();
        for (stem, file) in &sources {
            let text = read_text(files.host.as_ref(), file)
                .map_err(|e| format!("cannot read `{}`: {e}", path::file_name(file)))?
                .text;
            let unit = n2t_vm::parse(stem, &text);
            if let Some(d) = unit.diagnostics.iter().find(|d| d.is_error()) {
                return Err(describe(path::file_name(file), &text, d));
            }
            parsed.push(unit);
        }
        let (program, diagnostics) = Program::link(parsed);
        if let Some(d) = diagnostics.iter().find(|d| d.is_error()) {
            return Err(format!("the program does not link: {}", d.message));
        }
        self.vm = Some(Box::new(Vm::new(program)));
        self.files = sources;
        self.now_ms = 0;
        Ok(())
    }

    /// Executes one VM command, as `vmstep` does.
    pub fn step(&mut self) -> Result<(), String> {
        let Some(vm) = self.vm.as_deref_mut() else {
            return Err("no program is loaded; use `load` first".to_string());
        };
        match vm.step(self.now_ms) {
            StepStatus::Stepped | StepStatus::Halted | StepStatus::WaitingForKey => Ok(()),
            StepStatus::Sleeping { until_ms } => {
                self.now_ms = self.now_ms.max(until_ms);
                Ok(())
            }
            StepStatus::Error(e) => Err(e.to_string()),
        }
    }

    fn loaded(&self, span: n2t_core::Span) -> Result<&Vm, RunError> {
        self.vm
            .as_deref()
            .ok_or_else(|| error("no program is loaded; use `load` first", span))
    }
}

fn describe(name: &str, text: &str, d: &Diagnostic) -> String {
    let at = d.span.map_or(String::new(), |span| {
        let (line, column) = LineIndex::new(text).line_col_one_based(text, span.start);
        format!(":{line}:{column}")
    });
    format!("`{name}` has an error:\n  {name}{at}: {}", d.message)
}

fn index_of(variable: &Variable) -> Result<Option<i64>, RunError> {
    match variable.index {
        None => Ok(None),
        Some(VariableIndex { value: Some(i), .. }) => Ok(Some(i as i64)),
        Some(VariableIndex { value: None, span }) => Err(error(
            format!(
                "`{}[]` needs an index, e.g. `{}[0]`",
                variable.name.name, variable.name.name
            ),
            span,
        )),
    }
}

impl Machine for VmMachine {
    fn dialect(&self) -> Dialect {
        Dialect::Vm
    }

    fn load(&mut self, files: &Files<'_>, file: &FileName) -> Result<(), RunError> {
        // `load` alone means the script's own folder.
        let target = if file.name.is_empty() {
            files.dir.to_string()
        } else {
            path::join(files.dir, &file.name)
        };
        self.load_path(files, &target)
            .map_err(|message| error(message, file.span))
    }

    fn execute(&mut self, _files: &Files<'_>, statement: &SimpleStatement) -> Result<(), RunError> {
        match &statement.command {
            Command::VmStep => self
                .step()
                .map_err(|message| error(message, statement.command_span)),
            other => Err(error(
                format!(
                    "`{}` is not a VM Emulator command; a program is stepped with `vmstep`",
                    other.name()
                ),
                statement.keyword.span,
            )),
        }
    }

    fn read(&self, variable: &Variable) -> Result<Value, RunError> {
        let vm = self.loaded(variable.span)?;
        let name = variable.name.name.as_str();
        let index = index_of(variable)?;
        if index.is_none() && matches!(name, "currentFunction" | "line") {
            return vm
                .get_var_text(name)
                .map(Value::Text)
                .map_err(|m| error(m, variable.span));
        }
        vm.get_var(name, index)
            .map(|value| Value::Int {
                bits: u64::from(value as i16 as u16),
                width: 16,
            })
            .map_err(|message| error(vm_message(&message, variable), variable.span))
    }

    fn write(&mut self, variable: &Variable, value: &IntegerLiteral) -> Result<(), RunError> {
        self.loaded(variable.span)?;
        let number = value.constant_value().map_err(|m| error(m, value.span))?;
        let index = index_of(variable)?;
        let name = variable.name.name.clone();
        let Some(vm) = self.vm.as_deref_mut() else {
            return Ok(());
        };
        vm.set_var(&name, index, number)
            .map_err(|message| error(vm_message(&message, variable), variable.span))
    }

    fn observe(&self) -> Vec<Observed> {
        let Some(vm) = self.vm() else {
            return Vec::new();
        };
        let register = |name: &str, value: i16| Observed {
            name: name.to_string(),
            role: Role::Register,
            width: 16,
            bits: u64::from(value as u16),
        };
        let mut values = vec![
            register("sp", vm.sp()),
            register("local", vm.local()),
            register("argument", vm.argument()),
            register("this", vm.this()),
            register("that", vm.that()),
        ];
        // The top of the stack, newest first.
        let stack = vm.stack();
        for (depth, value) in stack.iter().rev().take(6).enumerate() {
            values.push(Observed {
                name: if depth == 0 {
                    "stack top".to_string()
                } else {
                    format!("stack top-{depth}")
                },
                role: Role::Memory,
                width: 16,
                bits: u64::from(*value as u16),
            });
        }
        values
    }

    fn set_key(&mut self, code: u16) -> bool {
        match self.vm.as_deref_mut() {
            Some(vm) => {
                vm.set_key(code);
                true
            }
            None => false,
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// The emulator's own wording for a variable problem, with what to write
/// instead when the name is unknown.
fn vm_message(message: &str, variable: &Variable) -> String {
    if message.starts_with("Unknown variable") {
        format!(
            "the VM Emulator has no variable `{variable}`; it has `sp`, `local`, `argument`, `this`, `that` (also indexed, like `local[0]`), `temp[i]`, `RAM[i]`, `currentFunction` and `line`, all spelled exactly so"
        )
    } else {
        message.to_string()
    }
}
