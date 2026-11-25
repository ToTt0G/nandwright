//! A VM program running on the VM emulator: `.vm` files as they are on disk,
//! or the Jack classes of a folder, compiled in memory.
//!
//! Running Jack never writes `.vm` files. In projects 10 and 11 the `.vm`
//! files in a folder are the output of the student's own compiler, and a
//! "run" that silently replaced them would destroy the thing being tested.

use n2t_core::host::{HostError, SharedHost, path, read_text};
use n2t_core::source::LineIndex;
use n2t_simulator::VmMachine;
use n2t_simulator::machine::Files;
use n2t_simulator::vm_machine::{jack_lines, vm_lines};
use n2t_vm::{Breakpoints, ParsedVm, Program, RunStatus, StepStatus, Vm};
use serde_json::{Value, json};

use crate::chip::parse_value;
use crate::{Failure, Reply, text};

const DEFAULT_BUDGET: u64 = 100_000;
const MAX_WINDOW: u64 = 512;
/// Most commands one "step a line" may execute before giving up.
const LINE_STEP_LIMIT: u64 = 5_000_000;
/// Stack entries shown.
const STACK_SHOWN: usize = 24;

/// One class of the program and where its code comes from.
struct LoadedFile {
    stem: String,
    /// The file to show the student: the `.vm` file, or the `.jack` file the
    /// code was compiled from.
    source_path: String,
    /// The text of each VM line, for display.
    vm_lines: Vec<String>,
    /// For a compiled Jack class: the zero-based Jack line of each VM line.
    jack_lines: Option<Vec<u32>>,
}

impl LoadedFile {
    /// The line to highlight in `source_path` for a VM line.
    fn source_line(&self, vm_line: u32) -> u32 {
        match &self.jack_lines {
            Some(map) => map.get(vm_line as usize).copied().unwrap_or(0),
            None => vm_line,
        }
    }
}

pub struct VmSession {
    host: SharedHost,
    target: String,
    ignore_case: bool,
    /// When promoted from a test run, `vm.reset` reloads with the same
    /// source-building policy (`build_from_source`, `prefer_sources`) so a
    /// script that compiled a missing `.vm` from `.jack` can still reset.
    reload_from_test: Option<(bool, Vec<String>)>,
    vm: Box<Vm>,
    files: Vec<LoadedFile>,
    from_jack: bool,
    stopped: Option<String>,
    sent_screen: Option<u64>,
}

/// What `open` loads for a path the student launched from.
enum Source {
    /// These `.vm` files only.
    Vm(Vec<String>),
    /// These `.jack` files compiled in memory, plus any sibling `.vm` files
    /// whose class has no `.jack` source (for mixed project 12 folders).
    Jack {
        jack: Vec<String>,
        extra_vm: Vec<String>,
    },
}

