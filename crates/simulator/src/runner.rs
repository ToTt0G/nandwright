//! Executes `.tst` scripts.
//!
//! The runner owns control flow (statements, loops, terminators, pauses),
//! output schemas, the `.out` file, `.cmp` comparison and breakpoints. A
//! [`Machine`] owns the simulated chip, CPU or VM. Which machine a script
//! gets is decided by [`Script::dialect`].
//!
//! Results follow the official Java tools, which coursework is graded
//! against. In particular a run ends at the first comparison failure and the
//! `.out` file ends with the failing line; a compare file with lines left over
//! still passes; and a script without `compare-to` simply ends.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use n2t_core::builtins::BuiltinLibrary;
use n2t_core::diagnostic::{Diagnostic, has_errors};
use n2t_core::hdl::ChipUnit;
use n2t_core::hdl::resolve::{HostSourceProvider, SourceProvider};
use n2t_core::host::{HostError, SharedHost, path, read_text};
use n2t_core::source::{LineIndex, Span};
use n2t_core::tst::ast::*;

use crate::format::{self, Comparator, Mismatch, Value};
use crate::machine::{Files, HardwareMachine, Machine, Observed};

pub use crate::program::parse_hack;

/// The line ending written to `.out` files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Newline {
    #[default]
    Lf,
    /// What the official tools write on Windows.
    CrLf,
}

impl Newline {
    pub fn as_str(self) -> &'static str {
        match self {
            Newline::Lf => "\n",
            Newline::CrLf => "\r\n",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RunnerOptions {
    /// Stop with [`Outcome::LimitExceeded`] after this many simple commands.
    pub max_commands: Option<u64>,
    /// Set to `true` from another thread to cancel the run.
    pub cancel: Option<Arc<AtomicBool>>,
    pub newline: Newline,
    /// Keep executing after the first comparison failure, to collect every
    /// failing line for display. The outcome and the `.out` file are still
    /// those of the first failure, as with the official tools.
    pub collect_all_mismatches: bool,
    /// Press the key a script is waiting for. Some official tests (project
    /// 5's `Memory.tst`) loop until the keyboard reads a given code and ask
    /// the student to hold that key down in the simulator window. With this
    /// option the runner presses it, holds it until the next `output`, and
    /// reports an [`Event::KeyPressed`].
    pub auto_keyboard: bool,
}

/// An error that stops a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunError {
    pub message: String,
    /// The span of the failing command in the script.
    pub span: Option<Span>,
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuspendReason {
    /// A command terminated by `!`.
    Pause,
    /// A breakpoint's condition became true.
    Breakpoint,
    /// End of a `;` step while single-stepping.
    StepEnd,
    /// The step budget of [`ScriptRunner::run_for`] ran out.
    Budget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The script completed and every compared line matched.
    Passed {
        compared_lines: usize,
        /// Lines of the compare file the script never reached. The official
        /// tools pass such a run, so this does too; a front-end can say so.
        unreached_lines: usize,
    },
    /// The script completed without a compare file.
    CompletedUnchecked,
    FailedComparison {
        mismatch: Mismatch,
        span: Option<Span>,
    },
    FailedError(RunError),
    /// Resumable: call [`ScriptRunner::run`] or [`ScriptRunner::step`] again.
    Suspended {
        reason: SuspendReason,
        span: Span,
    },
    Cancelled,
    LimitExceeded,
}

impl Outcome {
    pub fn is_success(&self) -> bool {
        matches!(self, Outcome::Passed { .. } | Outcome::CompletedUnchecked)
    }

    pub fn is_terminal(&self) -> bool {
        !matches!(self, Outcome::Suspended { .. })
    }
}

/// Things a front-end may want to show while a script runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Echo(String),
    ClearEcho,
    /// A header or data line written to the output file.
    OutputLine(String),
    /// The runner pressed a key because the script was waiting for it
    /// (see [`RunnerOptions::auto_keyboard`]).
    KeyPressed(u16),
}

/// One compared line that did not match, with the machine's state when it
/// was produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub mismatch: Mismatch,
    /// The `output` command that produced the line.
    pub span: Span,
    /// Every observable value of the machine at that moment (for a chip:
    /// all pins, internal ones included).
    pub state: Vec<Observed>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Run,
    Step,
    StepCommand,
}

