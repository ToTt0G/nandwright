//! Producing the files the official tools produce: `.hack` from assembly,
//! `.vm` from Jack, and the reference text a student's own tool is checked
//! against.
//!
//! In projects 6, 10 and 11 the student writes an assembler, a syntax
//! analyzer and a compiler. The `.hack`, `.xml` and `.vm` files in those
//! folders are then *their* tool's output. So nothing here overwrites a file
//! without being asked to: each operation reports whether the file it would
//! write already exists with different content, and writes only with
//! `"write": true`.

use n2t_core::host::{HostError, SharedHost, path, read_text};
use n2t_core::source::LineIndex;
use serde_json::{Value, json};

use crate::{Failure, Reply, range, text};

fn read(host: &SharedHost, file: &str) -> Result<String, Failure> {
    read_text(host.as_ref(), file)
        .map(|f| f.text)
        .map_err(|e| format!("cannot read `{}`: {e}", path::file_name(file)))
}

fn same_lines(a: &str, b: &str) -> bool {
    a.lines()
        .map(str::trim_end)
        .eq(b.lines().map(str::trim_end))
}

/// `"missing"`, `"same"` or `"different"`: the file now on disk compared
/// with `text`, ignoring line endings.
fn existing(host: &SharedHost, file: &str, text: &str) -> Result<&'static str, Failure> {
    match read_text(host.as_ref(), file) {
        Ok(current) if same_lines(&current.text, text) => Ok("same"),
        Ok(_) => Ok("different"),
        Err(HostError::NotFound) => Ok("missing"),
        Err(e) => Err(format!("cannot read `{}`: {e}", path::file_name(file))),
    }
}

fn write(host: &SharedHost, file: &str, text: &str, crlf: bool) -> Result<(), Failure> {
    let data = if crlf {
        text.replace('\n', "\r\n")
    } else {
        text.to_string()
    };
    host.write(file, data.as_bytes())
        .map_err(|e| format!("cannot write `{}`: {e}", path::file_name(file)))
}

fn with_extension(file: &str, extension: &str) -> String {
    let dir = path::dir(file);
    path::join(dir, &format!("{}.{extension}", path::stem(file)))
}

/// `{"op":"asm.assemble","path": X.asm, "write": bool, "crlf": bool}`.
pub fn assemble(host: &SharedHost, request: &Value) -> Reply {
    let file = path::normalize(text(request, "path")?);
    let source = read(host, &file)?;
    let assembly = n2t_hack::assemble(&source);
    let lines = LineIndex::new(&source);
    let problems: Vec<Value> = assembly
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| {
            json!({
                "path": file,
                "message": d.message,
                "range": d.span.map(|span| range(&source, &lines, span)),
            })
        })
        .collect();
    let out = with_extension(&file, "hack");
    if !problems.is_empty() {
        return Ok(json!({ "outputs": [], "problems": problems, "written": false }));
    }
    let hack = n2t_hack::hack_text(&assembly.words);
    let state = existing(host, &out, &hack)?;
    let wanted = request["write"].as_bool().unwrap_or(false);
    if wanted {
        write(
            host,
            &out,
            &hack,
            request["crlf"].as_bool().unwrap_or(false),
        )?;
    }
    Ok(json!({
        "outputs": [{ "path": out, "text": hack, "existing": state }],
        "problems": [],
        "written": wanted,
    }))
}

fn jack_folder(host: &SharedHost, target: &str) -> Result<Vec<(String, String, String)>, Failure> {
    let dir = if path::has_extension(target, "jack") {
        path::dir(target).to_string()
    } else {
        target.to_string()
    };
    let mut files: Vec<String> = host
        .list(&dir)
        .map_err(|e| format!("cannot read the folder `{}`: {e}", path::file_name(&dir)))?
        .into_iter()
        .filter(|e| !e.is_dir && path::has_extension(&e.name, "jack"))
        .map(|e| path::join(&dir, &e.name))
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(format!(
            "there are no `.jack` files in `{}`",
            path::file_name(&dir)
        ));
    }
    files
        .into_iter()
        .map(|file| Ok((path::stem(&file).to_string(), read(host, &file)?, file)))
        .collect()
}

/// `{"op":"jack.compile","path": folder or X.jack, "write": bool, "crlf": bool}`:
/// compiles the Jack classes of a folder, as the official JackCompiler does
/// for a folder. Nothing is written unless every class compiles.
pub fn jack_compile(host: &SharedHost, request: &Value) -> Reply {
    let target = path::normalize(text(request, "path")?);
    let classes = jack_folder(host, &target)?;
    let inputs: Vec<(&str, &str)> = classes
        .iter()
        .map(|(stem, source, _)| (stem.as_str(), source.as_str()))
        .collect();
    let program = n2t_jack::compile_program(&inputs);
    let mut problems = Vec::new();
    for (output, (_, source, file)) in program.files.iter().zip(&classes) {
        let lines = LineIndex::new(source);
        for d in output.compiled.diagnostics.iter().filter(|d| d.is_error()) {
            problems.push(json!({
                "path": file,
                "message": d.message,
                "range": d.span.map(|span| range(source, &lines, span)),
            }));
        }
    }
    if !problems.is_empty() {
        return Ok(json!({ "outputs": [], "problems": problems, "written": false }));
    }
    let mut outputs = Vec::new();
    let wanted = request["write"].as_bool().unwrap_or(false);
    let crlf = request["crlf"].as_bool().unwrap_or(false);
    for (output, (_, _, file)) in program.files.iter().zip(&classes) {
        let Some(vm) = &output.compiled.vm else {
            continue;
        };
        let out = with_extension(file, "vm");
        let state = existing(host, &out, vm)?;
        if wanted {
            write(host, &out, vm, crlf)?;
        }
        outputs.push(json!({ "path": out, "text": vm, "existing": state }));
    }
    Ok(json!({ "outputs": outputs, "problems": [], "written": wanted }))
}

