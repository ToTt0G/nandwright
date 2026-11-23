//! `n2t` — command-line tools for Nand to Tetris hardware projects.

mod report;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use n2t_core::Severity;
use n2t_core::builtins::BuiltinLibrary;
use n2t_core::hdl::definition::NetKind;
use n2t_core::hdl::resolve::{FsSourceProvider, SourceFile, is_valid_chip_name};
use n2t_core::hdl::{Elaborator, ElaboratorOptions};
use n2t_simulator::runner::{Event, load_failure};
use n2t_simulator::{
    BuiltinRegistry, Circuit, Outcome, RunnerOptions, ScriptRunner, StartError, SuspendReason,
};

use crate::report::Reporter;

#[derive(Parser)]
#[command(
    name = "n2t",
    version,
    about = "Nand to Tetris HDL and test-script tools"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check `.hdl` and `.tst` files for errors (directories are searched recursively).
    Check {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Report empty `PARTS:` stubs as warnings rather than errors.
        #[arg(long)]
        allow_stubs: bool,
        /// Exit with failure if there are warnings.
        #[arg(long)]
        deny_warnings: bool,
    },
    /// Run `.tst` scripts (directories are searched recursively).
    Test {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Stop a script after this many commands (guards against infinite loops).
        #[arg(long, default_value_t = 10_000_000)]
        max_commands: u64,
        /// Treat scripts without `compare-to` as failures.
        #[arg(long)]
        require_compare: bool,
        /// Treat `!` pauses and breakpoints as failures instead of continuing.
        #[arg(long)]
        strict_pauses: bool,
        /// Comma-separated keys or numeric keycodes to feed to interactive Keyboard wait loops (e.g. `--keys K,Y` for `Memory.tst`).
        #[arg(long, value_delimiter = ',')]
        keys: Vec<String>,
    },
    /// Load a chip, set inputs, evaluate, and print its outputs.
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
        /// Maximum line width before wrapping long lists.
        #[arg(long, default_value_t = 100)]
        max_width: usize,
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
            allow_stubs,
            deny_warnings,
        } => check(&paths, allow_stubs, deny_warnings),
        Command::Test {
            paths,
            max_commands,
            require_compare,
            strict_pauses,
            keys,
        } => test(&paths, max_commands, require_compare, strict_pauses, &keys),
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
            max_width,
        } => fmt(&paths, check, indent, tabs, max_width),
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

/// Expands directories into the files with one of `extensions`, sorted.
fn collect(paths: &[PathBuf], extensions: &[&str]) -> Result<Vec<PathBuf>, String> {
    fn walk(dir: &Path, extensions: &[&str], out: &mut Vec<PathBuf>) -> Result<(), String> {
        let entries =
            std::fs::read_dir(dir).map_err(|e| format!("cannot read `{}`: {e}", dir.display()))?;
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, extensions, out)?;
            } else if has_extension(&path, extensions) {
                out.push(path);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            walk(path, extensions, &mut files)?;
        } else if path.exists() {
            files.push(path.clone());
        } else {
            return Err(format!("`{}` does not exist", path.display()));
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| extensions.contains(&e))
}

fn display(path: &Path) -> String {
    let cwd = std::env::current_dir().ok();
    cwd.and_then(|c| path.strip_prefix(c).ok().map(|p| p.display().to_string()))
        .unwrap_or_else(|| path.display().to_string())
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string()
}