#[derive(Debug, Clone, Copy, Default)]
struct Cursor {
    statement: usize,
    body: usize,
    iteration: u64,
    condition_checked: bool,
}

struct Breakpoint {
    variable: Variable,
    value: Scalar,
    latched: bool,
}

enum Stop {
    Error(RunError),
    Mismatch(Mismatch),
}

impl From<RunError> for Stop {
    fn from(e: RunError) -> Self {
        Stop::Error(e)
    }
}

pub(crate) fn error(message: impl Into<String>, span: Span) -> RunError {
    RunError {
        message: message.into(),
        span: Some(span),
    }
}

/// Why a script could not be started.
#[derive(Debug)]
pub enum StartError {
    /// The script could not be read.
    Io(String, HostError),
    /// Lexical/syntax errors. Nothing has been executed or written.
    Syntax(Vec<Diagnostic>),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::Io(path, e) => write!(f, "cannot read `{path}`: {e}"),
            StartError::Syntax(d) => write!(
                f,
                "the script has {} syntax error(s)",
                d.iter().filter(|d| d.is_error()).count()
            ),
        }
    }
}

impl std::error::Error for StartError {}

/// Makes the machine for a dialect.
pub type MachineFactory<'a> = &'a dyn Fn(Dialect, &SharedHost, &str) -> Box<dyn Machine>;

/// The standard machines: chips resolve against the script's folder on the
/// host, then the built-in chip library.
pub fn standard_machine(dialect: Dialect, host: &SharedHost, dir: &str) -> Box<dyn Machine> {
    hardware_machine(
        dialect,
        Box::new(HostSourceProvider::new(host.clone(), dir)),
    )
}

/// The standard machines, with chip sources from `provider` (an editor
/// passes one that sees unsaved buffers).
pub fn hardware_machine(_dialect: Dialect, provider: Box<dyn SourceProvider>) -> Box<dyn Machine> {
    Box::new(HardwareMachine::new(
        provider,
        Arc::new(BuiltinLibrary::standard()),
    ))
}

/// Runs one script. A runner is single-use: create a new one for a fresh run.
pub struct ScriptRunner {
    host: SharedHost,
    /// The script's folder: every file it names is relative to it.
    dir: String,
    script_path: Option<String>,
    script: Arc<Script>,
    dialect: Dialect,
    options: RunnerOptions,
    machine: Box<dyn Machine>,

    cursor: Cursor,
    /// The output file and the text written to it so far.
    output: Option<(String, String)>,
    output_dirty: bool,
    compare: Option<(String, Comparator)>,
    schema: Option<Vec<OutputItem>>,
    breakpoints: Vec<Breakpoint>,
    echo: Option<String>,
    events: Vec<Event>,
    failures: Vec<Failure>,
    /// Set once a comparison has failed and the run continues only to collect
    /// the remaining failures.
    first_failure: Option<(Mismatch, Span)>,
    /// The key held down by [`RunnerOptions::auto_keyboard`].
    held_key: Option<u16>,
    commands: u64,
    current_span: Span,
    finished: Option<Outcome>,
}

impl ScriptRunner {
    /// Opens a script file on `host`. `script_path` is an engine-form path.
    pub fn open(
        host: SharedHost,
        script_path: &str,
        options: RunnerOptions,
    ) -> Result<Self, StartError> {
        let text = read_text(host.as_ref(), script_path)
            .map_err(|e| StartError::Io(script_path.to_string(), e))?;
        let dir = path::dir(script_path).to_string();
        let mut runner = Self::new(host, &text.text, &dir, options, &standard_machine)?;
        runner.script_path = Some(script_path.to_string());
        Ok(runner)
    }

