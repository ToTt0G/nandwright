//! Half-typed files, huge numbers and bad requests: the server answers,
//! places diagnostics where the problem is, and never panics.

mod common;

use std::time::{Duration, Instant};

use common::{Harness, url};
use lsp_types as lsp;
use lsp_types::request as req;
use n2t_core::host::MemoryHost;

const HDL: &str = "\
// A 3-input majority vote, é ✓.
CHIP Vote3 {
    IN a, b, c;
    OUT out;

    PARTS:
    And(a=a, b=b, out=ab);
    Or(a=a, b=b, out=aorb);
    And(a=aorb, b=c, out=rest);
    Or(a=ab, b=rest, out=out);
}
";

const TST: &str = "\
// 😀 tests for Vote3
load Vote3.hdl,
output-file Vote3.out,
compare-to Vote3.cmp,
output-list a%B3.1.3 b c out;
set a 1, set b 0, set c 1, eval, output;
repeat 2 {
    tick, tock;
}
while out = 0 {
    set a 1, eval;
}
";

const CPU_TST: &str = "\
load Spin.asm,
output-file Spin.out,
output-list RAM[16]%D1.6.1 A D;
set PC 0, set RAM[0] 3,
repeat 20 { ticktock; }
output;
";

const ASM: &str = "\
// Adds 1 to RAM[0] until it reaches 5 (é).
(TOP)
    @R0
    D=M
    @5
    D=D-A
    @DONE
    D;JGE
    @R0
    M=M+1
    @count
    M=M+1
    @TOP
    0;JMP
(DONE)
    @DONE
    0;JMP
";

const HACK: &str = "0000000000000101\n1110110000010000\n0000000000010000\n1110001100001000\n";

const JACK_LIB: &str = "\
/** A pair of numbers (é). */
class Pair {
    field int left, right;

    constructor Pair new(int a, int b) {
        let left = a;
        let right = b;
        return this;
    }

    method int sum() {
        return left + right;
    }

    method void swap() {
        var int t;
        let t = left;
        let left = right;
        let right = t;
        return;
    }
}
";

const JACK_MAIN: &str = "\
class Main {
    function void main() {
        var Pair p;
        var Array cells;
        let p = Pair.new(2, 5);
        do p.swap();
        let cells = Array.new(3);
        let cells[0] = p.sum();
        if (cells[0] > 6) {
            do Output.printString(\"big 😀\");
        } else {
            do Output.printInt(Math.min(cells[0], 9));
        }
        do Memory.deAlloc(cells);
        return;
    }
}
";

const CMP: &str = "|  a  |  b  |  c  | out |\n|  1  |  0  |  1  |  1  |\n";
const OUT: &str = "|  a  |  b  |  c  | out |\n|  1  |  0  |  1  |  0  |\n";

fn host() -> MemoryHost {
    MemoryHost::new()
        .with("/proj/Vote3.cmp", CMP)
        .with("/proj/Pair.jack", JACK_LIB)
        .with("/proj/Spin.asm", ASM)
}

fn end_of(text: &str) -> lsp::Position {
    let line = text.matches('\n').count() as u32;
    let last = text.rsplit('\n').next().unwrap_or("");
    lsp::Position::new(line, last.encode_utf16().count() as u32)
}

