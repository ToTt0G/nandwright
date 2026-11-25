//! The VM Emulator: a VM program on the Hack memory model, with the built-in
//! Jack OS for whatever the program does not define.

use std::any::Any;

use n2t_core::diagnostic::Diagnostic;
use n2t_core::host::{HostError, path, read_text};
use n2t_core::source::LineIndex;
use n2t_core::tst::ast::*;
use n2t_vm::{ParsedVm, Program, StepStatus, Vm};

use crate::format::Value;
use crate::machine::{Files, LoadedInput, Machine, Observed, Role};
use crate::runner::{RunError, error};

/// One class of a loaded program and where its code comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmLoadedFile {
    /// The class name.
    pub stem: String,
    /// The file to show the student: the `.vm` file, or the `.jack` file the
    /// code was compiled from.
    pub source_path: String,
    /// Whether the code was compiled from Jack in memory.
    pub built_from_source: bool,
    /// The text of each VM line, trimmed, for display.
    pub vm_lines: Vec<String>,
    /// For a compiled Jack class: the zero-based Jack line of each VM line.
    pub jack_lines: Option<Vec<u32>>,
}

impl VmLoadedFile {
    fn from_vm(stem: String, source_path: String, vm_text: &str) -> Self {
        Self {
            stem,
            source_path,
            built_from_source: false,
            vm_lines: vm_lines(vm_text),
            jack_lines: None,
        }
    }
}

/// A loaded program taken out of a [`VmMachine`] (see [`VmMachine::into_parts`]).
pub struct VmMachineParts {
    pub vm: Box<Vm>,
    /// What `load` named: a `.vm` file or a folder.
    pub target: String,
    pub files: Vec<VmLoadedFile>,
    /// Whether any class was compiled from Jack.
    pub from_jack: bool,
}

