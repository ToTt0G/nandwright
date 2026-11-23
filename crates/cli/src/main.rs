//! `n2t`: the Nandwright engine on the command line.
//!
//! The same engine ships inside the VS Code extension as WebAssembly. This
//! native build exists for the test suite and for the conformance harness,
//! which diffs it against the official Java tools.

mod report;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use n2t_core::Severity;
use n2t_core::builtins::BuiltinLibrary;
use n2t_core::hdl::resolve::{HostSourceProvider, SourceFile, is_valid_chip_name};
use n2t_core::hdl::{Elaborator, ElaboratorOptions};
use n2t_core::host::{Encoding, SharedHost, StdHost, decode_text, encode_text, path};
use n2t_core::source::LineIndex;
use n2t_simulator::runner::Event;
use n2t_simulator::{
    HardwareMachine, Machine, Newline, Outcome, Role, RunnerOptions, ScriptRunner, StartError,
    SuspendReason,
};
use serde_json::json;

use crate::report::Reporter;

#[derive(Parser)]
#[command(
    name = "n2t",
    version,
    about = "Nand to Tetris tools: check, test, evaluate and format"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check `.hdl`, `.tst`, `.asm`, `.hack`, `.vm` and `.jack` files for errors
    /// (directories are searched recursively).
    Check {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Exit with failure if there are warnings.
        #[arg(long)]
        deny_warnings: bool,
    },
    /// Run `.tst` scripts (directories are searched recursively).
    Test {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Stop a script after this many commands (guards against infinite loops).
        #[arg(long, default_value_t = 50_000_000)]
        max_commands: u64,
        /// Treat scripts without `compare-to` as failures.
        #[arg(long)]
        require_compare: bool,
        /// Treat `!` pauses and breakpoints as failures instead of continuing.
        #[arg(long)]
        strict_pauses: bool,
        /// Do not press the key a script is waiting for (as the official
        /// tools require a person to).
        #[arg(long)]
        no_keys: bool,
        /// Write `.out` files with CRLF line endings, as the official tools do on Windows.
        #[arg(long)]
        crlf: bool,
        /// Print one JSON object per script instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Load a chip, set inputs, evaluate, and print its pins.
    Eval {
        /// The chip's `.hdl` file.
        chip: PathBuf,
        /// Input assignments `name=value` (decimal, `0b…` or `0x…`).
        inputs: Vec<String>,
        /// Clock cycles (tick/tock) to run after setting the inputs.
        #[arg(long, default_value_t = 0)]
        cycles: u64,
    },
    /// Format `.hdl` and `.tst` files (directories are searched recursively).
    Fmt {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Check whether files are formatted without modifying them.
        #[arg(long)]
        check: bool,
        /// Number of spaces per indentation level.
        #[arg(long, default_value_t = 4)]
        indent: usize,
        /// Indent with tabs instead of spaces.
        #[arg(long)]
        tabs: bool,
    },
    /// List the built-in chips.
    Builtins,
    /// Start the language server on stdin/stdout.
    Lsp,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Check {
            paths,
            deny_warnings,
        } => check(&paths, deny_warnings),
        Command::Test {
            paths,
            max_commands,
            require_compare,
            strict_pauses,
            no_keys,
            crlf,
            json,
        } => test(
            &paths,
            &TestOptions {
                max_commands,
                require_compare,
                strict_pauses,
                auto_keyboard: !no_keys,
                crlf,
                json,
            },
        ),
        Command::Eval {
            chip,
            inputs,
            cycles,
        } => eval(&chip, &inputs, cycles),
        Command::Fmt {
            paths,
            check,
            indent,
            tabs,
        } => fmt(&paths, check, indent, tabs),
        Command::Builtins => {
            for chip in BuiltinLibrary::standard().iter() {
                println!("{}", chip.interface);
            }
            Ok(true)
        }
        Command::Lsp => n2t_lsp::run_stdio()
            .map(|()| true)
            .map_err(|e| e.to_string()),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
    }
}

fn host() -> SharedHost {
    Arc::new(StdHost)
}