/// Every request a client sends while typing, at `pos`. Each must answer
/// without an error response.
fn exercise(h: &mut Harness, uri: &lsp::Url, pos: lsp::Position) {
    let tdp = common::position(uri, pos);
    let doc = lsp::TextDocumentIdentifier { uri: uri.clone() };
    let check = |r: lsp_server::Response, what: &str| {
        assert!(r.error.is_none(), "{what} failed at {pos:?}: {:?}", r.error);
    };
    check(
        h.raw::<req::HoverRequest>(lsp::HoverParams {
            text_document_position_params: tdp.clone(),
            work_done_progress_params: Default::default(),
        }),
        "hover",
    );
    check(
        h.raw::<req::Completion>(lsp::CompletionParams {
            text_document_position: tdp.clone(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: None,
        }),
        "completion",
    );
    check(
        h.raw::<req::SignatureHelpRequest>(lsp::SignatureHelpParams {
            context: None,
            text_document_position_params: tdp.clone(),
            work_done_progress_params: Default::default(),
        }),
        "signature help",
    );
    check(
        h.raw::<req::GotoDefinition>(lsp::GotoDefinitionParams {
            text_document_position_params: tdp.clone(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        }),
        "definition",
    );
    check(
        h.raw::<req::References>(lsp::ReferenceParams {
            text_document_position: tdp.clone(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: lsp::ReferenceContext {
                include_declaration: true,
            },
        }),
        "references",
    );
    check(
        h.raw::<req::DocumentHighlightRequest>(lsp::DocumentHighlightParams {
            text_document_position_params: tdp.clone(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        }),
        "highlights",
    );
    check(
        h.raw::<req::PrepareRenameRequest>(tdp.clone()),
        "prepare rename",
    );
    check(
        h.raw::<req::DocumentSymbolRequest>(lsp::DocumentSymbolParams {
            text_document: doc.clone(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        }),
        "symbols",
    );
    check(
        h.raw::<req::SemanticTokensFullRequest>(lsp::SemanticTokensParams {
            text_document: doc.clone(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        }),
        "semantic tokens",
    );
    check(
        h.raw::<req::CodeActionRequest>(lsp::CodeActionParams {
            text_document: doc.clone(),
            range: lsp::Range::new(lsp::Position::new(0, 0), pos),
            context: Default::default(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        }),
        "code actions",
    );
    check(
        h.raw::<req::FoldingRangeRequest>(lsp::FoldingRangeParams {
            text_document: doc.clone(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        }),
        "folding",
    );
    check(
        h.raw::<req::InlayHintRequest>(lsp::InlayHintParams {
            text_document: doc.clone(),
            range: lsp::Range::new(lsp::Position::new(0, 0), pos),
            work_done_progress_params: Default::default(),
        }),
        "inlay hints",
    );
    check(
        h.raw::<req::Formatting>(lsp::DocumentFormattingParams {
            text_document: doc,
            options: lsp::FormattingOptions {
                tab_size: 4,
                insert_spaces: true,
                ..Default::default()
            },
            work_done_progress_params: Default::default(),
        }),
        "formatting",
    );
}

/// Feeds every prefix of `text` (at character boundaries) to the server as
/// the content of `name`, and exercises every request at the end of the
/// prefix and in its middle.
fn every_prefix(name: &str, text: &str) {
    let mut h = Harness::new(host());
    let uri = url(&format!("file:///proj/{name}"));
    h.open(&uri, "");
    let mut boundaries: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    boundaries.push(text.len());
    for &end in &boundaries {
        let prefix = &text[..end];
        let diagnostics = h.change(&uri, prefix);
        if !prefix.trim().is_empty() {
            for d in &diagnostics {
                let at_origin = d.range.start == lsp::Position::new(0, 0)
                    && d.range.end == lsp::Position::new(0, 0);
                assert!(
                    !at_origin,
                    "{name}: a diagnostic at 0:0 for prefix {prefix:?}: {d:?}"
                );
            }
        }
        let pos = end_of(prefix);
        exercise(&mut h, &uri, pos);
        exercise(
            &mut h,
            &uri,
            lsp::Position::new(pos.line / 2, pos.character / 2),
        );
    }
}

#[test]
fn every_prefix_of_an_hdl_file() {
    every_prefix("Vote3.hdl", HDL);
}

#[test]
fn every_prefix_of_hardware_and_cpu_scripts() {
    every_prefix("Vote3.tst", TST);
    every_prefix("Spin.tst", CPU_TST);
}

#[test]
fn every_prefix_of_assembly_and_machine_code() {
    every_prefix("Spin.asm", ASM);
    every_prefix("Spin.hack", HACK);
}

#[test]
fn every_prefix_of_jack_files() {
    every_prefix("Main.jack", JACK_MAIN);
    every_prefix("Pair2.jack", &JACK_LIB.replace("Pair", "Pair2"));
}

#[test]
fn every_prefix_of_out_and_cmp_files() {
    every_prefix("Vote3.out", OUT);
    every_prefix("Other.cmp", CMP);
}

#[test]
fn huge_numbers_do_not_stall() {
    let cases = [
        (
            "Wide.hdl",
            "CHIP Wide {\n    IN a;\n    OUT out[4000000000];\n    PARTS:\n    Not(in=a, out=out[3999999999]);\n}\n",
        ),
        ("Far.asm", "@99999999999\nD=A\n@4294967296\n(L4000000000)\n"),
        (
            "Long.tst",
            "load Wide.hdl,\nrepeat 4000000000 {\n    tick, tock;\n}\nset a 99999999999999999999,\n",
        ),
        (
            "Loop.tst",
            "load Far.asm,\nrepeat 4000000000 {\n    ticktock;\n}\nset RAM[4000000000] 1,\n",
        ),
        (
            "Big.jack",
            "class Big {\n    function int f() {\n        var Array a;\n        let a[99999999999999] = 4000000000;\n        return 99999999999999999999;\n    }\n}\n",
        ),
        ("Wide.out", "| 4000000000 |\n"),
    ];
    let mut h = Harness::new(host().with("/proj/Wide.cmp", "| * |\n"));
    for (name, text) in cases {
        let started = Instant::now();
        let uri = url(&format!("file:///proj/{name}"));
        let diagnostics = h.open(&uri, text);
        // The assembler accepts these symbols, and the CPU dialect checks
        // RAM addresses only when the script runs.
        let silent = matches!(name, "Far.asm" | "Loop.tst");
        assert!(
            silent || !diagnostics.is_empty(),
            "{name}: expected diagnostics"
        );
        exercise(&mut h, &uri, end_of(text));
        exercise(&mut h, &uri, lsp::Position::new(2, 10));
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_secs(2), "{name} took {elapsed:?}");
    }
}

#[test]
fn bad_requests_answer_null_or_empty() {
    let mut h = Harness::new(host());
    // Unknown documents.
    let unknown = url("file:///proj/Nowhere.jack");
    exercise(&mut h, &unknown, lsp::Position::new(0, 0));
    assert_eq!(h.hover(&unknown, lsp::Position::new(3, 3)), None);
    assert!(h.completion(&unknown, lsp::Position::new(0, 0)).is_empty());

    // Positions outside the text and inside a surrogate pair, every language.
    let docs = [
        ("Vote3.hdl", HDL),
        ("Vote3.tst", TST),
        ("Spin.asm", ASM),
        ("Spin.hack", HACK),
        ("Main.jack", JACK_MAIN),
        ("Vote3.out", OUT),
        ("Notes.txt", "plain 😀 text"),
    ];
    for (name, text) in docs {
        let uri = url(&format!("file:///proj/{name}"));
        h.open(&uri, text);
        for pos in [
            lsp::Position::new(0, 10_000),
            lsp::Position::new(10_000, 0),
            lsp::Position::new(u32::MAX, u32::MAX),
        ] {
            exercise(&mut h, &uri, pos);
        }
        // Inside the emoji or after the `é`, whichever the file has.
        for (line, l) in text.lines().enumerate() {
            if let Some(i) = l.find(['😀', 'é']) {
                let col = l[..i].encode_utf16().count() as u32;
                exercise(&mut h, &uri, lsp::Position::new(line as u32, col + 1));
            }
        }
        // Renames outside the text are refused or empty, never errors of
        // the server itself.
        let response = h.raw::<req::Rename>(lsp::RenameParams {
            text_document_position: common::position(&uri, lsp::Position::new(9_999, 9_999)),
            new_name: "x".into(),
            work_done_progress_params: Default::default(),
        });
        assert!(
            response.error.is_none() || response.error.as_ref().is_some_and(|e| e.code != -32603),
            "{name}: {:?}",
            response.error
        );
    }
}

#[test]
fn json_server_survives_malformed_messages() {
    let host: std::sync::Arc<MemoryHost> = std::sync::Arc::new(host());
    let mut server = n2t_lsp::JsonServer::new(host);
    assert!(server.handle("not json").is_empty());
    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#;
    let out = server.handle(init);
    assert!(out[0].contains("signatureHelpProvider"), "{out:?}");
    assert!(out[0].contains("codeActionProvider"), "{out:?}");
    // Parameters of the wrong shape: an error response, not a crash.
    let out =
        server.handle(r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"x":1}}"#);
    assert!(out[0].contains("\"error\""), "{out:?}");
    // A notification with bad parameters is dropped.
    let _ = server.handle(r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{}}"#);
    // A Jack document through the JSON path, with a Windows URI.
    let open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///c%3A/Users/me/My%20Chips/Main.jack","languageId":"jack","version":1,"text":"class Main { function void main() { do Math."}}}"#;
    let out = server.handle(open);
    assert!(
        out.iter().any(|m| m.contains("publishDiagnostics")),
        "{out:?}"
    );
    let completion = r#"{"jsonrpc":"2.0","id":3,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///c%3A/Users/me/My%20Chips/Main.jack"},"position":{"line":0,"character":44}}}"#;
    let out = server.handle(completion);
    assert!(out[0].contains("\"max\""), "{out:?}");
}
