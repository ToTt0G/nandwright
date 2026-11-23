//! A test-script run, steppable from the editor.

use n2t_core::host::{SharedHost, path, read_text};
use n2t_core::source::{LineIndex, Span};
use n2t_simulator::runner::{Event, standard_machine};
use n2t_simulator::{Newline, Outcome, RunnerOptions, ScriptRunner, StartError, SuspendReason};
use serde_json::{Value, json};

use crate::{Failure, Reply, budget, key_code, observed, range, text};

/// Commands per `test.run` when the caller does not say.
const DEFAULT_BUDGET: u64 = 200_000;

pub struct TestSession {
    runner: ScriptRunner,
    source: String,
    lines: LineIndex,
    /// How many failures have already been reported to the caller.
    reported_failures: usize,
}

impl TestSession {
    /// `{"op":"test.open","path":…}` with optional `autoKeyboard` (default
    /// true), `stopEndlessWaits` (default false), `crlf`, `collectAll` (default true) and `maxCommands`.
    pub fn open(host: &SharedHost, request: &Value) -> Result<Self, Failure> {
        let script_path = path::normalize(text(request, "path")?);
        let source = read_text(host.as_ref(), &script_path)
            .map_err(|e| format!("cannot read `{}`: {e}", path::file_name(&script_path)))?
            .text;
        let options = RunnerOptions {
            max_commands: request["maxCommands"].as_u64(),
            cancel: None,
            newline: if request["crlf"].as_bool().unwrap_or(false) {
                Newline::CrLf
            } else {
                Newline::Lf
            },
            collect_all_mismatches: request["collectAll"].as_bool().unwrap_or(true),
            auto_keyboard: request["autoKeyboard"].as_bool().unwrap_or(true),
            stop_endless_waits: request["stopEndlessWaits"].as_bool().unwrap_or(false),
        };
        let lines = LineIndex::new(&source);
        let mut runner = ScriptRunner::new(
            host.clone(),
            &source,
            path::dir(&script_path),
            options,
            &standard_machine,
        )
        .map_err(|e| match e {
            StartError::Syntax(diagnostics) => {
                let first = diagnostics.iter().find(|d| d.is_error());
                match first {
                    Some(d) => {
                        let at = d.span.map_or(String::new(), |s| {
                            let (line, column) = lines.line_col_one_based(&source, s.start);
                            format!(" (line {line}, column {column})")
                        });
                        format!("the script has a syntax error{at}: {}", d.message)
                    }
                    None => "the script has syntax errors".to_string(),
                }
            }
            other => other.to_string(),
        })?;
        runner.set_script_path(&script_path);
        Ok(Self {
            runner,
            source,
            lines,
            reported_failures: 0,
        })
    }

    pub fn describe(&self) -> Value {
        json!({
            "dialect": self.runner.dialect().as_str(),
            "next": self.next_range(),
            "runsForever": self.runner.runs_forever(),
        })
    }

    pub fn call(&mut self, op: &str, request: &Value) -> Reply {
        match op {
            "test.run" => {
                let budget = budget(request, "budget", DEFAULT_BUDGET);
                let outcome = match request["breakLines"].as_array() {
                    Some(lines) if !lines.is_empty() => {
                        let lines: Vec<u64> = lines.iter().filter_map(Value::as_u64).collect();
                        self.run_to_line(budget, &lines)
                    }
                    _ => self.runner.run_for(budget),
                };
                Ok(self.status(outcome))
            }
            "test.step" => {
                // A step to the next `;` can be a loop that never ends; it
                // stops at the budget (reason `budget`), and the same
                // request carries on with it.
                let outcome = match request["unit"].as_str() {
                    Some("command") => self.runner.step_command(),
                    _ => self
                        .runner
                        .step_for(budget(request, "budget", DEFAULT_BUDGET)),
                };
                Ok(self.status(outcome))
            }
            "test.inspect" => Ok(self.inspect()),
            "key" => {
                let code = key_code(request)?;
                Ok(json!({ "accepted": self.runner.machine_mut().set_key(code) }))
            }
            other => Err(format!("`{other}` is not an operation of a test run")),
        }
    }

