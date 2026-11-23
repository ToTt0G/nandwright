//! Language server for Nand to Tetris `.hdl` and `.tst` files.
//!
//! Every folder containing an open document is treated as a project
//! directory: chips resolve against `.hdl` files in that folder (including
//! unsaved editor buffers) and then the built-in chip library.

mod convert;
mod format;
mod hdl;
mod semantic_tokens;
mod tst;
mod workspace;

use std::error::Error;

use lsp_server::{ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types as lsp;
use lsp_types::Url;
use lsp_types::notification::{self as notif, Notification as _};
use lsp_types::request::{self as req, Request as _};
use n2t_core::host::SharedHost;
use serde_json::Value;

use crate::workspace::{DocKind, Workspace};

pub use crate::convert::{BUILTIN_SCHEME, path_to_url, url_to_path};

pub type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

/// The request that returns the text of a built-in chip's interface:
/// `{ "name": "And" }` gives `{ "text": "CHIP And { … }" }` or `null`.
pub const BUILTIN_SOURCE_REQUEST: &str = "n2t/builtinSource";

/// Runs the server over stdin/stdout until the client shuts it down.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_stdio() -> Result<()> {
    let (connection, io_threads) = lsp_server::Connection::stdio();
    run(connection, std::sync::Arc::new(n2t_core::host::StdHost))?;
    io_threads.join()?;
    Ok(())
}

/// Runs the server on an established connection.
///
/// A request that fails, or even panics, costs that one request: the client
/// gets an error response and the server keeps running. A student's half-typed
/// file must never take the language server down.
#[cfg(not(target_arch = "wasm32"))]
pub fn run(connection: lsp_server::Connection, host: SharedHost) -> Result<()> {
    let capabilities = serde_json::to_value(capabilities())?;
    let params: lsp::InitializeParams =
        serde_json::from_value(connection.initialize(capabilities)?)?;
    let mut server = Server::new(&params, host);
    while let Ok(message) = connection.receiver.recv() {
        if let Message::Request(request) = &message
            && connection.handle_shutdown(request)?
        {
            return Ok(());
        }
        let request_id = match &message {
            Message::Request(request) => Some(request.id.clone()),
            _ => None,
        };
        let handled =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| server.handle(message)));
        let failure = match handled {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(e.to_string()),
            Err(_) => Some("the language server hit an internal error".to_string()),
        };
        for message in server.take_outgoing() {
            if connection.sender.send(message).is_err() {
                return Ok(());
            }
        }
        if let (Some(message), Some(id)) = (failure, request_id) {
            let response = Response::new_err(id, ErrorCode::InternalError as i32, message);
            if connection.sender.send(Message::Response(response)).is_err() {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// A server driven by JSON text instead of a connection: one JSON-RPC message
/// in, any number out. This is the entry point of the WebAssembly build,
/// where the editor's Node process owns the transport. It performs the
/// `initialize` / `shutdown` handshake itself.
pub struct JsonServer {
    host: SharedHost,
    server: Option<Server>,
}

impl JsonServer {
    pub fn new(host: SharedHost) -> Self {
        Self { host, server: None }
    }

    /// Handles one JSON-RPC message and returns the messages to send back.
    /// Never panics on malformed input.
    pub fn handle(&mut self, message: &str) -> Vec<String> {
        let Ok(message) = serde_json::from_str::<Message>(message) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        match message {
            Message::Request(request) if request.method == req::Initialize::METHOD => {
                let params: lsp::InitializeParams =
                    serde_json::from_value(request.params).unwrap_or_default();
                self.server = Some(Server::new(&params, self.host.clone()));
                let result = lsp::InitializeResult {
                    capabilities: capabilities(),
                    server_info: Some(lsp::ServerInfo {
                        name: "nandwright".to_string(),
                        version: Some(env!("CARGO_PKG_VERSION").to_string()),
                    }),
                };
                out.push(Message::Response(Response::new_ok(request.id, result)));
            }
            Message::Request(request) if request.method == req::Shutdown::METHOD => {
                out.push(Message::Response(Response::new_ok(request.id, Value::Null)));
            }
            message => match &mut self.server {
                Some(server) => {
                    let request_id = match &message {
                        Message::Request(request) => Some(request.id.clone()),
                        _ => None,
                    };
                    let failed = server.handle(message).err();
                    out.extend(server.take_outgoing());
                    if let (Some(e), Some(id)) = (failed, request_id) {
                        out.push(Message::Response(Response::new_err(
                            id,
                            ErrorCode::InternalError as i32,
                            e.to_string(),
                        )));
                    }
                }
                None => {
                    if let Message::Request(request) = message {
                        out.push(Message::Response(Response::new_err(
                            request.id,
                            ErrorCode::ServerNotInitialized as i32,
                            "the server has not been initialized".to_string(),
                        )));
                    }
                }
            },
        }
        out.iter()
            .filter_map(|m| serde_json::to_string(m).ok())
            .collect()
    }
}

/// The capabilities advertised in the `initialize` response.
pub fn capabilities() -> lsp::ServerCapabilities {
    lsp::ServerCapabilities {
        text_document_sync: Some(lsp::TextDocumentSyncCapability::Options(
            lsp::TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(lsp::TextDocumentSyncKind::FULL),
                save: Some(lsp::TextDocumentSyncSaveOptions::Supported(true)),
                ..Default::default()
            },
        )),
        hover_provider: Some(lsp::HoverProviderCapability::Simple(true)),
        definition_provider: Some(lsp::OneOf::Left(true)),
        references_provider: Some(lsp::OneOf::Left(true)),
        document_highlight_provider: Some(lsp::OneOf::Left(true)),
        document_symbol_provider: Some(lsp::OneOf::Left(true)),
        rename_provider: Some(lsp::OneOf::Right(lsp::RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: Default::default(),
        })),
        completion_provider: Some(lsp::CompletionOptions {
            trigger_characters: Some(vec!["(".into(), ",".into(), "=".into(), " ".into()]),
            ..Default::default()
        }),
        document_formatting_provider: Some(lsp::OneOf::Left(true)),
        document_range_formatting_provider: Some(lsp::OneOf::Left(true)),
        inlay_hint_provider: Some(lsp::OneOf::Left(true)),
        semantic_tokens_provider: Some(
            lsp::SemanticTokensServerCapabilities::SemanticTokensOptions(
                lsp::SemanticTokensOptions {
                    legend: semantic_tokens::legend(),
                    full: Some(lsp::SemanticTokensFullOptions::Bool(true)),
                    range: None,
                    ..Default::default()
                },
            ),
        ),
        ..Default::default()
    }
}