    /// Parses `script` (rejecting syntax errors) and prepares a run whose
    /// companion files resolve against `dir` on `host`.
    pub fn new(
        host: SharedHost,
        script: &str,
        dir: &str,
        options: RunnerOptions,
        machines: MachineFactory<'_>,
    ) -> Result<Self, StartError> {
        let parsed = n2t_core::tst::parse(script);
        if has_errors(&parsed.diagnostics) {
            return Err(StartError::Syntax(parsed.diagnostics));
        }
        let dialect = parsed.script.dialect();
        let machine = machines(dialect, &host, dir);
        Ok(Self {
            host,
            dir: dir.to_string(),
            script_path: None,
            script: Arc::new(parsed.script),
            dialect,
            options,
            machine,
            cursor: Cursor::default(),
            output: None,
            output_dirty: false,
            compare: None,
            schema: None,
            breakpoints: Vec::new(),
            echo: None,
            events: Vec::new(),
            failures: Vec::new(),
            first_failure: None,
            held_key: None,
            commands: 0,
            current_span: Span::default(),
            finished: None,
        })
    }

    /// Records where the script lives, so `output-file` can refuse to
    /// overwrite it.
    pub fn set_script_path(&mut self, script_path: &str) {
        self.script_path = Some(script_path.to_string());
    }

    pub fn dialect(&self) -> Dialect {
        self.dialect
    }

    pub fn machine(&self) -> &dyn Machine {
        self.machine.as_ref()
    }

    pub fn machine_mut(&mut self) -> &mut dyn Machine {
        self.machine.as_mut()
    }

    /// The chip under test, for a hardware script that has loaded one.
    pub fn hardware(&self) -> Option<&HardwareMachine> {
        self.machine.as_any().downcast_ref()
    }

    pub fn hardware_mut(&mut self) -> Option<&mut HardwareMachine> {
        self.machine.as_any_mut().downcast_mut()
    }

    pub fn echo(&self) -> Option<&str> {
        self.echo.as_deref()
    }

    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    pub fn commands_executed(&self) -> u64 {
        self.commands
    }

    /// The path of the current output file, if any.
    pub fn output_path(&self) -> Option<&str> {
        self.output.as_ref().map(|(path, _)| path.as_str())
    }

    /// Everything written to the output file so far (with `\n` line breaks).
    pub fn output_text(&self) -> Option<&str> {
        self.output.as_ref().map(|(_, text)| text.as_str())
    }

    /// The path of the compare file, if one is open.
    pub fn compare_path(&self) -> Option<&str> {
        self.compare.as_ref().map(|(path, _)| path.as_str())
    }

    /// The lines of the compare file, if one is open.
    pub fn expected_lines(&self) -> Option<&[String]> {
        self.compare.as_ref().map(|(_, c)| c.expected_lines())
    }

    /// Every comparison failure seen so far. With
    /// [`RunnerOptions::collect_all_mismatches`] this goes beyond the first.
    pub fn failures(&self) -> &[Failure] {
        &self.failures
    }

    /// The span of the most recently executed command.
    pub fn current_span(&self) -> Span {
        self.current_span
    }

    /// The span of the command that will execute next, if the script has not
    /// ended.
    pub fn next_span(&self) -> Option<Span> {
        match self.script.statements.get(self.cursor.statement)? {
            Statement::Simple(s) => Some(s.span),
            Statement::Repeat(r) => r.body.get(self.cursor.body).map(|s| s.span),
            Statement::While(w) => w.body.get(self.cursor.body).map(|s| s.span),
        }
    }

    /// Whether the runner is currently positioned on a `while` loop statement.
    pub fn in_while_loop(&self) -> bool {
        matches!(
            self.script.statements.get(self.cursor.statement),
            Some(Statement::While(_))
        )
    }

    /// Completed iterations of the current `repeat` or `while` loop.
    pub fn loop_iteration(&self) -> u64 {
        self.cursor.iteration
    }

    /// Runs until completion, failure or suspension.
    pub fn run(&mut self) -> Outcome {
        self.execute(Mode::Run, u64::MAX)
    }

    /// Runs at most `budget` commands, then suspends with
    /// [`SuspendReason::Budget`] so the caller can stay responsive.
    pub fn run_for(&mut self, budget: u64) -> Outcome {
        self.execute(Mode::Run, budget)
    }

    /// Runs until the end of the current `;` step (or an earlier stop).
    pub fn step(&mut self) -> Outcome {
        self.execute(Mode::Step, u64::MAX)
    }

