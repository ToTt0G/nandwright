//! Producing the files the official tools produce: `.hack` from assembly,
//! `.vm` from Jack, and the reference text a student's own tool is checked
//! against.
//!
//! In projects 6, 10 and 11 the student writes an assembler, a syntax
//! analyzer and a compiler. The `.hack`, `.xml` and `.vm` files in those
//! folders are then *their* tool's output. So nothing here overwrites a file
//! without being asked to. Each operation reports, for every file it would
//! write, whether that file exists on disk with other content, and a `state`
//! token for what is there now. It writes only with `"write": true`, and it
//! replaces a file with other content only if the request hands back the
//! token it was given for that file (`"expect": {path: state}`): what gets
//! replaced is what the student was asked about, not whatever is there by
//! the time they have answered.

use n2t_core::host::{HostError, SharedHost, decode_text, path, read_text};
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

/// One file an operation produces, and what is in its place now.
struct Planned {
    path: String,
    text: String,
    /// `"missing"`, `"same"` or `"different"`: the file on disk compared
    /// with `text`, ignoring line endings.
    existing: &'static str,
    /// Identifies the bytes on disk now: `"missing"`, or a digest.
    state: String,
}

/// FNV-1a over the bytes, with their length: enough to notice that a file
/// is no longer the one that was asked about.
fn digest(data: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in data {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{}-{hash:016x}", data.len())
}

/// Looks at `path` on `disk`: the files themselves, not the editor's unsaved
/// buffers. A buffer that happens to match says nothing about the file a
/// write would replace.
fn plan(disk: &SharedHost, path: String, text: String) -> Result<Planned, Failure> {
    let (existing, state) = match disk.read(&path) {
        Ok(bytes) => {
            let state = digest(&bytes);
            let existing = if same_lines(&decode_text(bytes).text, &text) {
                "same"
            } else {
                "different"
            };
            (existing, state)
        }
        Err(HostError::NotFound) => ("missing", "missing".to_string()),
        Err(e) => return Err(format!("cannot read `{}`: {e}", path::file_name(&path))),
    };
    Ok(Planned {
        path,
        text,
        existing,
        state,
    })
}

/// Writes the planned files if the request asks for it and allows every
/// replacement, and reports what happened. Either every file that needs
/// writing is written or none is.
fn finish(disk: &SharedHost, planned: Vec<Planned>, request: &Value) -> Reply {
    let wanted = request["write"].as_bool().unwrap_or(false);
    let crlf = request["crlf"].as_bool().unwrap_or(false);
    // A file with other content is replaced only if it is still the file
    // the caller was told about.
    let conflicts: Vec<&str> = planned
        .iter()
        .filter(|p| p.existing == "different")
        .filter(|p| request["expect"][p.path.as_str()].as_str() != Some(p.state.as_str()))
        .map(|p| p.path.as_str())
        .collect();
    let write = wanted && conflicts.is_empty();
    let mut outputs = Vec::new();
    for p in &planned {
        // A file that already says the same is left alone, line endings
        // and all.
        let written = write && p.existing != "same";
        if written {
            let data = if crlf {
                p.text.replace('\n', "\r\n")
            } else {
                p.text.clone()
            };
            disk.write(&p.path, data.as_bytes())
                .map_err(|e| format!("cannot write `{}`: {e}", path::file_name(&p.path)))?;
        }
        outputs.push(json!({
            "path": p.path,
            "text": p.text,
            "existing": p.existing,
            "state": p.state,
            "written": written,
        }));
    }
    Ok(json!({
        "outputs": outputs,
        "problems": [],
        "written": write,
        "conflicts": if wanted { json!(conflicts) } else { json!([]) },
    }))
}

fn with_extension(file: &str, extension: &str) -> String {
    let dir = path::dir(file);
    path::join(dir, &format!("{}.{extension}", path::stem(file)))
}

/// `{"op":"asm.assemble","path": X.asm, "write": bool, "crlf": bool,
/// "expect": {path: state}}`. The source is read as the editor shows it;
/// the `.hack` file is compared and written on disk.
pub fn assemble(host: &SharedHost, disk: &SharedHost, request: &Value) -> Reply {
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
    if !problems.is_empty() {
        return Ok(
            json!({ "outputs": [], "problems": problems, "written": false, "conflicts": [] }),
        );
    }
    let hack = n2t_hack::hack_text(&assembly.words);
    let planned = plan(disk, with_extension(&file, "hack"), hack)?;
    finish(disk, vec![planned], request)
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

/// `{"op":"jack.compile","path": folder or X.jack, "write": bool, "crlf": bool,
/// "expect": {path: state}}`: compiles the Jack classes of a folder, as the
/// official JackCompiler does for a folder. Nothing is written unless every
/// class compiles and every replacement is allowed.
pub fn jack_compile(host: &SharedHost, disk: &SharedHost, request: &Value) -> Reply {
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
        return Ok(
            json!({ "outputs": [], "problems": problems, "written": false, "conflicts": [] }),
        );
    }
    let mut planned = Vec::new();
    for (output, (_, _, file)) in program.files.iter().zip(&classes) {
        if let Some(vm) = &output.compiled.vm {
            planned.push(plan(disk, with_extension(file, "vm"), vm.clone())?);
        }
    }
    finish(disk, planned, request)
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