/// A transport-independent server: feed it messages with [`Server::handle`]
/// and collect its replies with [`Server::take_outgoing`]. The
/// `initialize`/`shutdown` handshake is left to the transport.
pub struct Server {
    workspace: Workspace,
    snippets: bool,
    outbox: Vec<Message>,
}

impl Server {
    pub fn new(params: &lsp::InitializeParams, host: SharedHost) -> Self {
        let snippets = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|t| t.completion.as_ref())
            .and_then(|c| c.completion_item.as_ref())
            .and_then(|i| i.snippet_support)
            .unwrap_or(false);
        Self {
            workspace: Workspace::new(host),
            snippets,
            outbox: Vec::new(),
        }
    }

    pub fn handle(&mut self, message: Message) -> Result<()> {
        match message {
            Message::Request(request) => self.request(request),
            Message::Notification(notification) => self.notification(notification),
            Message::Response(_) => Ok(()),
        }
    }

    /// Messages produced since the last call.
    pub fn take_outgoing(&mut self) -> Vec<Message> {
        std::mem::take(&mut self.outbox)
    }

    fn send(&mut self, message: Message) -> Result<()> {
        self.outbox.push(message);
        Ok(())
    }

    fn respond(
        &mut self,
        id: RequestId,
        result: std::result::Result<Value, (ErrorCode, String)>,
    ) -> Result<()> {
        let response = match result {
            Ok(value) => Response::new_ok(id, value),
            Err((code, message)) => Response::new_err(id, code as i32, message),
        };
        self.send(Message::Response(response))
    }

    fn request(&mut self, request: Request) -> Result<()> {
        let id = request.id.clone();
        let result = self.dispatch(request);
        self.respond(id, result)
    }

    fn dispatch(&mut self, request: Request) -> std::result::Result<Value, (ErrorCode, String)> {
        fn params<R: req::Request>(
            request: Request,
        ) -> std::result::Result<R::Params, (ErrorCode, String)> {
            serde_json::from_value(request.params)
                .map_err(|e| (ErrorCode::InvalidParams, e.to_string()))
        }
        fn ok<T: serde::Serialize>(value: T) -> std::result::Result<Value, (ErrorCode, String)> {
            serde_json::to_value(value).map_err(|e| (ErrorCode::InternalError, e.to_string()))
        }
        let ws = &mut self.workspace;
        let kind = |ws: &Workspace, uri: &Url| ws.docs.get(uri).map(|d| d.kind);

        match request.method.as_str() {
            req::HoverRequest::METHOD => {
                let p = params::<req::HoverRequest>(request)?;
                let uri = p.text_document_position_params.text_document.uri;
                let position = p.text_document_position_params.position;
                ok(match kind(ws, &uri) {
                    Some(DocKind::Hdl) => hdl::hover(ws, &uri, position),
                    Some(DocKind::Tst) => tst::hover(ws, &uri, position),
                    _ => None,
                })
            }
            req::GotoDefinition::METHOD => {
                let p = params::<req::GotoDefinition>(request)?;
                let uri = p.text_document_position_params.text_document.uri;
                let position = p.text_document_position_params.position;
                let location = match kind(ws, &uri) {
                    Some(DocKind::Hdl) => hdl::definition(ws, &uri, position),
                    Some(DocKind::Tst) => tst::definition(ws, &uri, position),
                    _ => None,
                };
                ok(location.map(lsp::GotoDefinitionResponse::Scalar))
            }
            req::References::METHOD => {
                let p = params::<req::References>(request)?;
                let uri = p.text_document_position.text_document.uri;
                let position = p.text_document_position.position;
                ok(match kind(ws, &uri) {
                    Some(DocKind::Hdl) => {
                        hdl::references(ws, &uri, position, p.context.include_declaration)
                    }
                    _ => Vec::new(),
                })
            }
            req::DocumentHighlightRequest::METHOD => {
                let p = params::<req::DocumentHighlightRequest>(request)?;
                let uri = p.text_document_position_params.text_document.uri;
                let position = p.text_document_position_params.position;
                ok(match kind(ws, &uri) {
                    Some(DocKind::Hdl) => hdl::highlights(ws, &uri, position),
                    _ => Vec::new(),
                })
            }
            req::DocumentSymbolRequest::METHOD => {
                let p = params::<req::DocumentSymbolRequest>(request)?;
                let uri = p.text_document.uri;
                ok(match kind(ws, &uri) {
                    Some(DocKind::Hdl) => Some(lsp::DocumentSymbolResponse::Nested(
                        hdl::document_symbols(ws, &uri),
                    )),
                    _ => None,
                })
            }
            req::PrepareRenameRequest::METHOD => {
                let p = params::<req::PrepareRenameRequest>(request)?;
                let uri = p.text_document.uri;
                ok(match kind(ws, &uri) {
                    Some(DocKind::Hdl) => hdl::prepare_rename(ws, &uri, p.position),
                    _ => None,
                })
            }
            req::Rename::METHOD => {
                let p = params::<req::Rename>(request)?;
                let uri = p.text_document_position.text_document.uri;
                let position = p.text_document_position.position;
                match kind(ws, &uri) {
                    Some(DocKind::Hdl) => match hdl::rename(ws, &uri, position, &p.new_name) {
                        Ok(edit) => ok(edit),
                        Err(message) => Err((ErrorCode::RequestFailed, message)),
                    },
                    _ => ok(None::<lsp::WorkspaceEdit>),
                }
            }
            req::Completion::METHOD => {
                let p = params::<req::Completion>(request)?;
                let uri = p.text_document_position.text_document.uri;
                let position = p.text_document_position.position;
                let snippets = self.snippets;
                let items = match kind(ws, &uri) {
                    Some(DocKind::Hdl) => hdl::completion(ws, &uri, position, snippets),
                    Some(DocKind::Tst) => tst::completion(ws, &uri, position, snippets),
                    _ => Vec::new(),
                };
                ok(lsp::CompletionResponse::Array(items))
            }
            req::Formatting::METHOD => {
                let p = params::<req::Formatting>(request)?;
                ok(format::document(ws, &p.text_document.uri, &p.options))
            }
            req::RangeFormatting::METHOD => {
                let p = params::<req::RangeFormatting>(request)?;
                ok(format::range(ws, &p.text_document.uri, p.range, &p.options))
            }
            req::InlayHintRequest::METHOD => {
                let p = params::<req::InlayHintRequest>(request)?;
                let uri = p.text_document.uri;
                ok(match kind(ws, &uri) {
                    Some(DocKind::Hdl) => hdl::inlay_hints(ws, &uri, p.range),
                    _ => Vec::new(),
                })
            }
            req::SemanticTokensFullRequest::METHOD => {
                let p = params::<req::SemanticTokensFullRequest>(request)?;
                ok(semantic_tokens::full(ws, &p.text_document.uri)
                    .map(lsp::SemanticTokensResult::Tokens))
            }
            BUILTIN_SOURCE_REQUEST => {
                let name = request
                    .params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                ok(ws
                    .builtins
                    .get(name)
                    .map(|chip| serde_json::json!({ "text": &*chip.source })))
            }
            other => Err((
                ErrorCode::MethodNotFound,
                format!("unhandled request `{other}`"),
            )),
        }
    }

    fn notification(&mut self, notification: Notification) -> Result<()> {
        match notification.method.as_str() {
            notif::DidOpenTextDocument::METHOD => {
                let p: lsp::DidOpenTextDocumentParams =
                    serde_json::from_value(notification.params)?;
                let uri = p.text_document.uri;
                self.workspace
                    .open(uri.clone(), p.text_document.text, p.text_document.version);
                self.refresh_around(&uri)?;
            }
            notif::DidChangeTextDocument::METHOD => {
                let p: lsp::DidChangeTextDocumentParams =
                    serde_json::from_value(notification.params)?;
                let uri = p.text_document.uri;
                if let Some(change) = p.content_changes.into_iter().last() {
                    self.workspace
                        .change(&uri, change.text, p.text_document.version);
                    self.refresh_around(&uri)?;
                }
            }
            notif::DidSaveTextDocument::METHOD => {
                let p: lsp::DidSaveTextDocumentParams =
                    serde_json::from_value(notification.params)?;
                self.refresh_around(&p.text_document.uri)?;
            }
            notif::DidCloseTextDocument::METHOD => {
                let p: lsp::DidCloseTextDocumentParams =
                    serde_json::from_value(notification.params)?;
                let uri = p.text_document.uri;
                let dir = self.workspace.docs.get(&uri).map(|d| d.dir.clone());
                self.workspace.close(&uri);
                self.publish(&uri, Vec::new(), None)?;
                if let Some(dir) = dir {
                    self.refresh_dir(&dir)?;
                }
            }
            notif::DidChangeWatchedFiles::METHOD => {
                let dirs: Vec<_> = self.workspace.projects.keys().cloned().collect();
                for dir in dirs {
                    self.refresh_dir(&dir)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Re-publishes diagnostics for every open document in the same folder
    /// as `uri` (an edit to one chip can affect its users and scripts).
    fn refresh_around(&mut self, uri: &Url) -> Result<()> {
        let Some(dir) = self.workspace.docs.get(uri).map(|d| d.dir.clone()) else {
            return Ok(());
        };
        self.refresh_dir(&dir)
    }

    fn refresh_dir(&mut self, dir: &str) -> Result<()> {
        self.workspace.invalidate_scripts(dir);
        for uri in self.workspace.docs_in(dir) {
            let diagnostics = self.workspace.diagnostics(&uri);
            let version = self.workspace.docs.get(&uri).map(|d| d.version);
            self.publish(&uri, diagnostics, version)?;
        }
        Ok(())
    }

    fn publish(
        &mut self,
        uri: &Url,
        diagnostics: Vec<lsp::Diagnostic>,
        version: Option<i32>,
    ) -> Result<()> {
        let params = lsp::PublishDiagnosticsParams {
            uri: uri.clone(),
            diagnostics,
            version,
        };
        self.send(Message::Notification(Notification::new(
            notif::PublishDiagnostics::METHOD.to_string(),
            params,
        )))
    }
}