    /// Executes a single simple command, suspending with [`SuspendReason::StepEnd`]
    /// even if the command is comma-terminated (e.g. `while out <> 75 { eval, }`).
    pub fn step_command(&mut self) -> Outcome {
        self.execute(Mode::StepCommand, u64::MAX)
    }

    fn execute(&mut self, mode: Mode, budget: u64) -> Outcome {
        if let Some(outcome) = &self.finished {
            return outcome.clone();
        }
        let mut outcome = self.execute_inner(mode, budget);
        if let Some(e) = self.flush_output()
            && outcome.is_terminal()
            && outcome.is_success()
        {
            outcome = Outcome::FailedError(e);
        }
        if outcome.is_terminal() {
            self.finished = Some(outcome.clone());
        }
        outcome
    }

    /// Writes the output file if it changed. Returns the error, if any.
    fn flush_output(&mut self) -> Option<RunError> {
        if !self.output_dirty {
            return None;
        }
        self.output_dirty = false;
        let (path, text) = self.output.as_ref()?;
        let data = match self.options.newline {
            Newline::Lf => text.clone(),
            Newline::CrLf => text.replace('\n', "\r\n"),
        };
        self.host
            .write(path, data.as_bytes())
            .err()
            .map(|e| RunError {
                message: format!("cannot write `{}`: {e}", path::file_name(path)),
                span: None,
            })
    }

    fn execute_inner(&mut self, mode: Mode, budget: u64) -> Outcome {
        let script = self.script.clone();
        let mut executed: u64 = 0;
        loop {
            if self
                .options
                .cancel
                .as_ref()
                .is_some_and(|c| c.load(Ordering::Relaxed))
            {
                return Outcome::Cancelled;
            }
            if self
                .options
                .max_commands
                .is_some_and(|max| self.commands >= max)
            {
                return self.ended_early(Outcome::LimitExceeded);
            }

            let Some(statement) = script.statements.get(self.cursor.statement) else {
                return self.complete();
            };
            let (simple, body_len) = match statement {
                Statement::Simple(s) => (s, None),
                Statement::Repeat(r) => {
                    let done = r.body.is_empty()
                        || (self.cursor.body == 0
                            && r.count.is_some_and(|c| self.cursor.iteration >= c.value));
                    if done {
                        self.next_statement();
                        continue;
                    }
                    (&r.body[self.cursor.body], Some(r.body.len()))
                }
                Statement::While(w) => {
                    if w.body.is_empty() {
                        self.next_statement();
                        continue;
                    }
                    if self.cursor.body == 0 && !self.cursor.condition_checked {
                        match self.condition(&w.condition) {
                            Ok(true) => {
                                self.cursor.condition_checked = true;
                                if self.cursor.iteration >= 1 {
                                    self.press_awaited_key(&w.condition);
                                }
                            }
                            Ok(false) => {
                                self.next_statement();
                                continue;
                            }
                            Err(e) => return self.ended_early(Outcome::FailedError(e)),
                        }
                    }
                    (&w.body[self.cursor.body], Some(w.body.len()))
                }
            };

            if executed >= budget {
                return Outcome::Suspended {
                    reason: SuspendReason::Budget,
                    span: simple.span,
                };
            }
            executed += 1;

            self.current_span = simple.span;
            match self.command(simple) {
                Ok(()) => {}
                Err(Stop::Error(e)) => return self.ended_early(Outcome::FailedError(e)),
                Err(Stop::Mismatch(mismatch)) => {
                    let failure = Failure {
                        mismatch: mismatch.clone(),
                        span: simple.span,
                        state: self.machine.observe(),
                    };
                    self.failures.push(failure);
                    if self.first_failure.is_none() {
                        // The official tools stop here, and so does the
                        // output file.
                        if let Some(e) = self.flush_output() {
                            return Outcome::FailedError(e);
                        }
                        self.first_failure = Some((mismatch.clone(), simple.span));
                    }
                    if !self.options.collect_all_mismatches {
                        return Outcome::FailedComparison {
                            mismatch,
                            span: Some(simple.span),
                        };
                    }
                }
            }
            self.commands += 1;

            match body_len {
                None => self.next_statement(),
                Some(len) => {
                    self.cursor.body += 1;
                    if self.cursor.body == len {
                        self.cursor.body = 0;
                        self.cursor.iteration += 1;
                        self.cursor.condition_checked = false;
                    }
                }
            }

            if self.first_failure.is_some() {
                // Only collecting failures now: no pauses, no stepping.
                continue;
            }
            match self.check_breakpoints() {
                Ok(true) => {
                    return Outcome::Suspended {
                        reason: SuspendReason::Breakpoint,
                        span: simple.span,
                    };
                }
                Ok(false) => {}
                Err(e) => return Outcome::FailedError(e),
            }
            match simple.terminator.map(|t| t.kind) {
                Some(TerminatorKind::Bang) => {
                    return Outcome::Suspended {
                        reason: SuspendReason::Pause,
                        span: simple.span,
                    };
                }
                Some(TerminatorKind::Semicolon) if mode == Mode::Step => {
                    return Outcome::Suspended {
                        reason: SuspendReason::StepEnd,
                        span: simple.span,
                    };
                }
                _ if mode == Mode::StepCommand => {
                    return Outcome::Suspended {
                        reason: SuspendReason::StepEnd,
                        span: simple.span,
                    };
                }
                _ => {}
            }
        }
    }

