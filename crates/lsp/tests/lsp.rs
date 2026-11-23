//! Integration tests for `n2t-lsp` covering formatting, range formatting,
//! autocomplete, diagnostics, hover, go-to-definition, and inlay hints.

use lsp_server::{Message, Notification, Request, RequestId};
use lsp_types as lsp;
use lsp_types::Url;
use lsp_types::notification::{self as notif, Notification as _};
use lsp_types::request as req;
use n2t_lsp::{Server, capabilities};
use serde_json::Value;

fn test_server() -> Server {
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
    Server::new(&params, std::sync::Arc::new(n2t_core::host::StdHost))
}

fn open_doc(server: &mut Server, uri: &Url, language_id: &str, text: &str) {
    let params = lsp::DidOpenTextDocumentParams {
        text_document: lsp::TextDocumentItem {
            uri: uri.clone(),
            language_id: language_id.to_string(),
            version: 1,
            text: text.to_string(),
        },
    };
    server
        .handle(Message::Notification(Notification::new(
            notif::DidOpenTextDocument::METHOD.to_string(),
            params,
        )))
        .unwrap();
}

fn send_request<R: req::Request>(server: &mut Server, id: i32, params: R::Params) -> R::Result {
    let req = Request::new(RequestId::from(id), R::METHOD.to_string(), params);
    server.handle(Message::Request(req)).unwrap();
    let msgs = server.take_outgoing();
    let resp = msgs
        .into_iter()
        .find_map(|m| match m {
            Message::Response(r) if r.id == RequestId::from(id) => Some(r),
            _ => None,
        })
        .expect("expected LSP response");
    assert!(
        resp.error.is_none(),
        "unexpected LSP error: {:?}",
        resp.error
    );
    serde_json::from_value(resp.result.unwrap_or(Value::Null)).unwrap()
}

fn apply_edits(original: &str, edits: &[lsp::TextEdit]) -> String {
    let lines = n2t_core::LineIndex::new(original);
    let mut result = original.to_string();
    let mut sorted = edits.to_vec();
    sorted.sort_by_key(|e| (e.range.start.line, e.range.start.character));
    for edit in sorted.into_iter().rev() {
        let start = lines
            .offset_utf16(
                original,
                n2t_core::Position {
                    line: edit.range.start.line,
                    column: edit.range.start.character,
                },
            )
            .unwrap();
        let end = lines
            .offset_utf16(
                original,
                n2t_core::Position {
                    line: edit.range.end.line,
                    column: edit.range.end.character,
                },
            )
            .unwrap();
        result.replace_range(start..end, &edit.new_text);
    }
    result
}

#[test]
fn capabilities_advertise_formatting_and_intelligence() {
    let caps = capabilities();
    assert!(caps.document_formatting_provider.is_some());
    assert!(caps.document_range_formatting_provider.is_some());
    assert!(caps.completion_provider.is_some());
    assert!(caps.hover_provider.is_some());
    assert!(caps.definition_provider.is_some());
    assert!(caps.inlay_hint_provider.is_some());
    assert!(caps.semantic_tokens_provider.is_some());
    assert!(caps.signature_help_provider.is_some());
    assert!(caps.code_action_provider.is_some());
    assert!(caps.folding_range_provider.is_some());
    assert!(caps.references_provider.is_some());
    assert!(caps.rename_provider.is_some());
}

#[test]
fn formats_hdl_document_and_is_idempotent() {
    let mut server = test_server();
    let uri = Url::parse("file:///tmp/Nor.hdl").unwrap();
    let messy = "CHIP   Nor{\nIN a,b;\n  OUT out;\n\n\nPARTS:\nOr(a=a,b=b,out=w);\n      Not(in=w,out=out);\n}";
    open_doc(&mut server, &uri, "hdl", messy);
    let _ = server.take_outgoing();

    let edits = send_request::<req::Formatting>(
        &mut server,
        1,
        lsp::DocumentFormattingParams {
            text_document: lsp::TextDocumentIdentifier { uri: uri.clone() },
            options: lsp::FormattingOptions {
                tab_size: 4,
                insert_spaces: true,
                ..Default::default()
            },
            work_done_progress_params: Default::default(),
        },
    )
    .expect("valid HDL should produce edits");

    let formatted = apply_edits(messy, &edits);
    assert_eq!(
        formatted,
        "CHIP Nor {\n    IN a, b;\n    OUT out;\n\n    PARTS:\n    Or(a=a, b=b, out=w);\n    Not(in=w, out=out);\n}\n"
    );

    // Re-open formatted text and ensure formatting returns empty edit list.
    open_doc(&mut server, &uri, "hdl", &formatted);
    let _ = server.take_outgoing();
    let second_edits = send_request::<req::Formatting>(
        &mut server,
        2,
        lsp::DocumentFormattingParams {
            text_document: lsp::TextDocumentIdentifier { uri },
            options: lsp::FormattingOptions {
                tab_size: 4,
                insert_spaces: true,
                ..Default::default()
            },
            work_done_progress_params: Default::default(),
        },
    )
    .unwrap();
    assert!(second_edits.is_empty());
}