/// `{"op":"reference","path": file}`: what the official tools would have
/// produced for a file that a student's own tool wrote.
///
/// - `Prog.hack`: `Prog.asm` assembled (project 6);
/// - `Class.vm`: `Class.jack` compiled, with the folder's other classes (project 11);
/// - `ClassT.xml`: the tokens of `Class.jack`; `Class.xml`: its parse tree (project 10).
pub fn reference(host: &SharedHost, request: &Value) -> Reply {
    let file = path::normalize(text(request, "path")?);
    let name = path::file_name(&file).to_string();
    let stem = path::stem(&file).to_string();
    let missing = |source: &str, tool: &str| {
        format!(
            "`{name}` is compared with what {tool} makes of `{source}`, but there is no `{source}` next to it"
        )
    };
    let first_error = |source_name: &str, source: &str, diagnostics: &[n2t_core::Diagnostic]| {
        let detail = diagnostics
            .iter()
            .find(|d| d.is_error())
            .map_or_else(String::new, |d| {
                let at = d.span.map_or(String::new(), |span| {
                    let (line, column) =
                        LineIndex::new(source).line_col_one_based(source, span.start);
                    format!(":{line}:{column}")
                });
                format!("\n  {source_name}{at}: {}", d.message)
            });
        format!("`{source_name}` has an error, so there is no reference output:{detail}")
    };
    let exists = |candidate: &str| host.read(candidate).is_ok();

    if path::has_extension(&file, "hack") {
        let source_file = with_extension(&file, "asm");
        if !exists(&source_file) {
            return Err(missing(&format!("{stem}.asm"), "the assembler"));
        }
        let source = read(host, &source_file)?;
        let assembly = n2t_hack::assemble(&source);
        if assembly.diagnostics.iter().any(|d| d.is_error()) {
            return Err(first_error(
                path::file_name(&source_file),
                &source,
                &assembly.diagnostics,
            ));
        }
        return Ok(json!({
            "text": n2t_hack::hack_text(&assembly.words),
            "from": source_file,
            "tool": "the assembler",
        }));
    }
    if path::has_extension(&file, "vm") {
        let source_file = with_extension(&file, "jack");
        if !exists(&source_file) {
            return Err(missing(&format!("{stem}.jack"), "the Jack compiler"));
        }
        let classes = jack_folder(host, &source_file)?;
        let inputs: Vec<(&str, &str)> = classes
            .iter()
            .map(|(s, source, _)| (s.as_str(), source.as_str()))
            .collect();
        // A compiler under test is compared class by class: a mistake in a
        // call to another class must not hide this class's code.
        let options = n2t_jack::Options {
            keep_vm_on_call_errors: true,
        };
        let program = n2t_jack::compile_program_with(&inputs, &options);
        let found = program
            .files
            .iter()
            .zip(&classes)
            .find(|(_, (s, _, _))| *s == stem);
        let Some((output, (_, source, _))) = found else {
            return Err(missing(&format!("{stem}.jack"), "the Jack compiler"));
        };
        return match &output.compiled.vm {
            Some(vm) => Ok(json!({ "text": vm, "from": source_file, "tool": "the Jack compiler" })),
            None => Err(first_error(
                path::file_name(&source_file),
                source,
                &output.compiled.diagnostics,
            )),
        };
    }
    if path::has_extension(&file, "xml") {
        // `MainT.xml` holds tokens, unless there is a class really named `MainT`.
        let plain = with_extension(&file, "jack");
        let (source_file, tokens) = match stem.strip_suffix('T') {
            Some(base) if !exists(&plain) => {
                (path::join(path::dir(&file), &format!("{base}.jack")), true)
            }
            _ => (plain, false),
        };
        if !exists(&source_file) {
            return Err(missing(
                path::file_name(&source_file),
                "the syntax analyzer",
            ));
        }
        let source = read(host, &source_file)?;
        let xml = if tokens {
            n2t_jack::tokens_xml(&source)
        } else {
            n2t_jack::parse_xml(&source)
        };
        return match xml {
            Ok(text) => Ok(json!({
                "text": text,
                "from": source_file,
                "tool": if tokens { "the tokenizer" } else { "the parser" },
            })),
            Err(diagnostics) => Err(first_error(
                path::file_name(&source_file),
                &source,
                &diagnostics,
            )),
        };
    }
    Err(format!(
        "`{name}` has no reference output; this works for `.hack`, `.vm` and `.xml` files written by your own assembler, compiler or analyzer"
    ))
}