    /// If the loop is waiting for a value that only the keyboard can
    /// produce (`while out <> 75 { … }`), presses that key.
    fn press_awaited_key(&mut self, condition: &Condition) {
        if !self.options.auto_keyboard || condition.relation != Relation::NotEqual {
            return;
        }
        let constant = match (&condition.left, &condition.right) {
            (Operand::Variable(_), Operand::Scalar(Scalar::Integer(i)))
            | (Operand::Scalar(Scalar::Integer(i)), Operand::Variable(_)) => i,
            _ => return,
        };
        let Ok(code) = constant.constant_value() else {
            return;
        };
        let Ok(code) = u16::try_from(code) else {
            return;
        };
        if code == 0 || self.held_key == Some(code) {
            return;
        }
        if self.machine.set_key(code) {
            self.held_key = Some(code);
            self.events.push(Event::KeyPressed(code));
        }
    }

    /// The outcome when a run stops before the end of the script: if a
    /// comparison had already failed, that is the result.
    fn ended_early(&self, outcome: Outcome) -> Outcome {
        match &self.first_failure {
            Some((mismatch, span)) => Outcome::FailedComparison {
                mismatch: mismatch.clone(),
                span: Some(*span),
            },
            None => outcome,
        }
    }

    fn next_statement(&mut self) {
        self.cursor = Cursor {
            statement: self.cursor.statement + 1,
            ..Cursor::default()
        };
    }

    fn complete(&mut self) -> Outcome {
        if let Some((mismatch, span)) = &self.first_failure {
            return Outcome::FailedComparison {
                mismatch: mismatch.clone(),
                span: Some(*span),
            };
        }
        match &self.compare {
            None => Outcome::CompletedUnchecked,
            Some((_, comparator)) => Outcome::Passed {
                compared_lines: comparator.compared(),
                unreached_lines: comparator.remaining(),
            },
        }
    }

    // ---------------------------------------------------------------------
    // Commands
    // ---------------------------------------------------------------------

