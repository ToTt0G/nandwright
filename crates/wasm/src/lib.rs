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
//! | `start` | sets the first session number of a fresh engine, so that numbers are never reused after a restart |
//! | `overlay.set`, `overlay.clear`, `overlay.replace` | unsaved editor buffers that shadow files |
//! | `test.open`, `test.run`, `test.step`, `test.inspect` | a `.tst` script run |
//! | `chip.open`, `chip.set`, `chip.eval`, `chip.tick`, `chip.tock`, `chip.reset`, `chip.state` | an interactive chip |
//! | `cpu.open`, `cpu.run`, `cpu.step`, `cpu.reset`, `cpu.restart`, `cpu.set`, `cpu.state` | a Hack computer running a program |
//! | `vm.open`, `vm.run`, `vm.step`, `vm.reset`, `vm.set`, `vm.state` | a VM program (or a Jack folder, compiled in memory) on the VM emulator |
//! | `asm.assemble` | assembles a `.asm` file to a `.hack` file |
//! | `jack.compile` | compiles the Jack classes of a folder to `.vm` files |
//! | `reference` | what the official tool would produce for a `.hack`, `.vm` or `.xml` file a student's own tool wrote |
//! | `key` | presses a key on a session's keyboard |
//! | `close` | ends a session |

#[cfg(target_arch = "wasm32")]
pub mod abi;
mod chip;
mod computer;
mod outputs;
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
use crate::overlay::{Buffer, OverlayHost};
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
            "start" => {
                // Sessions of an engine that was replaced after a crash must
                // not be mistaken for sessions of this one.
                if self.sessions.is_empty() && self.next_session == 1 {
                    self.next_session = request["firstSession"]
                        .as_u64()
                        .filter(|first| (1..=MAX_FIRST_SESSION).contains(first))
                        .ok_or("`firstSession` must be a positive whole number")?;
                }
                Ok(json!({}))
            }
            "overlay.set" => {
                self.overlay.set(buffer(request)?);
                Ok(json!({}))
            }
            "overlay.replace" => {
                let files = request["files"]
                    .as_array()
                    .ok_or("the request needs a `files` list")?;
                let buffers = files.iter().map(buffer).collect::<Result<Vec<_>, _>>()?;
                self.overlay.replace(buffers);
                Ok(json!({}))
            }
            "overlay.clear" => {
                match request["path"].as_str() {
                    Some(path) => self.overlay.clear(path),
                    None => self.overlay.replace(Vec::new()),
                }
                Ok(json!({}))
            }
            "test.open" => {
                // A run sees the editor as it was when the run began.
                let frozen: SharedHost = Arc::new(self.overlay.snapshot());
                let session = TestSession::open(&frozen, request)?;
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
            "jack.compile" => outputs::jack_compile(&self.host, self.overlay.disk(), request),
            "asm.assemble" => outputs::assemble(&self.host, self.overlay.disk(), request),
            "reference" => outputs::reference(&self.host, request),
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

/// The most commands or instructions one request may run. A caller that
/// wants more asks again; nothing a request says can make a call endless.
pub(crate) const MAX_BUDGET: u64 = 50_000_000;

/// Session numbers stay exact as JSON numbers (doubles).
const MAX_FIRST_SESSION: u64 = 1 << 52;

/// A `budget` or `count` field, capped at [`MAX_BUDGET`].
pub(crate) fn budget(request: &Value, field: &str, default: u64) -> u64 {
    request[field].as_u64().unwrap_or(default).min(MAX_BUDGET)
}

/// The `code` of a key: 0 (no key) to 65535, as the keyboard's memory cell
/// holds it. Anything else is a mistake in the request, not a key.
pub(crate) fn key_code(request: &Value) -> Result<u16, Failure> {
    request["code"]
        .as_u64()
        .and_then(|code| u16::try_from(code).ok())
        .ok_or_else(|| "a key `code` must be a whole number from 0 to 65535".to_string())
}

fn buffer(request: &Value) -> Result<Buffer, Failure> {
    Ok(Buffer::new(
        text(request, "path")?,
        text(request, "text")?,
        request["ignoreCase"].as_bool(),
    ))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_request_does_a_bounded_amount_of_work() {
        assert_eq!(
            budget(&json!({"budget": u64::MAX}), "budget", 7),
            MAX_BUDGET
        );
        assert_eq!(budget(&json!({"count": 12}), "count", 1), 12);
        assert_eq!(budget(&json!({}), "count", 1), 1);
        assert_eq!(budget(&json!({"count": -3}), "count", 1), 1);
    }

    #[test]
    fn key_codes_are_checked_not_truncated() {
        assert_eq!(key_code(&json!({"code": 0})), Ok(0));
        assert_eq!(key_code(&json!({"code": 65535})), Ok(65535));
        assert!(key_code(&json!({"code": 65536})).is_err());
        assert!(key_code(&json!({"code": 65611})).is_err());
        assert!(key_code(&json!({})).is_err());
    }
}
