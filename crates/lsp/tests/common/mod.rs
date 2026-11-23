//! A language server on an in-memory file system, driven through
//! `Server::handle` exactly as the transports drive it.
#![allow(dead_code)]

use std::sync::Arc;

use lsp_server::{Message, Notification, Request, RequestId};
use lsp_types as lsp;
use lsp_types::Url;
use lsp_types::notification::{self as notif, Notification as _};
use lsp_types::request as req;
use n2t_core::host::MemoryHost;
use n2t_lsp::Server;
use serde_json::Value;

pub struct Harness {
    pub server: Server,
    pub host: Arc<MemoryHost>,
    next_id: i32,
}

impl Harness {
    pub fn new(host: MemoryHost) -> Self {
        let params = lsp::InitializeParams {
            capabilities: lsp::ClientCapabilities {
                text_document: Some(lsp::TextDocumentClientCapabilities {
                    completion: Some(lsp::CompletionClientCapabilities {
                        completion_item: Some(lsp::CompletionItemCapability {
                            snippet_support: Some(true),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let host = Arc::new(host);
        let server = Server::new(&params, host.clone());
        Self {
            server,
            host,
            next_id: 1,
        }
    }

    pub fn empty() -> Self {
        Self::new(MemoryHost::new())
    }

    /// Opens a document and returns the diagnostics published for it.
    pub fn open(&mut self, uri: &Url, text: &str) -> Vec<lsp::Diagnostic> {
        let params = lsp::DidOpenTextDocumentParams {
            text_document: lsp::TextDocumentItem {
                uri: uri.clone(),
                language_id: "plaintext".to_string(),
                version: 1,
                text: text.to_string(),
            },
        };
        self.server
            .handle(Message::Notification(Notification::new(
                notif::DidOpenTextDocument::METHOD.to_string(),
                params,
            )))
            .expect("didOpen failed");
        self.diagnostics_for(uri)
    }

    /// Replaces a document's text and returns its new diagnostics.
    pub fn change(&mut self, uri: &Url, text: &str) -> Vec<lsp::Diagnostic> {
        self.send_change(uri, text);
        self.diagnostics_for(uri)
    }

    /// Replaces a document's text and returns every diagnostics notification
    /// it caused (other documents of the folder are refreshed too).
    pub fn change_all(&mut self, uri: &Url, text: &str) -> Vec<(Url, Vec<lsp::Diagnostic>)> {
        self.send_change(uri, text);
        self.published()
    }

    fn send_change(&mut self, uri: &Url, text: &str) {
        let params = lsp::DidChangeTextDocumentParams {
            text_document: lsp::VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            content_changes: vec![lsp::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: text.to_string(),
            }],
        };
        self.server
            .handle(Message::Notification(Notification::new(
                notif::DidChangeTextDocument::METHOD.to_string(),
                params,
            )))
            .expect("didChange failed");
    }

    /// All diagnostics published since the last call, by document.
    pub fn published(&mut self) -> Vec<(Url, Vec<lsp::Diagnostic>)> {
        self.server
            .take_outgoing()
            .into_iter()
            .filter_map(|m| match m {
                Message::Notification(n) if n.method == notif::PublishDiagnostics::METHOD => {
                    let p: lsp::PublishDiagnosticsParams = serde_json::from_value(n.params).ok()?;
                    Some((p.uri, p.diagnostics))
                }
                _ => None,
            })
            .collect()
    }

    fn diagnostics_for(&mut self, uri: &Url) -> Vec<lsp::Diagnostic> {
        self.published()
            .into_iter()
            .rev()
            .find(|(u, _)| u == uri)
            .map(|(_, d)| d)
            .unwrap_or_default()
    }

    /// Sends a request and returns the raw response (result or error).
    pub fn raw<R: req::Request>(&mut self, params: R::Params) -> lsp_server::Response {
        let id = self.next_id;
        self.next_id += 1;
        let request = Request::new(RequestId::from(id), R::METHOD.to_string(), params);
        self.server
            .handle(Message::Request(request))
            .expect("request failed");
        self.server
            .take_outgoing()
            .into_iter()
            .find_map(|m| match m {
                Message::Response(r) if r.id == RequestId::from(id) => Some(r),
                _ => None,
            })
            .expect("no response")
    }

    pub fn request<R: req::Request>(&mut self, params: R::Params) -> R::Result {
        let response = self.raw::<R>(params);
        assert!(
            response.error.is_none(),
            "unexpected error: {:?}",
            response.error
        );
        serde_json::from_value(response.result.unwrap_or(Value::Null)).expect("bad result")
    }

    pub fn hover(&mut self, uri: &Url, pos: lsp::Position) -> Option<String> {
        let hover = self.request::<req::HoverRequest>(lsp::HoverParams {
            text_document_position_params: position(uri, pos),
            work_done_progress_params: Default::default(),
        })?;
        match hover.contents {
            lsp::HoverContents::Markup(m) => Some(m.value),
            other => panic!("unexpected hover contents {other:?}"),
        }
    }

    pub fn completion(&mut self, uri: &Url, pos: lsp::Position) -> Vec<lsp::CompletionItem> {
        match self.request::<req::Completion>(lsp::CompletionParams {
            text_document_position: position(uri, pos),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: None,
        }) {
            Some(lsp::CompletionResponse::Array(items)) => items,
            Some(lsp::CompletionResponse::List(list)) => list.items,
            None => Vec::new(),
        }
    }

    pub fn labels(&mut self, uri: &Url, pos: lsp::Position) -> Vec<String> {
        self.completion(uri, pos)
            .into_iter()
            .map(|i| i.label)
            .collect()
    }

    pub fn definition(&mut self, uri: &Url, pos: lsp::Position) -> Option<lsp::Location> {
        match self.request::<req::GotoDefinition>(lsp::GotoDefinitionParams {
            text_document_position_params: position(uri, pos),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })? {
            lsp::GotoDefinitionResponse::Scalar(l) => Some(l),
            lsp::GotoDefinitionResponse::Array(mut v) => v.pop(),
            lsp::GotoDefinitionResponse::Link(_) => panic!("unexpected links"),
        }
    }

    pub fn references(&mut self, uri: &Url, pos: lsp::Position) -> Vec<lsp::Location> {
        self.request::<req::References>(lsp::ReferenceParams {
            text_document_position: position(uri, pos),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: lsp::ReferenceContext {
                include_declaration: true,
            },
        })
        .unwrap_or_default()
    }

    pub fn highlights(&mut self, uri: &Url, pos: lsp::Position) -> Vec<lsp::DocumentHighlight> {
        self.request::<req::DocumentHighlightRequest>(lsp::DocumentHighlightParams {
            text_document_position_params: position(uri, pos),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
        .unwrap_or_default()
    }

    /// `Ok(edit)` or `Err(message)` for a refused rename.
    pub fn rename(
        &mut self,
        uri: &Url,
        pos: lsp::Position,
        new_name: &str,
    ) -> Result<Option<lsp::WorkspaceEdit>, String> {
        let response = self.raw::<req::Rename>(lsp::RenameParams {
            text_document_position: position(uri, pos),
            new_name: new_name.to_string(),
            work_done_progress_params: Default::default(),
        });
        match response.error {
            Some(e) => Err(e.message),
            None => Ok(serde_json::from_value(response.result.unwrap_or(Value::Null)).unwrap()),
        }
    }

    pub fn symbols(&mut self, uri: &Url) -> Vec<lsp::DocumentSymbol> {
        match self.request::<req::DocumentSymbolRequest>(lsp::DocumentSymbolParams {
            text_document: lsp::TextDocumentIdentifier { uri: uri.clone() },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        }) {
            Some(lsp::DocumentSymbolResponse::Nested(v)) => v,
            Some(lsp::DocumentSymbolResponse::Flat(_)) => panic!("flat symbols"),
            None => Vec::new(),
        }
    }

    pub fn signature_help(&mut self, uri: &Url, pos: lsp::Position) -> Option<lsp::SignatureHelp> {
        self.request::<req::SignatureHelpRequest>(lsp::SignatureHelpParams {
            context: None,
            text_document_position_params: position(uri, pos),
            work_done_progress_params: Default::default(),
        })
    }

    pub fn code_actions(&mut self, uri: &Url, range: lsp::Range) -> Vec<lsp::CodeAction> {
        let diagnostics = Vec::new();
        self.request::<req::CodeActionRequest>(lsp::CodeActionParams {
            text_document: lsp::TextDocumentIdentifier { uri: uri.clone() },
            range,
            context: lsp::CodeActionContext {
                diagnostics,
                only: None,
                trigger_kind: None,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
        .unwrap_or_default()
        .into_iter()
        .filter_map(|a| match a {
            lsp::CodeActionOrCommand::CodeAction(a) => Some(a),
            lsp::CodeActionOrCommand::Command(_) => None,
        })
        .collect()
    }

    pub fn semantic_tokens(&mut self, uri: &Url) -> Vec<lsp::SemanticToken> {
        match self.request::<req::SemanticTokensFullRequest>(lsp::SemanticTokensParams {
            text_document: lsp::TextDocumentIdentifier { uri: uri.clone() },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        }) {
            Some(lsp::SemanticTokensResult::Tokens(t)) => t.data,
            _ => Vec::new(),
        }
    }
}

pub fn position(uri: &Url, pos: lsp::Position) -> lsp::TextDocumentPositionParams {
    lsp::TextDocumentPositionParams {
        text_document: lsp::TextDocumentIdentifier { uri: uri.clone() },
        position: pos,
    }
}

pub fn url(s: &str) -> Url {
    Url::parse(s).expect("bad url")
}

/// The position of the `nth` (0-based) occurrence of `needle` in `text`,
/// plus `delta` characters (ASCII text only).
pub fn pos_of(text: &str, needle: &str, nth: usize, delta: u32) -> lsp::Position {
    let offset = text
        .match_indices(needle)
        .nth(nth)
        .unwrap_or_else(|| panic!("`{needle}` #{nth} not found"))
        .0;
    let before = &text[..offset];
    let line = before.matches('\n').count() as u32;
    let column = (offset - before.rfind('\n').map_or(0, |i| i + 1)) as u32;
    lsp::Position::new(line, column + delta)
}

/// The text covered by an LSP range (ASCII text only).
pub fn slice(text: &str, range: lsp::Range) -> &str {
    let offset = |p: lsp::Position| {
        let start: usize = text
            .split_inclusive('\n')
            .take(p.line as usize)
            .map(str::len)
            .sum();
        start + p.character as usize
    };
    &text[offset(range.start)..offset(range.end)]
}

/// Applies a workspace edit's changes for one document.
pub fn apply(text: &str, edits: &[lsp::TextEdit]) -> String {
    let lines = n2t_core::LineIndex::new(text);
    let mut sorted = edits.to_vec();
    sorted.sort_by_key(|e| (e.range.start.line, e.range.start.character));
    let mut result = text.to_string();
    for edit in sorted.into_iter().rev() {
        let at = |p: lsp::Position| {
            lines
                .offset_utf16(
                    text,
                    n2t_core::Position {
                        line: p.line,
                        column: p.character,
                    },
                )
                .unwrap()
        };
        result.replace_range(at(edit.range.start)..at(edit.range.end), &edit.new_text);
    }
    result
}

pub fn edits_for(edit: &lsp::WorkspaceEdit, uri: &Url) -> Vec<lsp::TextEdit> {
    if let Some(changes) = &edit.changes {
        return changes.get(uri).cloned().unwrap_or_default();
    }
    let mut out = Vec::new();
    if let Some(lsp::DocumentChanges::Operations(ops)) = &edit.document_changes {
        for op in ops {
            if let lsp::DocumentChangeOperation::Edit(e) = op
                && e.text_document.uri == *uri
            {
                for edit in &e.edits {
                    if let lsp::OneOf::Left(t) = edit {
                        out.push(t.clone());
                    }
                }
            }
        }
    }
    if let Some(lsp::DocumentChanges::Edits(edits)) = &edit.document_changes {
        for e in edits {
            if e.text_document.uri == *uri {
                for edit in &e.edits {
                    if let lsp::OneOf::Left(t) = edit {
                        out.push(t.clone());
                    }
                }
            }
        }
    }
    out
}