    fn command(&mut self, statement: &SimpleStatement) -> Result<(), Stop> {
        let span = statement.command_span;
        match &statement.command {
            Command::Load(file) => {
                let files = Files {
                    host: &self.host,
                    dir: &self.dir,
                };
                self.machine.load(&files, file)?;
                self.breakpoints.clear();
            }
            Command::OutputFile(file) => self.output_file(file)?,
            Command::CompareTo(file) => {
                let path = path::join(&self.dir, &file.name);
                let text = read_text(self.host.as_ref(), &path).map_err(|e| {
                    let message = match e {
                        HostError::NotFound => format!(
                            "the compare file `{}` does not exist in the script's folder",
                            file.name
                        ),
                        other => format!("cannot read `{}`: {other}", file.name),
                    };
                    error(message, file.span)
                })?;
                self.compare = Some((path, Comparator::new(&text.text)));
            }
            Command::OutputList(items) => {
                if self.output.is_none() {
                    return Err(error(
                        "`output-list` requires an output file; use `output-file Xxx.out` first",
                        span,
                    )
                    .into());
                }
                for item in items {
                    self.machine.read(&item.variable)?;
                }
                self.schema = Some(items.clone());
                let labels: Vec<(String, OutputFormat)> = items
                    .iter()
                    .map(|i| (i.variable.to_string(), i.effective_format()))
                    .collect();
                let line = format::header(labels.iter().map(|(n, f)| (n.as_str(), *f)));
                self.output_line(&line)?;
            }
            Command::Output => {
                let Some(schema) = self.schema.clone() else {
                    return Err(error(
                        "`output` requires an output schema; use `output-list` first",
                        span,
                    )
                    .into());
                };
                let mut values = Vec::with_capacity(schema.len());
                for item in &schema {
                    values.push((self.machine.read(&item.variable)?, item.effective_format()));
                }
                let line = format::row(values.iter().map(|(v, f)| (v, *f)));
                let result = self.output_line(&line);
                // The awaited key has now been seen in the output: release it.
                if self.held_key.is_some() && !self.in_while_loop() {
                    self.machine.set_key(0);
                    self.held_key = None;
                }
                result?;
            }
            Command::Set { variable, value } => self.machine.write(variable, value)?,
            Command::Eval
            | Command::Tick
            | Command::Tock
            | Command::TickTock
            | Command::VmStep
            | Command::BuiltinMethod { .. } => {
                let files = Files {
                    host: &self.host,
                    dir: &self.dir,
                };
                self.machine.execute(&files, statement)?;
            }
            Command::Echo(text) => {
                self.echo = Some(text.value.clone());
                self.events.push(Event::Echo(text.value.clone()));
            }
            Command::ClearEcho => {
                self.echo = None;
                self.events.push(Event::ClearEcho);
            }
            Command::Breakpoint { variable, value } => {
                let current = self.machine.read(variable)?;
                // Validate the value's type once at registration.
                breakpoint_matches(&current, value).map_err(|m| error(m, span))?;
                let duplicate = self.breakpoints.iter().any(|b| {
                    b.variable
                        .to_string()
                        .eq_ignore_ascii_case(&variable.to_string())
                        && scalar_text(&b.value) == scalar_text(value)
                });
                if !duplicate {
                    self.breakpoints.push(Breakpoint {
                        variable: variable.clone(),
                        value: value.clone(),
                        latched: false,
                    });
                }
            }
            Command::ClearBreakpoints => self.breakpoints.clear(),
        }
        Ok(())
    }

    fn output_file(&mut self, file: &FileName) -> Result<(), RunError> {
        let target = path::join(&self.dir, &file.name);
        let protected = [
            self.script_path.as_deref(),
            self.compare.as_ref().map(|(p, _)| p.as_str()),
        ];
        if protected
            .iter()
            .flatten()
            .any(|p| p.eq_ignore_ascii_case(&target))
        {
            return Err(error(
                format!(
                    "refusing to overwrite `{}`; choose another output file",
                    file.name
                ),
                file.span,
            ));
        }
        if let Some(e) = self.flush_output() {
            return Err(e);
        }
        self.output = Some((target, String::new()));
        // The official tools create the file at once, even if nothing is
        // ever written to it.
        self.output_dirty = true;
        Ok(())
    }

    fn output_line(&mut self, line: &str) -> Result<(), Stop> {
        let span = self.current_span;
        let failed_before = self.first_failure.is_some();
        let Some((_, text)) = &mut self.output else {
            return Err(error(
                "no output file is open; use `output-file Xxx.out` first",
                span,
            )
            .into());
        };
        if !failed_before {
            text.push_str(line);
            text.push('\n');
            self.output_dirty = true;
            self.events.push(Event::OutputLine(line.to_string()));
        }
        if let Some((_, comparator)) = &mut self.compare
            && let Err(mismatch) = comparator.compare(line)
        {
            return Err(Stop::Mismatch(mismatch));
        }
        Ok(())
    }

    // ---------------------------------------------------------------------
    // Conditions and breakpoints
    // ---------------------------------------------------------------------