fn project_dir(path: &Path) -> PathBuf {
    path.canonicalize()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

// -------------------------------------------------------------------------
// check
// -------------------------------------------------------------------------

fn check(paths: &[PathBuf], allow_stubs: bool, deny_warnings: bool) -> Result<bool, String> {
    let files = collect(paths, &["hdl", "tst"])?;
    let builtins = Arc::new(BuiltinLibrary::standard());
    let options = if allow_stubs {
        ElaboratorOptions::editor()
    } else {
        ElaboratorOptions::default()
    };
    let mut by_dir: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for file in files {
        by_dir.entry(project_dir(&file)).or_default().push(file);
    }

    let mut reporter = Reporter::new();
    let mut checked = 0;
    for (dir, files) in by_dir {
        let mut elaborator = Elaborator::with_options(
            FsSourceProvider::new(&dir),
            builtins.clone(),
            options.clone(),
        );
        for file in files {
            checked += 1;
            let text = n2t_core::source::read_text_file(&file)
                .map_err(|e| format!("cannot read `{}`: {e}", file.display()))?;
            let shown = display(&file);
            if has_extension(&file, &["hdl"]) {
                let name = stem(&file);
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
            } else {
                let (parsed, analysis) = n2t_core::tst::check(&text, &mut elaborator);
                for d in parsed.diagnostics.iter().chain(&analysis.diagnostics) {
                    reporter.diagnostic(&shown, &text, d);
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

fn parse_key(spec: &str) -> Result<u16, String> {
    let trimmed = spec.trim();
    if let Ok(code) = trimmed.parse::<u16>() {
        return Ok(code);
    }
    let mut chars = trimmed.chars();
    if let (Some(c), None) = (chars.next(), chars.next())
        && (c as u32) <= u16::MAX as u32
    {
        return Ok(c as u16);
    }
    Err(format!(
        "invalid key `{spec}`: expected a single character (e.g. `K`) or numeric Hack keycode (e.g. `75`)"
    ))
}

fn test(
    paths: &[PathBuf],
    max_commands: u64,
    require_compare: bool,
    strict_pauses: bool,
    keys: &[String],
) -> Result<bool, String> {
    let key_codes: Vec<u16> = keys
        .iter()
        .map(|k| parse_key(k))
        .collect::<Result<Vec<_>, _>>()?;
    let scripts = collect(paths, &["tst"])?;
    if scripts.is_empty() {
        return Err("no `.tst` files found".into());
    }
    let builtins = Arc::new(BuiltinLibrary::standard());
    let registry = BuiltinRegistry::standard();
    let mut reporter = Reporter::new();
    let (mut passed, mut failed) = (0, 0);

    for script in &scripts {
        let shown = display(script);
        let source = n2t_core::source::read_text_file(script).unwrap_or_default();
        let options = RunnerOptions {
            max_commands: Some(max_commands),
            cancel: None,
        };
        let mut runner =
            match ScriptRunner::open(script, builtins.clone(), registry.clone(), options) {
                Ok(runner) => runner,
                Err(StartError::Syntax(diagnostics)) => {
                    for d in &diagnostics {
                        reporter.diagnostic(&shown, &source, d);
                    }
                    println!("{} {shown} (syntax errors)", reporter.red("FAIL"));
                    failed += 1;
                    continue;
                }
                Err(e) => {
                    println!("{} {shown}: {e}", reporter.red("FAIL"));
                    failed += 1;
                    continue;
                }
            };

        let mut pauses = 0;
        let outcome = if key_codes.is_empty() {
            loop {
                match runner.run() {
                    Outcome::Suspended {
                        reason: SuspendReason::Pause | SuspendReason::Breakpoint,
                        ..
                    } if !strict_pauses => pauses += 1,
                    outcome => break outcome,
                }
            }
        } else {
            let mut key_idx = 0usize;
            let mut key_held = false;
            loop {
                let outcome = runner.step_command();
                let events = runner.take_events();
                for event in &events {
                    if let Event::OutputLine(_) = event
                        && key_held
                        && !runner.in_while_loop()
                    {
                        if let Some(c) = runner.circuit_mut() {
                            c.set_key(0);
                        }
                        key_held = false;
                        key_idx += 1;
                    }
                }
                match outcome {
                    Outcome::Suspended {
                        reason: SuspendReason::StepEnd,
                        ..
                    } => {
                        // A `while` loop that completes an iteration while an echo prompt
                        // is active indicates an interactive keyboard wait loop (such as
                        // `Memory.tst`, with either `eval,` or `eval;`/`tick, tock;`).
                        if !key_held
                            && runner.in_while_loop()
                            && runner.loop_iteration() >= 1
                            && runner.echo().is_some()
                            && let Some(&code) = key_codes.get(key_idx)
                        {
                            if let Some(c) = runner.circuit_mut() {
                                c.set_key(code);
                            }
                            key_held = true;
                        }
                    }
                    Outcome::Suspended {
                        reason: SuspendReason::Pause | SuspendReason::Breakpoint,
                        ..
                    } if !strict_pauses => pauses += 1,
                    other => break other,
                }
            }
        };
        let last_echo = runner.echo().map(str::to_string);
        let note = if pauses > 0 {
            format!(
                " [continued past {pauses} pause{}]",
                if pauses == 1 { "" } else { "s" }
            )
        } else {
            String::new()
        };

        let ok = match &outcome {
            Outcome::Passed { compared_lines } => {
                println!(
                    "{} {shown} ({compared_lines} lines compared){note}",
                    reporter.green("PASS")
                );
                true
            }
            Outcome::CompletedUnchecked => {
                let ok = !require_compare;
                let label = if ok {
                    reporter.yellow("DONE")
                } else {
                    reporter.red("FAIL")
                };
                println!("{label} {shown} (no compare file){note}");
                ok
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
                false
            }
            Outcome::FailedError(error) => {
                println!("{} {shown}{note}", reporter.red("FAIL"));
                reporter.message(Severity::Error, &error.message, &shown, &source, error.span);
                false
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
                false
            }
            Outcome::LimitExceeded => {
                println!(
                    "{} {shown}: exceeded {max_commands} commands",
                    reporter.red("FAIL")
                );
                if let Some(echo) = &last_echo {
                    eprintln!("  note: last echo message: \"{echo}\"");
                    eprintln!(
                        "  hint: if this script waits for interactive Keyboard input (e.g. Memory.tst), run it in the official HardwareSimulator GUI or pass `--keys K,Y`"
                    );
                }
                false
            }
            Outcome::Cancelled => {
                println!("{} {shown}: cancelled", reporter.red("FAIL"));
                false
            }
        };
        if ok {
            passed += 1;
        } else {
            failed += 1;
        }
    }

    let summary = format!("{passed} passed, {failed} failed");
    println!(
        "{}",
        if failed == 0 {
            reporter.green(&summary)
        } else {
            reporter.red(&summary)
        }
    );
    Ok(failed == 0)
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
    let dir = project_dir(chip);
    let name = stem(chip);
    let mut elaborator = Elaborator::new(
        FsSourceProvider::new(&dir),
        Arc::new(BuiltinLibrary::standard()),
    );
    let unit = elaborator.compile(&name);
    let Some(definition) = unit.definition.clone() else {
        let mut reporter = Reporter::new();
        let shown = display(chip);
        let source = unit.text().unwrap_or("").to_string();
        if unit.diagnostics.is_empty() {
            return Err(load_failure(&format!("{name}.hdl"), &unit));
        }
        for d in &unit.diagnostics {
            reporter.diagnostic(&shown, &source, d);
        }
        return Ok(false);
    };
    let mut circuit = Circuit::new(definition.clone(), &BuiltinRegistry::standard())
        .map_err(|e| e.to_string())?;

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

    let internals: Vec<_> = circuit
        .pins()
        .iter()
        .filter(|p| p.kind == NetKind::Internal)
        .collect();
    let width = definition
        .interface
        .inputs
        .iter()
        .chain(&definition.interface.outputs)
        .map(|p| p.name.len())
        .chain(internals.iter().map(|p| p.name.len()))
        .max()
        .unwrap_or(0);
    for (label, pins) in [
        ("IN", &definition.interface.inputs),
        ("OUT", &definition.interface.outputs),
    ] {
        for pin in pins.iter() {
            let value = circuit.get(&pin.name).unwrap_or(0);
            let binary = format!("{value:0w$b}", w = pin.width as usize);
            let signed = if pin.width == 16 {
                value as u16 as i16 as i64
            } else {
                value as i64
            };
            println!("{label:<3} {:<width$}  {binary}  ({signed})", pin.name);
        }
    }
    for pin in internals {
        let value = circuit.read(pin);
        let binary = format!("{value:0w$b}", w = pin.width as usize);
        let signed = if pin.width == 16 {
            value as u16 as i16 as i64
        } else {
            value as i64
        };
        println!("INT {:<width$}  {binary}  ({signed})", pin.name);
    }
    if cycles > 0 {
        println!("time {}", circuit.time());
    }
    Ok(true)
}

// -------------------------------------------------------------------------
// fmt
// -------------------------------------------------------------------------

fn fmt(
    paths: &[PathBuf],
    check_only: bool,
    indent: usize,
    tabs: bool,
    max_width: usize,
) -> Result<bool, String> {
    let files = collect(paths, &["hdl", "tst"])?;
    if files.is_empty() {
        return Err("no `.hdl` or `.tst` files found".into());
    }
    let options = n2t_core::FormatOptions {
        tab_size: indent.max(1),
        use_tabs: tabs,
        max_line_width: max_width.max(20),
        trailing_newline: true,
    };

    let mut changed = 0usize;
    let mut skipped = 0usize;
    for file in &files {
        let source = n2t_core::source::read_text_file(file)
            .map_err(|e| format!("cannot read `{}`: {e}", file.display()))?;
        let formatted = if has_extension(file, &["hdl"]) {
            n2t_core::hdl::format_with_options(&source, &options)
        } else {
            n2t_core::tst::format_with_options(&source, &options)
        };
        let shown = display(file);
        let Some(formatted) = formatted else {
            eprintln!("skipped {shown} (contains syntax errors)");
            skipped += 1;
            continue;
        };
        if formatted != source {
            changed += 1;
            if check_only {
                println!("would format {shown}");
            } else {
                std::fs::write(file, &formatted)
                    .map_err(|e| format!("cannot write `{}`: {e}", file.display()))?;
                println!("formatted {shown}");
            }
        }
    }

    if check_only {
        Ok(changed == 0 && skipped == 0)
    } else {
        Ok(skipped == 0)
    }
}
