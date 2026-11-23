//! The Nandwright engine behind one call: JSON in, JSON out.
//!
//! [`Engine::call`] is the whole API. It is what the WebAssembly build
//! exports (see [`abi`]) and what the tests of this crate drive natively, so
//! the code the editor runs is the code the tests run.
//!
//! Requests are objects with an `op`. A failed request answers
//! `{"error": {"message": …}}`; nothing here panics on bad input.
//!
//! | op | does |
//! |---|---|
//! | `lsp` | feeds one JSON-RPC message to the language server |
//! | `overlay.set`, `overlay.clear` | unsaved editor buffers that shadow files |
//! | `test.open`, `test.run`, `test.step`, `test.inspect` | a `.tst` script run |
//! | `chip.open`, `chip.set`, `chip.eval`, `chip.tick`, `chip.tock`, `chip.reset`, `chip.state` | an interactive chip |
//! | `cpu.open`, `cpu.run`, `cpu.step`, `cpu.reset`, `cpu.restart`, `cpu.set`, `cpu.state` | a Hack computer running a program |
//! | `vm.open`, `vm.run`, `vm.step`, `vm.reset`, `vm.set`, `vm.state` | a VM program (or a Jack folder, compiled in memory) on the VM emulator |
//! | `jack.compile` | compiles the Jack classes of a folder to `.vm` files |
//! | `key` | presses a key on a session's keyboard |
//! | `close` | ends a session |

#[cfg(target_arch = "wasm32")]
pub mod abi;
mod chip;
mod computer;
mod overlay;
mod test;
mod vmsession;

use std::collections::HashMap;
use std::sync::Arc;

use n2t_core::host::SharedHost;
use n2t_core::source::{LineIndex, Span};
use n2t_lsp::JsonServer;
use serde_json::{Value, json};

use crate::chip::ChipSession;
use crate::computer::ComputerSession;
use crate::overlay::OverlayHost;
use crate::test::TestSession;
use crate::vmsession::VmSession;

/// A request failure, as a sentence for the user.
pub(crate) type Failure = String;
pub(crate) type Reply = Result<Value, Failure>;

enum Session {
    Test(Box<TestSession>),
    Chip(Box<ChipSession>),
    Computer(Box<ComputerSession>),
    Vm(Box<VmSession>),
}

pub struct Engine {
    host: SharedHost,
    overlay: Arc<OverlayHost>,
    lsp: JsonServer,
    sessions: HashMap<u64, Session>,
    next_session: u64,
}

impl Engine {
    pub fn new(host: SharedHost) -> Self {
        let overlay = Arc::new(OverlayHost::new(host));
        let host: SharedHost = overlay.clone();
        Self {
            lsp: JsonServer::new(host.clone()),
            host,
            overlay,
            sessions: HashMap::new(),
            next_session: 1,
        }
    }

    /// Handles one request. Always returns a JSON object.
    pub fn call(&mut self, request: &str) -> String {
        let reply = match serde_json::from_str::<Value>(request) {
            Ok(request) => self.dispatch(&request),
            Err(e) => Err(format!("the request is not valid JSON: {e}")),
        };
        match reply {
            Ok(value) => value.to_string(),
            Err(message) => json!({ "error": { "message": message } }).to_string(),
        }
    }

    fn dispatch(&mut self, request: &Value) -> Reply {
        let op = request["op"].as_str().unwrap_or_default();
        match op {
            "version" => Ok(json!({ "version": env!("CARGO_PKG_VERSION") })),
            "lsp" => {
                let message = text(request, "message")?;
                Ok(json!({ "messages": self.lsp.handle(message) }))
            }
            "overlay.set" => {
                self.overlay
                    .set(text(request, "path")?, text(request, "text")?);
                Ok(json!({}))
            }
            "overlay.clear" => {
                match request["path"].as_str() {
                    Some(path) => self.overlay.clear(path),
                    None => self.overlay.clear_all(),
                }
                Ok(json!({}))
            }
            "test.open" => {
                let session = TestSession::open(&self.host, request)?;
                let mut reply = session.describe();
                reply["session"] = json!(self.add(Session::Test(Box::new(session))));
                Ok(reply)
            }
            "chip.open" => {
                let session = ChipSession::open(&self.host, request)?;
                let mut reply = session.state();
                reply["session"] = json!(self.add(Session::Chip(Box::new(session))));
                Ok(reply)
            }
            "cpu.open" => {
                let mut session = ComputerSession::open(&self.host, request)?;
                let mut reply = session.state("paused", request);
                reply["session"] = json!(self.add(Session::Computer(Box::new(session))));
                Ok(reply)
            }
            "vm.open" => {
                let mut session = VmSession::open(&self.host, request)?;
                let mut reply = session.state("paused", request);
                reply["session"] = json!(self.add(Session::Vm(Box::new(session))));
                Ok(reply)
            }
            "jack.compile" => jack_compile(&self.host, request),
            "close" => {
                self.sessions
                    .remove(&request["session"].as_u64().unwrap_or(0));
                Ok(json!({}))
            }
            _ => {
                let id = request["session"]
                    .as_u64()
                    .ok_or_else(|| format!("unknown operation `{op}`"))?;
                let session = self
                    .sessions
                    .get_mut(&id)
                    .ok_or("this session has ended; start it again")?;
                match session {
                    Session::Test(test) => test.call(op, request),
                    Session::Chip(chip) => chip.call(op, request),
                    Session::Computer(computer) => computer.call(op, request),
                    Session::Vm(vm) => vm.call(op, request),
                }
            }
        }
    }