fn same_stem(a: &str, b: &str, ignore_case: bool) -> bool {
    if ignore_case {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

fn list(host: &SharedHost, dir: &str, extension: &str) -> Result<Vec<String>, Failure> {
    let entries = host.list(dir).map_err(|e| match e {
        HostError::NotFound => format!("the folder `{}` does not exist", path::file_name(dir)),
        other => format!("cannot read the folder `{}`: {other}", path::file_name(dir)),
    })?;
    let mut files: Vec<String> = entries
        .into_iter()
        .filter(|e| !e.is_dir && path::has_extension(&e.name, extension))
        .map(|e| path::join(dir, &e.name))
        .collect();
    files.sort();
    Ok(files)
}

/// Whether a `.vm` file defines the entry point of a multi-file program.
fn defines_entry(text: &str) -> bool {
    text.lines().any(|line| {
        let mut words = line.split_whitespace();
        words.next() == Some("function") && matches!(words.next(), Some("Sys.init" | "Main.main"))
    })
}

fn choose(host: &SharedHost, target: &str, ignore_case: bool) -> Result<Source, Failure> {
    if path::has_extension(target, "jack") {
        let dir = path::dir(target);
        let jack = list(host, dir, "jack")?;
        let vm = list(host, dir, "vm")?;
        let extra_vm = vm
            .into_iter()
            .filter(|v| {
                let v_stem = path::stem(v);
                !jack
                    .iter()
                    .any(|j| same_stem(path::stem(j), v_stem, ignore_case))
            })
            .collect();
        return Ok(Source::Jack { jack, extra_vm });
    }
    if path::has_extension(target, "vm") {
        // A file of a program (a folder with `Sys.init` or `Main.main`) runs
        // with the rest of that program; a lone file runs alone, the way the
        // project 7 tests load one.
        let siblings = list(host, path::dir(target), "vm")?;
        let is_program = siblings.len() > 1
            && siblings
                .iter()
                .any(|file| read_text(host.as_ref(), file).is_ok_and(|f| defines_entry(&f.text)));
        return Ok(Source::Vm(if is_program {
            siblings
        } else {
            vec![target.to_string()]
        }));
    }
    // A folder: its VM files, or else its Jack classes.
    let vm = list(host, target, "vm")?;
    if !vm.is_empty() {
        return Ok(Source::Vm(vm));
    }
    let jack = list(host, target, "jack")?;
    if jack.is_empty() {
        return Err(format!(
            "there are no `.vm` or `.jack` files in `{}`",
            path::file_name(target)
        ));
    }
    Ok(Source::Jack {
        jack,
        extra_vm: Vec::new(),
    })
}

fn at(text: &str, span: Option<n2t_core::Span>) -> String {
    span.map_or(String::new(), |span| {
        let (line, column) = LineIndex::new(text).line_col_one_based(text, span.start);
        format!(":{line}:{column}")
    })
}

fn load(
    host: &SharedHost,
    target: &str,
    ignore_case: bool,
) -> Result<(Vm, Vec<LoadedFile>, bool), Failure> {
    let source = choose(host, target, ignore_case)?;
    let read = |file: &String| -> Result<String, Failure> {
        read_text(host.as_ref(), file)
            .map(|f| f.text)
            .map_err(|e| format!("cannot read `{}`: {e}", path::file_name(file)))
    };
    let mut loaded = Vec::new();
    let mut parsed: Vec<ParsedVm> = Vec::new();
    let from_jack = matches!(source, Source::Jack { .. });
    let parse_vm_files = |vm_files: &[String],
                          loaded: &mut Vec<LoadedFile>,
                          parsed: &mut Vec<ParsedVm>|
     -> Result<(), Failure> {
        for file in vm_files {
            let text = read(file)?;
            let stem = path::stem(file).to_string();
            let unit = n2t_vm::parse(&stem, &text);
            if let Some(d) = unit.diagnostics.iter().find(|d| d.is_error()) {
                let name = path::file_name(file);
                return Err(format!(
                    "`{name}` has an error:\n  {name}{}: {}",
                    at(&text, d.span),
                    d.message
                ));
            }
            loaded.push(LoadedFile {
                stem,
                source_path: file.clone(),
                vm_lines: vm_lines(&text),
                jack_lines: None,
            });
            parsed.push(unit);
        }
        Ok(())
    };
    match source {
        Source::Vm(files) => {
            parse_vm_files(&files, &mut loaded, &mut parsed)?;
        }
        Source::Jack {
            jack: files,
            extra_vm,
        } => {
            let texts: Vec<(String, String)> = files
                .iter()
                .map(|file| Ok((path::stem(file).to_string(), read(file)?)))
                .collect::<Result<_, Failure>>()?;
            let inputs: Vec<(&str, &str)> = texts
                .iter()
                .map(|(stem, text)| (stem.as_str(), text.as_str()))
                .collect();
            let program = n2t_jack::compile_program(&inputs);
            for ((output, file), (stem, text)) in program.files.iter().zip(&files).zip(&texts) {
                let name = path::file_name(file);
                let Some(vm_text) = &output.compiled.vm else {
                    let message = output
                        .compiled
                        .diagnostics
                        .iter()
                        .find(|d| d.is_error())
                        .map_or_else(
                            || format!("`{name}` does not compile"),
                            |d| {
                                format!(
                                    "`{name}` does not compile:\n  {name}{}: {}",
                                    at(text, d.span),
                                    d.message
                                )
                            },
                        );
                    return Err(message);
                };
                // One VM command per line, in the order of `line_map`.
                loaded.push(LoadedFile {
                    stem: stem.clone(),
                    source_path: file.clone(),
                    vm_lines: vm_lines(vm_text),
                    jack_lines: Some(jack_lines(vm_text, &output.compiled.line_map)),
                });
                parsed.push(n2t_vm::parse(stem, vm_text));
            }
            parse_vm_files(&extra_vm, &mut loaded, &mut parsed)?;
        }
    }
    let (program, diagnostics) = Program::link(parsed);
    if let Some(d) = diagnostics.iter().find(|d| d.is_error()) {
        return Err(format!("the program does not link: {}", d.message));
    }
    Ok((Vm::new(program), loaded, from_jack))
}

impl VmSession {
    /// `{"op":"vm.open","path":…}` for a `.vm` file, a `.jack` file or a folder.
    pub fn open(host: &SharedHost, request: &Value) -> Result<Self, Failure> {
        let target = path::normalize(text(request, "path")?);
        let ignore_case = request["ignoreCase"].as_bool().unwrap_or(false);
        let (vm, files, from_jack) = load(host, &target, ignore_case)?;
        Ok(Self {
            host: host.clone(),
            target,
            ignore_case,
            reload_from_test: None,
            vm: Box::new(vm),
            files,
            from_jack,
            stopped: None,
            sent_screen: None,
        })
    }

    /// Carries on with the program a test script was running, as it stands.
    /// `fallback_target` is what `vm.reset` reloads if the machine does not
    /// know what its script loaded.
    pub fn from_machine(
        host: &SharedHost,
        fallback_target: String,
        ignore_case: bool,
        build_from_source: bool,
        prefer_sources: Vec<String>,
        machine: VmMachine,
    ) -> Result<Self, Failure> {
        let parts = machine
            .into_parts()
            .ok_or("the test script has not loaded a VM program yet")?;
        let target = if parts.target.is_empty() {
            path::normalize(&fallback_target)
        } else {
            parts.target
        };
        let files = parts
            .files
            .into_iter()
            .map(|f| LoadedFile {
                stem: f.stem,
                source_path: f.source_path,
                vm_lines: f.vm_lines,
                jack_lines: f.jack_lines,
            })
            .collect();
        Ok(Self {
            host: host.clone(),
            target,
            ignore_case,
            reload_from_test: Some((build_from_source, prefer_sources)),
            vm: parts.vm,
            files,
            from_jack: parts.from_jack,
            stopped: None,
            sent_screen: None,
        })
    }

    pub fn is_halted(&self) -> bool {
        self.vm.is_halted()
    }

    fn file(&self, stem: &str) -> Option<&LoadedFile> {
        self.files.iter().find(|f| f.stem == stem)
    }

    /// Where the next command is, in the file the student sees.
    fn position(&self) -> Option<(&LoadedFile, u32)> {
        let file = self.file(self.vm.current_file()?)?;
        Some((file, file.source_line(self.vm.current_line()?)))
    }

    /// `[{"path": …, "lines": [zero-based, …]}, …]` as VM breakpoints.
    fn breakpoints(&self, request: &Value) -> Breakpoints {
        let mut points = Breakpoints::new();
        let Some(list) = request["breakpoints"].as_array() else {
            return points;
        };
        for entry in list {
            let Some(file_path) = entry["path"].as_str() else {
                continue;
            };
            let wanted = path::normalize(file_path);
            // The exact file if there is one; otherwise the same name in
            // another case, as an editor on Windows may spell it.
            let Some(file) = self
                .files
                .iter()
                .find(|f| f.source_path == wanted)
                .or_else(|| {
                    self.files
                        .iter()
                        .find(|f| f.source_path.eq_ignore_ascii_case(&wanted))
                })
            else {
                continue;
            };
            let lines = entry["lines"].as_array().into_iter().flatten();
            for line in lines.filter_map(Value::as_u64) {
                match &file.jack_lines {
                    // A Jack line is many VM commands: stop at the first of
                    // each run of them, so that continuing moves on.
                    Some(map) => {
                        for (vm_line, &jack_line) in map.iter().enumerate() {
                            let starts_run = vm_line == 0 || map[vm_line - 1] != jack_line;
                            if u64::from(jack_line) == line && starts_run {
                                points.insert(file.stem.clone(), vm_line as u32);
                            }
                        }
                    }
                    // A VM breakpoint on a blank line or a comment stops at
                    // the next command.
                    None => {
                        let next = (line as usize..file.vm_lines.len()).find(|&i| {
                            let text = &file.vm_lines[i];
                            !text.is_empty() && !text.starts_with("//")
                        });
                        if let Some(vm_line) = next {
                            points.insert(file.stem.clone(), vm_line as u32);
                        }
                    }
                }
            }
        }
        points
    }

    pub fn call(&mut self, op: &str, request: &Value) -> Reply {
        let now = request["now"].as_u64().unwrap_or(0);
        let mut status = if self.vm.is_halted() {
            "halted"
        } else {
            "paused"
        };
        let mut until = None;
        match op {
            "vm.state" => {}
            "vm.run" | "vm.step" => {
                if let Some(message) = &self.stopped {
                    return Err(format!("{message} Reset the program to run it again."));
                }
                let run_status = if op == "vm.run" {
                    let budget = crate::budget(request, "budget", DEFAULT_BUDGET);
                    let breakpoints = self.breakpoints(request);
                    // The program is shown paused on the command it will
                    // execute next. Pressing Run there executes it, even if
                    // it has a breakpoint: a breakpoint stops a run that
                    // arrives at it, and a loop arrives again next time
                    // round. Only the first slice of a run says `resume`.
                    let resuming = request["resume"].as_bool().unwrap_or(false)
                        && budget > 0
                        && !breakpoints.is_empty();
                    let first = if resuming {
                        match self.vm.step(now) {
                            StepStatus::Stepped => None,
                            StepStatus::Halted => Some(RunStatus::Halted),
                            StepStatus::WaitingForKey => Some(RunStatus::WaitingForKey),
                            StepStatus::Sleeping { until_ms } => {
                                Some(RunStatus::Sleeping { until_ms })
                            }
                            StepStatus::Error(e) => Some(RunStatus::Error(e)),
                        }
                    } else {
                        None
                    };
                    match first {
                        Some(stopped) => stopped,
                        None => self.vm.run(budget - u64::from(resuming), now, &breakpoints),
                    }
                } else if request["unit"].as_str() == Some("line") {
                    self.step_line(now)
                } else {
                    let count = crate::budget(request, "count", 1);
                    let mut last = RunStatus::Budget;
                    for _ in 0..count {
                        last = match self.vm.step(now) {
                            StepStatus::Stepped => RunStatus::Budget,
                            StepStatus::Halted => RunStatus::Halted,
                            StepStatus::WaitingForKey => RunStatus::WaitingForKey,
                            StepStatus::Sleeping { until_ms } => RunStatus::Sleeping { until_ms },
                            StepStatus::Error(e) => RunStatus::Error(e),
                        };
                        if !matches!(last, RunStatus::Budget) {
                            break;
                        }
                    }
                    last
                };
                status = match run_status {
                    RunStatus::Budget if op == "vm.run" => "running",
                    RunStatus::Budget => "paused",
                    RunStatus::Breakpoint { .. } => "breakpoint",
                    RunStatus::Halted => "halted",
                    RunStatus::WaitingForKey => "waiting",
                    RunStatus::Sleeping { until_ms } => {
                        until = Some(until_ms);
                        "sleeping"
                    }
                    RunStatus::Error(e) => {
                        self.stopped = Some(e.to_string());
                        "error"
                    }
                };
            }
            "vm.reset" => {
                // Read (and for Jack, compile) the program again.
                let key = self.vm.key();
                let (vm, files, from_jack) = if let Some((build_from_source, prefer_sources)) =
                    &self.reload_from_test
                {
                    let dir = if path::has_extension(&self.target, "vm") {
                        path::dir(&self.target)
                    } else {
                        self.target.as_str()
                    };
                    let files_cfg = Files {
                        host: &self.host,
                        dir,
                        build_from_source: *build_from_source,
                        prefer_sources,
                        ignore_case: self.ignore_case,
                    };
                    let mut fresh = VmMachine::new();
                    fresh.load_path(&files_cfg, &self.target)?;
                    let parts = fresh
                        .into_parts()
                        .ok_or("the program could not be reloaded")?;
                    let loaded_files = parts
                        .files
                        .into_iter()
                        .map(|f| LoadedFile {
                            stem: f.stem,
                            source_path: f.source_path,
                            vm_lines: f.vm_lines,
                            jack_lines: f.jack_lines,
                        })
                        .collect();
                    (*parts.vm, loaded_files, parts.from_jack)
                } else {
                    load(&self.host, &self.target, self.ignore_case)?
                };
                *self.vm = vm;
                self.files = files;
                self.from_jack = from_jack;
                self.stopped = None;
                self.sent_screen = None;
                self.vm.set_key(key);
                status = "paused";
            }
            "vm.set" => {
                let target = text(request, "target")?.to_string();
                let value = parse_value(&request["value"], 16)? as u16 as i16;
                self.set(&target, request["address"].as_u64(), value)?;
            }
            "key" => {
                let code = crate::key_code(request)?;
                self.vm.set_key(code);
                return Ok(json!({ "accepted": true }));
            }
            other => return Err(format!("`{other}` is not an operation of the VM emulator")),
        }
        let mut state = self.state(status, request);
        if let Some(until) = until {
            state["until"] = json!(until);
        }
        Ok(state)
    }

    /// Runs until the next command is on another source line (or in another
    /// function): one step of a Jack statement's worth of VM commands.
    fn step_line(&mut self, now: u64) -> RunStatus {
        let start = self
            .position()
            .map(|(file, line)| (file.stem.clone(), line));
        let depth = self.vm.call_stack().len();
        for _ in 0..LINE_STEP_LIMIT {
            match self.vm.step(now) {
                StepStatus::Stepped => {}
                StepStatus::Halted => return RunStatus::Halted,
                StepStatus::WaitingForKey => return RunStatus::WaitingForKey,
                StepStatus::Sleeping { until_ms } => return RunStatus::Sleeping { until_ms },
                StepStatus::Error(e) => return RunStatus::Error(e),
            }
            let here = self
                .position()
                .map(|(file, line)| (file.stem.clone(), line));
            // Calls into other functions are stepped over, not into.
            if here != start && self.vm.call_stack().len() <= depth {
                break;
            }
        }
        RunStatus::Budget
    }

    fn set(&mut self, target: &str, address: Option<u64>, value: i16) -> Result<(), Failure> {
        let (name, index) = match target {
            "SP" => ("sp", None),
            "LCL" => ("local", None),
            "ARG" => ("argument", None),
            "THIS" => ("this", None),
            "THAT" => ("that", None),
            "RAM" => ("RAM", Some(address.ok_or("which RAM address?")? as i64)),
            other => return Err(format!("`{other}` cannot be set")),
        };
        self.vm.set_var(name, index, i64::from(value))
    }

    pub fn state(&mut self, status: &str, request: &Value) -> Value {
        let position = self
            .position()
            .map(|(file, line)| (file.source_path.clone(), line));
        let instruction = match (self.vm.current_file(), self.vm.current_line()) {
            (Some(stem), Some(line)) => self
                .file(stem)
                .and_then(|f| f.vm_lines.get(line as usize))
                .cloned()
                .unwrap_or_default(),
            _ => String::new(),
        };
        let frames: Vec<Value> = self
            .vm
            .call_stack()
            .iter()
            .rev()
            .map(|frame| {
                let file = frame.file.as_deref().and_then(|stem| self.file(stem));
                json!({
                    "function": frame.function,
                    "file": file.map(|f| f.source_path.clone()),
                    "line": match (file, frame.line) {
                        (Some(file), Some(line)) => json!(file.source_line(line)),
                        _ => Value::Null,
                    },
                    "locals": frame.locals,
                    "arguments": frame.arguments,
                })
            })
            .collect();
        let vm = &self.vm;
        let word = |value: i16| u64::from(value as u16);
        let stack: Vec<u64> = vm
            .stack()
            .iter()
            .rev()
            .take(STACK_SHOWN)
            .map(|&v| word(v))
            .collect();
        let memory = match request["ram"].as_array() {
            Some(window) => {
                let start = window
                    .first()
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .min(24576);
                let count = window
                    .get(1)
                    .and_then(Value::as_u64)
                    .unwrap_or(16)
                    .min(MAX_WINDOW);
                let end = (start + count).min(24577);
                let values: Vec<u64> = (start..end).map(|a| word(vm.ram(a as u16))).collect();
                json!({ "start": start, "values": values })
            }
            None => Value::Null,
        };
        let generation = vm.screen_generation();
        let force = request["screen"].as_str() == Some("force");
        let send_screen = force
            || (request["screen"].as_bool().unwrap_or(false)
                && self.sent_screen != Some(generation));
        let screen = send_screen.then(|| {
            let mut hex = String::with_capacity(8192 * 4);
            for &value in vm.screen_slice() {
                hex.push_str(&format!("{:04x}", value as u16));
            }
            hex
        });
        if send_screen {
            self.sent_screen = Some(generation);
        }
        json!({
            "kind": "vm",
            "status": status,
            "error": self.stopped,
            "registers": [
                { "name": "SP", "value": word(vm.sp()) },
                { "name": "LCL", "value": word(vm.local()) },
                { "name": "ARG", "value": word(vm.argument()) },
                { "name": "THIS", "value": word(vm.this()) },
                { "name": "THAT", "value": word(vm.that()) },
            ],
            "instruction": instruction,
            "function": vm.current_function(),
            "steps": vm.steps(),
            "programLength": vm.program().len(),
            "file": position.as_ref().map(|(file, _)| file.clone()),
            "line": position.as_ref().map(|(_, line)| *line),
            "fromJack": self.from_jack,
            "callStack": frames,
            "stack": stack,
            "key": vm.key(),
            "ram": memory,
            "screen": screen,
        })
    }
}