    fn operand(&self, operand: &Operand) -> Result<Value, RunError> {
        match operand {
            Operand::Variable(v) => self.machine.read(v),
            Operand::Scalar(Scalar::String(s)) => Ok(Value::Text(s.value.clone())),
            Operand::Scalar(Scalar::Integer(i)) => {
                let value = i.constant_value().map_err(|m| error(m, i.span))?;
                Ok(Value::Int {
                    bits: value as u64,
                    width: 64,
                })
            }
        }
    }

    fn condition(&self, condition: &Condition) -> Result<bool, RunError> {
        let left = self.operand(&condition.left)?;
        let right = self.operand(&condition.right)?;
        let relation = condition.relation;
        match (&left, &right) {
            (Value::Text(a), Value::Text(b)) => {
                if !relation.applies_to_strings() {
                    return Err(error(
                        "text can only be compared with `=` or `<>`",
                        condition.relation_span,
                    ));
                }
                Ok(relation.evaluate(a, b))
            }
            (Value::Int { .. }, Value::Int { .. }) => {
                Ok(relation.evaluate(&int_scalar(&left), &int_scalar(&right)))
            }
            _ => Err(error("cannot compare text with an integer", condition.span)),
        }
    }

    /// Returns whether a breakpoint fired.
    fn check_breakpoints(&mut self) -> Result<bool, RunError> {
        let mut fired = false;
        for i in 0..self.breakpoints.len() {
            let current = self.machine.read(&self.breakpoints[i].variable)?;
            let matches = breakpoint_matches(&current, &self.breakpoints[i].value)
                .map_err(|m| error(m, self.breakpoints[i].variable.span))?;
            let breakpoint = &mut self.breakpoints[i];
            if matches && !breakpoint.latched {
                breakpoint.latched = true;
                fired = true;
            } else if !matches {
                breakpoint.latched = false;
            }
        }
        Ok(fired)
    }
}

/// Numeric value for comparisons: constants are stored as full 64-bit
/// patterns of their signed value; pins follow [`Value::scalar`].
fn int_scalar(value: &Value) -> i64 {
    match *value {
        Value::Int { bits, width: 64 } => bits as i64,
        ref v => v.scalar().unwrap_or(0),
    }
}

fn scalar_text(scalar: &Scalar) -> String {
    match scalar {
        Scalar::Integer(i) => i
            .constant_value()
            .map(|v| v.to_string())
            .unwrap_or_default(),
        Scalar::String(s) => format!("\"{}\"", s.value),
    }
}

fn breakpoint_matches(current: &Value, value: &Scalar) -> Result<bool, String> {
    match (current, value) {
        (Value::Text(t), Scalar::String(s)) => Ok(*t == s.value),
        (Value::Int { .. }, Scalar::Integer(i)) => {
            Ok(current.scalar() == Some(i.constant_value()?))
        }
        _ => Err("the breakpoint value's type does not match the variable (`time` is text)".into()),
    }
}

/// Summarises why a chip failed to load, with locations.
pub fn load_failure(file_name: &str, unit: &ChipUnit) -> String {
    let mut message = format!("cannot load `{file_name}`");
    let errors: Vec<&Diagnostic> = unit.diagnostics.iter().filter(|d| d.is_error()).collect();
    if errors.is_empty() {
        return message;
    }
    message.push(':');
    let text = unit.text().unwrap_or("");
    let lines = LineIndex::new(text);
    let source = unit.source_id().map(path::file_name);
    for d in errors.iter().take(10) {
        match (source, d.span) {
            (Some(source), Some(span)) => {
                let (line, column) = lines.line_col_one_based(text, span.start);
                message.push_str(&format!("\n  {source}:{line}:{column}: {}", d.message));
            }
            (Some(source), None) => message.push_str(&format!("\n  {source}: {}", d.message)),
            (None, _) => message.push_str(&format!("\n  {}", d.message)),
        }
        for related in &d.related {
            message.push_str(&format!("\n    note: {}", related.message));
        }
    }
    if errors.len() > 10 {
        message.push_str(&format!("\n  … and {} more", errors.len() - 10));
    }
    message
}
