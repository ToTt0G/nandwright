//! `jackc`: compile a `.jack` file or a directory of them to `.vm` files.
//!
//! ```text
//! cargo run -p n2t-jack --example jackc -- [options] <FILE.jack | DIR>...
//!
//!   --out DIR       write .vm files into DIR instead of next to the sources
//!   --check         compile but do not write .vm files
//!   --lenient       like the official compiler, still write the .vm file when
//!                   the only errors are in call checks
//!   --report        machine-readable output for the conformance harness:
//!                   one tab-separated line per diagnostic:
//!                   DIAG <file> <severity> <official line> <line> <message>
//!                   and one per file: FILE <file> <ok|failed>
//! ```
//!
//! A file argument is compiled on its own (calls to other classes are not
//! checked); a directory is compiled as one program. Diagnostics are printed as
//! `file:line:col: severity: message`. The exit status is 1 if any file has errors.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use n2t_jack::{Compiled, Options};

struct Args {
    out: Option<PathBuf>,
    check: bool,
    report: bool,
    options: Options,
    inputs: Vec<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        out: None,
        check: false,
        report: false,
        options: Options::default(),
        inputs: Vec::new(),
    };
    let mut it = std::env::args_os().skip(1);
    while let Some(a) = it.next() {
        match a.to_str() {
            Some("--out") => {
                args.out = Some(it.next().ok_or("--out needs a directory")?.into());
            }
            Some("--check") => args.check = true,
            Some("--report") => args.report = true,
            Some("--lenient") => args.options.keep_vm_on_call_errors = true,
            Some("-h" | "--help") => {
                return Err("usage: jackc [--out DIR] [--check] [--lenient] [--report] \
                            <FILE.jack | DIR>..."
                    .into());
            }
            _ => args.inputs.push(a.into()),
        }
    }
    if args.inputs.is_empty() {
        return Err("usage: jackc [--out DIR] [--check] [--lenient] [--report] <FILE.jack | DIR>..."
            .into());
    }
    Ok(args)
}

fn read(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    // The official tools read source as UTF-8 on this platform; invalid
    // sequences become U+FFFD.
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let mut failed = false;
    for input in &args.inputs {
        let mut files: Vec<PathBuf> = if input.is_dir() {
            match std::fs::read_dir(input) {
                Ok(rd) => rd
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().is_some_and(|x| x == "jack") && p.is_file())
                    .collect(),
                Err(e) => {
                    eprintln!("{}: {e}", input.display());
                    failed = true;
                    continue;
                }
            }
        } else {
            vec![input.clone()]
        };
        files.sort();
        let mut sources = Vec::new();
        for f in &files {
            match read(f) {
                Ok(s) => sources.push((f.clone(), stem(f), s)),
                Err(e) => {
                    eprintln!("{e}");
                    failed = true;
                }
            }
        }
        let results: Vec<Compiled> = if input.is_dir() {
            let pairs: Vec<(&str, &str)> = sources
                .iter()
                .map(|(_, s, src)| (s.as_str(), src.as_str()))
                .collect();
            let out = n2t_jack::compile_program_with(&pairs, &args.options);
            for d in &out.diagnostics {
                if !args.report {
                    eprintln!("{}: {}: {}", input.display(), d.severity, d.message);
                }
            }
            out.files.into_iter().map(|f| f.compiled).collect()
        } else {
            sources
                .iter()
                .map(|(_, s, src)| n2t_jack::compile_with(s, src, &args.options))
                .collect()
        };
        for ((path, stem, src), compiled) in sources.iter().zip(results) {
            let name = path.display().to_string();
            for (d, official) in compiled.diagnostics.iter().zip(&compiled.official_lines) {
                if args.report {
                    let line = d.span.map_or(0, |s| {
                        n2t_core::LineIndex::new(src).position(s.start).line + 1
                    });
                    println!(
                        "DIAG\t{}\t{}\t{}\t{}\t{}",
                        name,
                        d.severity,
                        official,
                        line,
                        d.message.replace(['\t', '\n'], " ")
                    );
                } else {
                    eprintln!("{}", n2t_jack::format_diagnostic(&name, src, d));
                }
            }
            if compiled.has_errors() {
                failed = true;
            }
            if args.report {
                println!(
                    "FILE\t{}\t{}",
                    name,
                    if compiled.has_errors() { "failed" } else { "ok" }
                );
            }
            let Some(vm) = compiled.vm else { continue };
            if args.check {
                continue;
            }
            let dir = match &args.out {
                Some(d) => d.clone(),
                None => path.parent().map(Path::to_path_buf).unwrap_or_default(),
            };
            let target = dir.join(format!("{stem}.vm"));
            if let Err(e) = std::fs::write(&target, vm) {
                eprintln!("{}: {e}", target.display());
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