/// Files with one of `extensions` under the given paths, as engine-form
/// absolute paths, sorted.
fn collect(paths: &[PathBuf], extensions: &[&str]) -> Result<Vec<String>, String> {
    fn walk(dir: &Path, extensions: &[&str], out: &mut Vec<String>) -> Result<(), String> {
        let entries = std::fs::read_dir(dir)
            .map_err(|e| format!("cannot read `{}`: {e}", display_native(dir)))?;
        for entry in entries.filter_map(Result::ok) {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                walk(&entry_path, extensions, out)?;
            } else {
                let file = StdHost::absolute(&entry_path);
                if extensions.iter().any(|e| path::has_extension(&file, e)) {
                    out.push(file);
                }
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    for item in paths {
        if item.is_dir() {
            walk(item, extensions, &mut files)?;
        } else if item.exists() {
            files.push(StdHost::absolute(item));
        } else {
            return Err(format!("`{}` does not exist", display_native(item)));
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn display_native(native: &Path) -> String {
    display(&StdHost::absolute(native))
}

/// A path as shown to the user: relative to the working directory when it is
/// inside it. Never the `\\?\` form.
fn display(file: &str) -> String {
    let cwd = std::env::current_dir()
        .map(|c| StdHost::absolute(&c))
        .unwrap_or_default();
    path::relative_to(file, &cwd).to_string()
}

fn read(file: &str) -> Result<n2t_core::host::TextFile, String> {
    std::fs::read(file)
        .map(decode_text)
        .map_err(|e| format!("cannot read `{}`: {e}", display(file)))
}

// -------------------------------------------------------------------------
// check
// -------------------------------------------------------------------------

fn check(paths: &[PathBuf], deny_warnings: bool) -> Result<bool, String> {
    let files = collect(paths, &["hdl", "tst", "asm", "hack", "vm", "jack"])?;
    let builtins = Arc::new(BuiltinLibrary::standard());
    let mut by_dir: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for file in files {
        by_dir
            .entry(path::dir(&file).to_string())
            .or_default()
            .push(file);
    }

    let mut reporter = Reporter::new();
    let mut checked = 0;
    for (dir, files) in by_dir {
        let mut elaborator = Elaborator::with_options(
            HostSourceProvider::new(host(), &dir),
            builtins.clone(),
            ElaboratorOptions::editor(),
        );
        for file in &files {
            let file = file.clone();
            checked += 1;
            let text = read(&file)?.text;
            let shown = display(&file);
            if path::has_extension(&file, "hdl") {
                let name = path::stem(&file).to_string();
                let unit = if is_valid_chip_name(&name) {
                    elaborator.compile(&name)
                } else {
                    Arc::new(
                        elaborator.compile_file(None, SourceFile::new(shown.clone(), text.clone())),
                    )
                };
                let source = unit.text().unwrap_or(&text).to_string();
                for d in &unit.diagnostics {
                    reporter.diagnostic(&shown, &source, d);
                }
            } else if path::has_extension(&file, "tst") {
                let (parsed, analysis) = n2t_core::tst::check(&text, &mut elaborator);
                for d in parsed.diagnostics.iter().chain(&analysis.diagnostics) {
                    reporter.diagnostic(&shown, &text, d);
                }
            } else if path::has_extension(&file, "asm") {
                for d in &n2t_hack::assemble(&text).diagnostics {
                    reporter.diagnostic(&shown, &text, d);
                }
            } else if path::has_extension(&file, "vm") {
                for d in &n2t_vm::parse(path::stem(&file), &text).diagnostics {
                    reporter.diagnostic(&shown, &text, d);
                }
            } else if path::has_extension(&file, "hack")
                && let Err(diagnostics) = n2t_hack::parse_hack(&text)
            {
                for d in &diagnostics {
                    reporter.diagnostic(&shown, &text, d);
                }
            }
        }
        // The Jack classes of a folder are one program: calls between them
        // are checked together, as the official compiler does for a folder.
        let jack: Vec<(String, String, String)> = files
            .iter()
            .filter(|f| path::has_extension(f, "jack"))
            .filter_map(|f| Some((path::stem(f).to_string(), read(f).ok()?.text, display(f))))
            .collect();
        if !jack.is_empty() {
            let sources: Vec<(&str, &str)> = jack
                .iter()
                .map(|(stem, text, _)| (stem.as_str(), text.as_str()))
                .collect();
            let program = n2t_jack::compile_program(&sources);
            // Shown as an editor shows them: an unwritten subroutine is a
            // hint (see `n2t_jack::editor`).
            for (output, (_, text, shown)) in program.files.iter().zip(&jack) {
                let diagnostics = n2t_jack::editor_diagnostics(
                    &n2t_jack::parse(text),
                    path::file_name(&dir),
                    &output.compiled.diagnostics,
                );
                for d in &diagnostics {
                    reporter.diagnostic(shown, text, d);
                }
            }
        }
    }

    let summary = format!(
        "checked {checked} file{}: {} error{}, {} warning{}",
        if checked == 1 { "" } else { "s" },
        reporter.errors,
        if reporter.errors == 1 { "" } else { "s" },
        reporter.warnings,
        if reporter.warnings == 1 { "" } else { "s" },
    );
    let ok = reporter.errors == 0 && !(deny_warnings && reporter.warnings > 0);
    eprintln!(
        "{}",
        if ok {
            reporter.green(&summary)
        } else {
            reporter.red(&summary)
        }
    );
    Ok(ok)
}

// -------------------------------------------------------------------------
// test
// -------------------------------------------------------------------------

struct TestOptions {
    max_commands: u64,
    require_compare: bool,
    strict_pauses: bool,
    auto_keyboard: bool,
    crlf: bool,
    json: bool,
}

/// The name of a key code, for messages.
fn key_name(code: u16) -> String {
    match char::from_u32(code as u32) {
        Some(c) if (33..127).contains(&code) => format!("'{c}'"),
        _ => format!("code {code}"),
    }
}

fn test(paths: &[PathBuf], options: &TestOptions) -> Result<bool, String> {
    let scripts = collect(paths, &["tst"])?;
    if scripts.is_empty() {
        return Err("no `.tst` files found".into());
    }
    let mut reporter = Reporter::new();
    let (mut passed, mut failed) = (0, 0);

    for script in &scripts {
        let shown = display(script);
        let source = read(script).map(|f| f.text).unwrap_or_default();
        let lines = LineIndex::new(&source);
        let position =
            |span: Option<n2t_core::Span>| span.map(|s| lines.line_col_one_based(&source, s.start));
        let runner_options = RunnerOptions {
            max_commands: Some(options.max_commands),
            newline: if options.crlf {
                Newline::CrLf
            } else {
                Newline::Lf
            },
            auto_keyboard: options.auto_keyboard,
            ..Default::default()
        };
        let mut runner = match ScriptRunner::open(host(), script, runner_options) {
            Ok(runner) => runner,
            Err(e) => {
                failed += 1;
                if options.json {
                    let (message, at) = match &e {
                        StartError::Syntax(diagnostics) => {
                            let first = diagnostics.iter().find(|d| d.is_error());
                            (
                                first.map_or_else(|| e.to_string(), |d| d.message.clone()),
                                position(first.and_then(|d| d.span)),
                            )
                        }
                        other => (other.to_string(), None),
                    };
                    let mut error = json!({ "message": message });
                    if let Some((line, column)) = at {
                        error["line"] = json!(line);
                        error["column"] = json!(column);
                    }
                    println!(
                        "{}",
                        json!({ "script": script, "outcome": "error", "error": error })
                    );
                } else if let StartError::Syntax(diagnostics) = &e {
                    for d in diagnostics {
                        reporter.diagnostic(&shown, &source, d);
                    }
                    println!("{} {shown} (syntax errors)", reporter.red("FAIL"));
                } else {
                    println!("{} {shown}: {e}", reporter.red("FAIL"));
                }
                continue;
            }
        };

        let mut pauses = 0;
        let mut keys = Vec::new();
        let mut notes = Vec::new();
        let outcome = loop {
            let outcome = runner.run();
            for event in runner.take_events() {
                match event {
                    Event::KeyPressed(code) => keys.push(code),
                    Event::Note(text) => notes.push(text),
                    _ => {}
                }
            }
            match outcome {
                Outcome::Suspended {
                    reason: SuspendReason::Pause | SuspendReason::Breakpoint,
                    ..
                } if !options.strict_pauses => pauses += 1,
                outcome => break outcome,
            }
        };
        let last_echo = runner.echo().map(str::to_string);
        let mut note = String::new();
        if pauses > 0 {
            note.push_str(&format!(
                " [continued past {pauses} pause{}]",
                if pauses == 1 { "" } else { "s" }
            ));
        }
        if !keys.is_empty() {
            let names: Vec<String> = keys.iter().map(|&k| key_name(k)).collect();
            note.push_str(&format!(" [pressed {} for the script]", names.join(", ")));
        }

        let ok = outcome.is_success()
            && !(options.require_compare && matches!(outcome, Outcome::CompletedUnchecked));
        if ok {
            passed += 1;
        } else {
            failed += 1;
        }

        if !options.json {
            for note in &notes {
                eprintln!("note: {shown}: {note}");
            }
        }
        if options.json {
            let mut record = json!({
                "script": script,
                "dialect": runner.dialect().as_str(),
            });
            if !notes.is_empty() {
                record["notes"] = json!(notes);
            }
            if let Some(out) = runner.output_path() {
                record["out_file"] = json!(out);
            }
            if !keys.is_empty() {
                record["keys_pressed"] = json!(keys);
            }
            let with_position = |mut value: serde_json::Value, span: Option<n2t_core::Span>| {
                if let Some((line, column)) = position(span) {
                    value["script_line"] = json!(line);
                    value["script_column"] = json!(column);
                }
                value
            };
            match &outcome {
                Outcome::Passed {
                    compared_lines,
                    unreached_lines,
                } => {
                    record["outcome"] = json!("passed");
                    record["compared_lines"] = json!(compared_lines);
                    if *unreached_lines > 0 {
                        record["unreached_lines"] = json!(unreached_lines);
                    }
                }
                Outcome::CompletedUnchecked => record["outcome"] = json!("unchecked"),
                Outcome::FailedComparison { mismatch, span } => {
                    record["outcome"] = json!("failed");
                    record["mismatch"] = with_position(
                        json!({
                            "line": mismatch.line,
                            "column": mismatch.column,
                            "expected": mismatch.expected,
                            "actual": mismatch.actual,
                        }),
                        *span,
                    );
                }
                Outcome::FailedError(error) => {
                    record["outcome"] = json!("error");
                    let mut value = json!({ "message": error.message });
                    if let Some((line, column)) = position(error.span) {
                        value["line"] = json!(line);
                        value["column"] = json!(column);
                    }
                    record["error"] = value;
                }
                Outcome::Suspended { span, .. } => {
                    record["outcome"] = json!("error");
                    record["error"] = with_position(
                        json!({ "message": "the script was suspended" }),
                        Some(*span),
                    );
                }
                Outcome::LimitExceeded => record["outcome"] = json!("limit"),
                Outcome::Cancelled => record["outcome"] = json!("cancelled"),
            }
            println!("{record}");
            continue;
        }

        match &outcome {
            Outcome::Passed {
                compared_lines,
                unreached_lines,
            } => {
                println!(
                    "{} {shown} ({compared_lines} lines compared){note}",
                    reporter.green("PASS")
                );
                if *unreached_lines > 0 {
                    eprintln!(
                        "  note: the compare file has {unreached_lines} more line(s) that the script never reached; the official tools pass this too"
                    );
                }
            }
            Outcome::CompletedUnchecked => {
                let label = if ok {
                    reporter.yellow("DONE")
                } else {
                    reporter.red("FAIL")
                };
                println!("{label} {shown} (no compare file){note}");
            }
            Outcome::FailedComparison { mismatch, span } => {
                println!("{} {shown}{note}", reporter.red("FAIL"));
                reporter.message(
                    Severity::Error,
                    &mismatch.to_string(),
                    &shown,
                    &source,
                    *span,
                );
                if let Some(failure) = runner.failures().first() {
                    let internal: Vec<String> = failure
                        .state
                        .iter()
                        .filter(|o| o.role == Role::Internal)
                        .map(|o| format!("{}={}", o.name, pin_text(o.bits, o.width)))
                        .collect();
                    if !internal.is_empty() {
                        eprintln!("  internal pins at that step: {}", internal.join(", "));
                    }
                }
            }
            Outcome::FailedError(error) => {
                println!("{} {shown}{note}", reporter.red("FAIL"));
                reporter.message(Severity::Error, &error.message, &shown, &source, error.span);
            }
            Outcome::Suspended { reason, span } => {
                println!("{} {shown}: suspended ({reason:?})", reporter.red("FAIL"));
                reporter.message(
                    Severity::Error,
                    "the script was suspended",
                    &shown,
                    &source,
                    Some(*span),
                );
            }
            Outcome::LimitExceeded => {
                println!(
                    "{} {shown}: still running after {} commands",
                    reporter.red("FAIL"),
                    options.max_commands
                );
                if let Some(echo) = &last_echo {
                    eprintln!("  note: the script's last message was: \"{echo}\"");
                }
                eprintln!(
                    "  hint: a script that loops forever or waits for a key press is meant to be run interactively"
                );
            }
            Outcome::Cancelled => {
                println!("{} {shown}: cancelled", reporter.red("FAIL"));
            }
        }
    }

    if !options.json {
        let summary = format!("{passed} passed, {failed} failed");
        println!(
            "{}",
            if failed == 0 {
                reporter.green(&summary)
            } else {
                reporter.red(&summary)
            }
        );
    }
    Ok(failed == 0)
}

fn pin_text(bits: u64, width: u32) -> String {
    if width == 1 {
        bits.to_string()
    } else if width == 16 {
        format!("{}", bits as u16 as i16)
    } else {
        format!("{bits}")
    }
}

// -------------------------------------------------------------------------
// eval
// -------------------------------------------------------------------------

fn parse_number(text: &str) -> Option<i64> {
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let value = if let Some(bin) = digits.strip_prefix("0b") {
        i64::from_str_radix(bin, 2).ok()?
    } else if let Some(hex) = digits.strip_prefix("0x") {
        i64::from_str_radix(hex, 16).ok()?
    } else {
        digits.parse().ok()?
    };
    Some(if negative { -value } else { value })
}

fn eval(chip: &Path, inputs: &[String], cycles: u64) -> Result<bool, String> {
    let file = StdHost::absolute(chip);
    let dir = path::dir(&file).to_string();
    let name = path::stem(&file).to_string();
    let mut machine = HardwareMachine::new(
        Box::new(HostSourceProvider::new(host(), &dir)),
        Arc::new(BuiltinLibrary::standard()),
    );
    if let Err(e) = machine.load_chip(&name, path::file_name(&file), n2t_core::Span::default()) {
        eprintln!("error: {}", e.message);
        return Ok(false);
    }
    let Some(circuit) = machine.circuit_mut() else {
        return Ok(false);
    };
    let definition = circuit.definition().clone();

    for assignment in inputs {
        let (pin, value) = assignment
            .split_once('=')
            .ok_or_else(|| format!("expected `name=value`, found `{assignment}`"))?;
        let value = parse_number(value).ok_or_else(|| format!("`{value}` is not a number"))?;
        let Some((_, info)) = definition.interface.input(pin) else {
            return Err(format!("`{name}` has no input named `{pin}`"));
        };
        let mask = if info.width >= 64 {
            u64::MAX
        } else {
            (1u64 << info.width) - 1
        };
        circuit.set(pin, value as u64 & mask);
    }
    circuit.eval();
    for _ in 0..cycles {
        circuit.step().map_err(|e| e.to_string())?;
    }
    let time = circuit.time();

    let observed = machine.observe();
    let width = observed.iter().map(|o| o.name.len()).max().unwrap_or(0);
    for role in [Role::Input, Role::Output, Role::Internal] {
        let label = match role {
            Role::Input => "IN",
            Role::Output => "OUT",
            _ => "INT",
        };
        for pin in observed.iter().filter(|o| o.role == role) {
            let binary = format!("{:0w$b}", pin.bits, w = pin.width as usize);
            let signed = if pin.width == 16 {
                pin.bits as u16 as i16 as i64
            } else {
                pin.bits as i64
            };
            println!("{label:<3} {:<width$}  {binary}  ({signed})", pin.name);
        }
    }
    if cycles > 0 {
        println!("time {time}");
    }
    Ok(true)
}

// -------------------------------------------------------------------------
// fmt
// -------------------------------------------------------------------------

fn fmt(paths: &[PathBuf], check_only: bool, indent: usize, tabs: bool) -> Result<bool, String> {
    let all = collect(paths, &["hdl", "tst"])?;
    // A file named on the command line that is not HDL or a test script is
    // said to be unsupported, not to have syntax errors.
    let (files, unsupported): (Vec<String>, Vec<String>) = all
        .into_iter()
        .partition(|f| path::has_extension(f, "hdl") || path::has_extension(f, "tst"));
    for file in &unsupported {
        eprintln!(
            "skipped {}: only `.hdl` and `.tst` files can be formatted",
            display(file)
        );
    }
    if files.is_empty() {
        return Err("no `.hdl` or `.tst` files found".into());
    }
    let options = n2t_core::FormatOptions {
        tab_size: indent.max(1),
        use_tabs: tabs,
        trailing_newline: true,
    };

    let mut changed = 0usize;
    let mut skipped = 0usize;
    for file in &files {
        let source = read(file)?;
        let formatted = if path::has_extension(file, "hdl") {
            n2t_core::hdl::format_with_options(&source.text, &options)
        } else {
            n2t_core::tst::format_with_options(&source.text, &options)
        };
        let shown = display(file);
        let Some(formatted) = formatted else {
            eprintln!("skipped {shown} (it has syntax errors; fix them and format again)");
            skipped += 1;
            continue;
        };
        if formatted == source.text {
            continue;
        }
        changed += 1;
        if check_only {
            println!("would format {shown}");
            continue;
        }
        // Write the file back in the encoding it was read in: formatting
        // changes whitespace, never the bytes of a comment.
        let bytes = encode_text(&formatted, source.encoding)
            .or_else(|| encode_text(&formatted, Encoding::Utf8))
            .unwrap_or_default();
        std::fs::write(file, bytes).map_err(|e| format!("cannot write `{shown}`: {e}"))?;
        println!("formatted {shown}");
    }

    if check_only {
        Ok(changed == 0 && skipped == 0)
    } else {
        Ok(skipped == 0)
    }
}