    fn add(&mut self, session: Session) -> u64 {
        let id = self.next_session;
        self.next_session += 1;
        self.sessions.insert(id, session);
        id
    }

    /// The number of live sessions, for tests and leak checks.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }
}

pub(crate) fn text<'a>(request: &'a Value, field: &str) -> Result<&'a str, Failure> {
    request[field]
        .as_str()
        .ok_or_else(|| format!("the request needs a `{field}` string"))
}

/// A span as an editor range: zero-based lines and UTF-16 columns.
pub(crate) fn range(source: &str, lines: &LineIndex, span: Span) -> Value {
    let start = lines.position_utf16(source, span.start);
    let end = lines.position_utf16(source, span.end);
    json!({
        "startLine": start.line,
        "startCharacter": start.column,
        "endLine": end.line,
        "endCharacter": end.column,
    })
}

/// A pin, register or memory value for the UI. Values wider than 32 bits are
/// sent as decimal strings, because JSON numbers are doubles.
pub(crate) fn observed(values: &[n2t_simulator::Observed]) -> Value {
    Value::Array(
        values
            .iter()
            .map(|o| {
                json!({
                    "name": o.name,
                    "role": o.role.as_str(),
                    "width": o.width,
                    "bits": if o.width > 32 { json!(o.bits.to_string()) } else { json!(o.bits) },
                })
            })
            .collect(),
    )
}

/// `{"op":"jack.compile","path": folder or .jack file}`: compiles the Jack
/// classes of a folder and writes a `.vm` file next to each, as the official
/// JackCompiler does. Nothing is written unless every class compiles.
fn jack_compile(host: &SharedHost, request: &Value) -> Reply {
    use n2t_core::host::{path, read_text};
    let target = path::normalize(text(request, "path")?);
    let dir = if path::has_extension(&target, "jack") {
        path::dir(&target).to_string()
    } else {
        target
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
    let mut texts = Vec::new();
    for file in &files {
        let text = read_text(host.as_ref(), file)
            .map_err(|e| format!("cannot read `{}`: {e}", path::file_name(file)))?
            .text;
        texts.push((path::stem(file).to_string(), text));
    }
    let inputs: Vec<(&str, &str)> = texts
        .iter()
        .map(|(s, t)| (s.as_str(), t.as_str()))
        .collect();
    let program = n2t_jack::compile_program(&inputs);
    let mut problems = Vec::new();
    for ((output, file), (_, source)) in program.files.iter().zip(&files).zip(&texts) {
        let lines = LineIndex::new(source);
        for d in output.compiled.diagnostics.iter().filter(|d| d.is_error()) {
            problems.push(json!({
                "path": file,
                "message": d.message,
                "range": d.span.map(|span| range(source, &lines, span)),
            }));
        }
    }
    let mut written = Vec::new();
    if problems.is_empty() {
        for (output, file) in program.files.iter().zip(&files) {
            let Some(vm) = &output.compiled.vm else {
                continue;
            };
            let out = format!("{}.vm", file.strip_suffix(".jack").unwrap_or(file));
            let data = if request["crlf"].as_bool().unwrap_or(false) {
                vm.replace('\n', "\r\n")
            } else {
                vm.clone()
            };
            host.write(&out, data.as_bytes())
                .map_err(|e| format!("cannot write `{}`: {e}", path::file_name(&out)))?;
            written.push(out);
        }
    }
    Ok(json!({ "written": written, "problems": problems }))
}