    /// Runs until the next command starts on one of `lines` (zero-based), as
    /// an editor breakpoint asks, or until the run stops for another reason.
    fn run_to_line(&mut self, budget: u64, lines: &[u64]) -> Outcome {
        for _ in 0..budget {
            let outcome = self.runner.step_command();
            let at_step_end = matches!(
                outcome,
                Outcome::Suspended {
                    reason: SuspendReason::StepEnd,
                    ..
                }
            );
            if !at_step_end {
                return outcome;
            }
            let executed = match &outcome {
                Outcome::Suspended { span, .. } => *span,
                _ => Span::default(),
            };
            let line_of =
                |offset: usize| u64::from(self.lines.position_utf16(&self.source, offset).line);
            // A breakpoint is hit on arriving at its line: from another line,
            // or by a loop coming back around to an earlier command on it.
            // The remaining commands of the line just left do not re-trigger.
            let next_line = self.runner.next_span().and_then(|next| {
                let line = line_of(next.start);
                (line != line_of(executed.start) || next.start <= executed.start).then_some(line)
            });
            if next_line.is_some_and(|line| lines.contains(&line)) {
                return match outcome {
                    Outcome::Suspended { span, .. } => Outcome::Suspended {
                        reason: SuspendReason::Breakpoint,
                        span,
                    },
                    other => other,
                };
            }
        }
        match self.runner.next_span() {
            Some(span) => Outcome::Suspended {
                reason: SuspendReason::Budget,
                span,
            },
            None => self.runner.run_for(0),
        }
    }

    fn range(&self, span: Span) -> Value {
        range(&self.source, &self.lines, span)
    }

    fn next_range(&self) -> Value {
        self.runner
            .next_span()
            .map_or(Value::Null, |span| self.range(span))
    }

    fn status(&mut self, outcome: Outcome) -> Value {
        let events: Vec<Value> = self
            .runner
            .take_events()
            .into_iter()
            .map(|event| match event {
                Event::Echo(text) => json!({ "type": "echo", "text": text }),
                Event::ClearEcho => json!({ "type": "clearEcho" }),
                Event::OutputLine(line) => json!({ "type": "output", "line": line }),
                Event::KeyPressed(code) => json!({ "type": "key", "code": code }),
                Event::Note(text) => json!({ "type": "note", "text": text }),
            })
            .collect();
        let failures: Vec<Value> = self.runner.failures()[self.reported_failures..]
            .iter()
            .map(|failure| {
                json!({
                    "line": failure.mismatch.line,
                    "column": failure.mismatch.column,
                    "expected": failure.mismatch.expected,
                    "actual": failure.mismatch.actual,
                    "range": self.range(failure.span),
                    "state": observed(&failure.state),
                })
            })
            .collect();
        self.reported_failures = self.runner.failures().len();

        let mut status = json!({
            "events": events,
            "failures": failures,
            "commands": self.runner.commands_executed(),
            "echo": self.runner.echo(),
        });
        match outcome {
            Outcome::Suspended { reason, span } => {
                status["state"] = json!("suspended");
                status["reason"] = json!(match reason {
                    SuspendReason::Budget => "budget",
                    SuspendReason::Pause => "pause",
                    SuspendReason::Breakpoint => "breakpoint",
                    SuspendReason::StepEnd => "step",
                });
                status["at"] = self.range(span);
                status["next"] = self.next_range();
            }
            finished => {
                status["state"] = json!("finished");
                status["outcome"] = self.outcome(&finished);
            }
        }
        status
    }

    fn outcome(&self, outcome: &Outcome) -> Value {
        match outcome {
            Outcome::Passed {
                compared_lines,
                unreached_lines,
            } => json!({
                "kind": "passed",
                "comparedLines": compared_lines,
                "unreachedLines": unreached_lines,
            }),
            Outcome::CompletedUnchecked => json!({ "kind": "unchecked" }),
            Outcome::FailedComparison { mismatch, span } => json!({
                "kind": "failed",
                "line": mismatch.line,
                "column": mismatch.column,
                "expected": mismatch.expected,
                "actual": mismatch.actual,
                "message": mismatch.to_string(),
                "range": span.map(|s| self.range(s)),
                "failureCount": self.runner.failures().len(),
            }),
            Outcome::FailedError(error) => json!({
                "kind": "error",
                "message": error.message,
                "range": error.span.map(|s| self.range(s)),
            }),
            Outcome::LimitExceeded => json!({
                "kind": "limit",
                "echo": self.runner.echo(),
                "commands": self.runner.commands_executed(),
            }),
            Outcome::Cancelled => json!({ "kind": "cancelled" }),
            Outcome::Suspended { .. } => json!({ "kind": "suspended" }),
        }
    }

    fn inspect(&self) -> Value {
        json!({
            "dialect": self.runner.dialect().as_str(),
            "state": observed(&self.runner.machine().observe()),
            "outputPath": self.runner.output_path(),
            "output": self.runner.output_text(),
            "comparePath": self.runner.compare_path(),
            "expected": self.runner.expected_lines(),
            "echo": self.runner.echo(),
            "next": self.next_range(),
            "commands": self.runner.commands_executed(),
        })
    }
}