#[test]
fn range_formatting_updates_only_selected_lines_when_line_count_matches() {
    let mut server = test_server();
    let uri = Url::parse("file:///tmp/Nor.hdl").unwrap();
    let src = "\
CHIP Nor {
  IN a,b;
    OUT out;

    PARTS:
  Or( a=a , b=b , out=w );
  Not( in=w , out=out );
}
";
    open_doc(&mut server, &uri, "hdl", src);
    let _ = server.take_outgoing();

    // Only format line 5 (`  Or( a=a , b=b , out=w );`).
    let edits = send_request::<req::RangeFormatting>(
        &mut server,
        1,
        lsp::DocumentRangeFormattingParams {
            text_document: lsp::TextDocumentIdentifier { uri },
            range: lsp::Range::new(lsp::Position::new(5, 0), lsp::Position::new(5, 28)),
            options: lsp::FormattingOptions {
                tab_size: 4,
                insert_spaces: true,
                ..Default::default()
            },
            work_done_progress_params: Default::default(),
        },
    )
    .unwrap();

    let updated = apply_edits(src, &edits);
    assert!(
        updated.contains("  IN a,b;\n"),
        "line 1 should stay untouched"
    );
    assert!(
        updated.contains("    Or(a=a, b=b, out=w);\n"),
        "line 5 should be formatted"
    );
    assert!(
        updated.contains("  Not( in=w , out=out );\n"),
        "line 6 should stay untouched"
    );
}

#[test]
fn formats_tst_document() {
    let mut server = test_server();
    let uri = Url::parse("file:///tmp/And.tst").unwrap();
    let messy = "load And.hdl,\nrepeat 2{\ntick,tock,output;\n}\n";
    open_doc(&mut server, &uri, "tst", messy);
    let _ = server.take_outgoing();

    let edits = send_request::<req::Formatting>(
        &mut server,
        1,
        lsp::DocumentFormattingParams {
            text_document: lsp::TextDocumentIdentifier { uri },
            options: lsp::FormattingOptions {
                tab_size: 4,
                insert_spaces: true,
                ..Default::default()
            },
            work_done_progress_params: Default::default(),
        },
    )
    .unwrap();

    let formatted = apply_edits(messy, &edits);
    assert_eq!(
        formatted,
        "load And.hdl,\nrepeat 2 {\n    tick, tock, output;\n}\n"
    );
}

#[test]
fn completion_suggests_chips_formal_pins_and_signals() {
    let mut server = test_server();
    let uri = Url::parse("file:///tmp/Nor.hdl").unwrap();
    let src = "\
CHIP Nor {
    IN a, b;
    OUT out;

    PARTS:
    Or(a=a, b=b, out=w);
    Not(in=);
}
";
    open_doc(&mut server, &uri, "hdl", src);
    let _ = server.take_outgoing();

    // After `Not(in=` -> suggests `a`, `b`, `w`, `true`, `false` (not `out` because `in` is input).
    let resp = send_request::<req::Completion>(
        &mut server,
        1,
        lsp::CompletionParams {
            text_document_position: lsp::TextDocumentPositionParams {
                text_document: lsp::TextDocumentIdentifier { uri: uri.clone() },
                position: lsp::Position::new(6, 11),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: None,
        },
    )
    .unwrap();
    let lsp::CompletionResponse::Array(items) = resp else {
        panic!("expected array completion response");
    };
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"a"), "labels: {labels:?}");
    assert!(labels.contains(&"b"), "labels: {labels:?}");
    assert!(labels.contains(&"w"), "labels: {labels:?}");
    assert!(labels.contains(&"true"), "labels: {labels:?}");
    assert!(
        !labels.contains(&"out"),
        "top-level output cannot feed an input pin"
    );
}