pub struct VmMachine {
    vm: Option<Box<Vm>>,
    /// What the last successful `load` named.
    target: Option<String>,
    files: Vec<VmLoadedFile>,
    /// Scripts have no clock. Time moves only when the program sleeps, and
    /// then jumps straight to the moment it wakes: a test never waits.
    now_ms: u64,
    notes: Vec<String>,
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
            target: None,
            files: Vec::new(),
            now_ms: 0,
            notes: Vec::new(),
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
            .find(|f| f.stem == stem)
            .map(|f| f.source_path.as_str())
    }

    /// The classes of the loaded program.
    pub fn files(&self) -> &[VmLoadedFile] {
        &self.files
    }

    /// What the last successful `load` named: a `.vm` file or a folder.
    pub fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }

    /// Whether any class of the loaded program was compiled from Jack.
    pub fn from_jack(&self) -> bool {
        self.files.iter().any(|f| f.jack_lines.is_some())
    }

    /// Takes the loaded program out, to keep running it elsewhere. `None`
    /// if nothing has been loaded.
    pub fn into_parts(self) -> Option<VmMachineParts> {
        let from_jack = self.from_jack();
        let vm = self.vm?;
        Some(VmMachineParts {
            vm,
            target: self.target.unwrap_or_default(),
            files: self.files,
            from_jack,
        })
    }

    fn same_stem(a: &str, b: &str, ignore_case: bool) -> bool {
        if ignore_case {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    }

    fn find_sibling_jack(files: &Files<'_>, dir: &str, stem: &str) -> Option<String> {
        if let Ok(entries) = files.host.list(dir) {
            let candidates: Vec<String> = entries
                .into_iter()
                .filter(|e| !e.is_dir && path::has_extension(&e.name, "jack"))
                .map(|e| e.name)
                .collect();
            if let Some(exact) = candidates.iter().find(|n| path::stem(n) == stem) {
                return Some(path::join(dir, exact));
            }
            if files.ignore_case {
                let folded: Vec<&String> = candidates
                    .iter()
                    .filter(|n| path::stem(n).eq_ignore_ascii_case(stem))
                    .collect();
                if let [one] = folded.as_slice() {
                    return Some(path::join(dir, one));
                }
            }
            return None;
        }
        let exact = path::join(dir, &format!("{stem}.jack"));
        files.host.read(&exact).is_ok().then_some(exact)
    }

    /// Compiles only the selected `.jack` files together in memory, so an
    /// unfinished `.jack` file whose `.vm` artifact was chosen does not fail
    /// the build.
    fn compile_jack_files(
        files: &Files<'_>,
        selected: &[(String, String)],
    ) -> Result<Vec<(VmLoadedFile, String)>, String> {
        let mut texts: Vec<(String, String, String)> = Vec::with_capacity(selected.len());
        for (stem, file) in selected {
            let text = read_text(files.host.as_ref(), file)
                .map_err(|e| format!("cannot read `{}`: {e}", path::file_name(file)))?
                .text;
            texts.push((stem.clone(), file.clone(), text));
        }
        let inputs: Vec<(&str, &str)> = texts
            .iter()
            .map(|(stem, _, text)| (stem.as_str(), text.as_str()))
            .collect();
        let program = n2t_jack::compile_program(&inputs);
        let mut compiled = Vec::with_capacity(texts.len());
        for (output, (stem, file, text)) in program.files.iter().zip(texts) {
            let Some(vm_text) = &output.compiled.vm else {
                let name = path::file_name(&file);
                let message = output
                    .compiled
                    .diagnostics
                    .iter()
                    .find(|d| d.is_error())
                    .map_or_else(
                        || format!("`{name}` does not compile"),
                        |d| {
                            let at = d.span.map_or(String::new(), |span| {
                                let (line, column) =
                                    LineIndex::new(&text).line_col_one_based(&text, span.start);
                                format!(":{line}:{column}")
                            });
                            format!("`{name}` does not compile:\n  {name}{at}: {}", d.message)
                        },
                    );
                return Err(message);
            };
            let file = VmLoadedFile {
                stem,
                source_path: file,
                built_from_source: true,
                vm_lines: vm_lines(vm_text),
                jack_lines: Some(jack_lines(vm_text, &output.compiled.line_map)),
            };
            compiled.push((file, vm_text.clone()));
        }
        Ok(compiled)
    }

    /// Loads one `.vm` file, or every `.vm` (or `.jack`, when `build_from_source`
    /// is enabled) file of a folder. The error is a message for the user.
    pub fn load_path(&mut self, files: &Files<'_>, target: &str) -> Result<(), String> {
        let mut loaded_files: Vec<VmLoadedFile> = Vec::new();
        let mut parsed: Vec<ParsedVm> = Vec::new();

        if path::has_extension(target, "vm") {
            let dir = path::dir(target);
            let stem = path::stem(target);
            let sibling_jack = if files.build_from_source {
                Self::find_sibling_jack(files, dir, stem)
            } else {
                None
            };
            let existing_vm = read_text(files.host.as_ref(), target);
            if let Some(jack_path) = &sibling_jack
                && (matches!(existing_vm, Err(HostError::NotFound)) || files.prefers(jack_path))
            {
                let selected = vec![(path::stem(jack_path).to_string(), jack_path.clone())];
                let compiled = Self::compile_jack_files(files, &selected)?;
                let Some((file, vm_text)) = compiled.into_iter().next() else {
                    return Err(format!("cannot compile `{}`", path::file_name(jack_path)));
                };
                let source_path = file.source_path.clone();
                if let Ok(disk_vm) = &existing_vm
                    && !same_lines(&disk_vm.text, &vm_text)
                {
                    self.notes.push(format!(
                        "Compiled `{}` in memory (newer than `{}` on disk).",
                        path::file_name(&source_path),
                        path::file_name(target)
                    ));
                }
                let unit = n2t_vm::parse(&file.stem, &vm_text);
                if let Some(d) = unit.diagnostics.iter().find(|d| d.is_error()) {
                    return Err(describe(path::file_name(&source_path), &vm_text, d));
                }
                loaded_files.push(file);
                parsed.push(unit);
            } else {
                let text = existing_vm
                    .map_err(|e| format!("cannot read `{}`: {e}", path::file_name(target)))?
                    .text;
                let unit = n2t_vm::parse(stem, &text);
                if let Some(d) = unit.diagnostics.iter().find(|d| d.is_error()) {
                    return Err(describe(path::file_name(target), &text, d));
                }
                loaded_files.push(VmLoadedFile::from_vm(
                    stem.to_string(),
                    target.to_string(),
                    &text,
                ));
                parsed.push(unit);
            }
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
            let mut vm_sources: Vec<(String, String)> = Vec::new();
            let mut jack_sources: Vec<(String, String)> = Vec::new();
            for entry in entries {
                if !entry.is_dir && path::has_extension(&entry.name, "vm") {
                    vm_sources.push((
                        path::stem(&entry.name).to_string(),
                        path::join(target, &entry.name),
                    ));
                } else if !entry.is_dir && path::has_extension(&entry.name, "jack") {
                    jack_sources.push((
                        path::stem(&entry.name).to_string(),
                        path::join(target, &entry.name),
                    ));
                }
            }
            vm_sources.sort();
            jack_sources.sort();

            let selected_jacks: Vec<(String, String)> = if files.build_from_source {
                jack_sources
                    .iter()
                    .filter(|(stem, jack_path)| {
                        let has_vm = vm_sources
                            .iter()
                            .any(|(vm_stem, _)| Self::same_stem(vm_stem, stem, files.ignore_case));
                        !has_vm || files.prefers(jack_path)
                    })
                    .cloned()
                    .collect()
            } else {
                Vec::new()
            };

            if !selected_jacks.is_empty() {
                let compiled = Self::compile_jack_files(files, &selected_jacks)?;
                let mut units: Vec<(VmLoadedFile, String)> = Vec::new();
                for (file, vm_text) in compiled {
                    let stem = file.stem.clone();
                    let jack_path = file.source_path.clone();
                    if let Some((_, disk_vm_path)) = vm_sources
                        .iter()
                        .find(|(vm_stem, _)| Self::same_stem(vm_stem, &stem, files.ignore_case))
                        && let Ok(disk_vm) = read_text(files.host.as_ref(), disk_vm_path)
                        && !same_lines(&disk_vm.text, &vm_text)
                    {
                        self.notes.push(format!(
                            "Compiled `{}` in memory (newer than `{}` on disk).",
                            path::file_name(&jack_path),
                            path::file_name(disk_vm_path)
                        ));
                    }
                    units.push((file, vm_text));
                }
                for (stem, vm_path) in &vm_sources {
                    let replaced = selected_jacks
                        .iter()
                        .any(|(jack_stem, _)| Self::same_stem(jack_stem, stem, files.ignore_case));
                    if !replaced {
                        let text = read_text(files.host.as_ref(), vm_path)
                            .map_err(|e| {
                                format!("cannot read `{}`: {e}", path::file_name(vm_path))
                            })?
                            .text;
                        let file = VmLoadedFile::from_vm(stem.clone(), vm_path.clone(), &text);
                        units.push((file, text));
                    }
                }
                units.sort_by(|a, b| a.0.stem.cmp(&b.0.stem));
                for (file, vm_text) in units {
                    let unit = n2t_vm::parse(&file.stem, &vm_text);
                    if let Some(d) = unit.diagnostics.iter().find(|d| d.is_error()) {
                        return Err(describe(path::file_name(&file.source_path), &vm_text, d));
                    }
                    loaded_files.push(file);
                    parsed.push(unit);
                }
            } else {
                if vm_sources.is_empty() {
                    return Err(format!(
                        "there are no `.vm` files in `{}`; compile the Jack program or write the VM code first",
                        path::file_name(target)
                    ));
                }
                for (stem, file) in vm_sources {
                    let text = read_text(files.host.as_ref(), &file)
                        .map_err(|e| format!("cannot read `{}`: {e}", path::file_name(&file)))?
                        .text;
                    let unit = n2t_vm::parse(&stem, &text);
                    if let Some(d) = unit.diagnostics.iter().find(|d| d.is_error()) {
                        return Err(describe(path::file_name(&file), &text, d));
                    }
                    loaded_files.push(VmLoadedFile::from_vm(stem, file, &text));
                    parsed.push(unit);
                }
            }
        }
        let (program, diagnostics) = Program::link(parsed);
        if let Some(d) = diagnostics.iter().find(|d| d.is_error()) {
            return Err(format!("the program does not link: {}", d.message));
        }
        self.vm = Some(Box::new(Vm::new(program)));
        self.files = loaded_files;
        self.target = Some(target.to_string());
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

    fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }

    fn loaded_inputs(&self) -> Vec<LoadedInput> {
        self.files
            .iter()
            .map(|f| LoadedInput {
                path: f.source_path.clone(),
                built_from_source: f.built_from_source,
            })
            .collect()
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

/// The trimmed text of each line of a VM file.
pub fn vm_lines(vm_text: &str) -> Vec<String> {
    vm_text.lines().map(|l| l.trim().to_string()).collect()
}

/// For each line of compiled VM code, the zero-based Jack line it came from.
/// `line_map` has one entry per VM command, in order; a blank line belongs
/// to the command before it.
pub fn jack_lines(vm_text: &str, line_map: &[u32]) -> Vec<u32> {
    let mut lines = Vec::new();
    let mut command = 0usize;
    for line in vm_text.lines() {
        if !line.trim().is_empty() {
            command += 1;
        }
        let index = command.saturating_sub(1);
        lines.push(line_map.get(index).copied().unwrap_or(0));
    }
    lines
}

fn same_lines(a: &str, b: &str) -> bool {
    a.lines()
        .map(str::trim_end)
        .eq(b.lines().map(str::trim_end))
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
