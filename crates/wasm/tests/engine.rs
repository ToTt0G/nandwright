//! Drives the engine through its JSON API, exactly as the editor does.

use std::sync::Arc;

use n2t_core::host::{MemoryHost, StdHost};
use n2t_wasm::Engine;
use serde_json::{Value, json};

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

/// An engine over an in-memory copy of the fixture chips, under a Windows
/// path with a space in it: what the owner's machine looks like.
const DIR: &str = "C:/Users/me/OneDrive - Uni/nand2tetris/chips";

fn engine() -> (Engine, Arc<MemoryHost>) {
    let host = Arc::new(MemoryHost::new());
    for file in [
        "Majority.hdl",
        "Majority.tst",
        "Majority.cmp",
        "Accumulator.hdl",
        "Accumulator.tst",
        "Accumulator.cmp",
        "KeyLatch.hdl",
        "KeyLatch.tst",
        "KeyLatch.cmp",
    ] {
        host.insert(&format!("{DIR}/{file}"), fixture(&format!("chips/{file}")));
    }
    (Engine::new(host.clone()), host)
}

fn call(engine: &mut Engine, request: Value) -> Value {
    serde_json::from_str(&engine.call(&request.to_string())).unwrap()
}

/// Runs a test session to the end in small slices, as the editor does.
fn run_to_end(engine: &mut Engine, session: &Value) -> (Value, Vec<Value>) {
    let mut failures = Vec::new();
    for _ in 0..10_000 {
        let status = call(
            engine,
            json!({"op": "test.run", "session": session, "budget": 7}),
        );
        failures.extend(status["failures"].as_array().cloned().unwrap_or_default());
        if status["state"] == "finished" {
            return (status, failures);
        }
        assert_eq!(status["state"], "suspended", "{status}");
    }
    panic!("the script did not finish");
}

#[test]
fn runs_a_passing_test_and_writes_the_output_file() {
    let (mut engine, host) = engine();
    // A native Windows path, as VS Code's `fsPath` gives it.
    let path = r"C:\Users\me\OneDrive - Uni\nand2tetris\chips\Majority.tst";
    let opened = call(
        &mut engine,
        json!({"op": "test.open", "path": path, "crlf": true}),
    );
    assert_eq!(opened["dialect"], "hardware", "{opened}");
    let (status, failures) = run_to_end(&mut engine, &opened["session"]);
    assert_eq!(status["outcome"]["kind"], "passed", "{status}");
    assert_eq!(status["outcome"]["comparedLines"], 9);
    assert!(failures.is_empty());
    let out = host.get_text(&format!("{DIR}/Majority.out")).unwrap();
    assert_eq!(out.replace("\r\n", "\n"), fixture("chips/Majority.cmp"));
    assert_eq!(out.matches("\r\n").count(), 9, "CRLF was requested");
}

#[test]
fn a_failing_test_reports_every_failing_row_with_internal_pins() {
    let (mut engine, host) = engine();
    // Break the chip in an unsaved editor buffer: the file on disk is intact.
    let broken =
        fixture("chips/Majority.hdl").replace("And(a=a, b=c, out=ac);", "Or(a=a, b=c, out=ac);");
    call(
        &mut engine,
        json!({"op": "overlay.set", "path": format!("{DIR}/Majority.hdl"), "text": broken}),
    );
    let opened = call(
        &mut engine,
        json!({"op": "test.open", "path": format!("{DIR}/Majority.tst")}),
    );
    let (status, failures) = run_to_end(&mut engine, &opened["session"]);
    let outcome = &status["outcome"];
    assert_eq!(outcome["kind"], "failed", "{status}");
    // a=0 b=0 c=1 is the first row where Or instead of And shows: line 3.
    assert_eq!(outcome["line"], 3);
    assert_eq!(outcome["expected"], "|   0   |   0   |   1   |   0   |");
    assert_eq!(outcome["actual"], "|   0   |   0   |   1   |   1   |");
    assert_eq!(outcome["range"]["startLine"], 8);
    // Every failing row is reported, not only the first.
    assert_eq!(failures.len(), 2, "{failures:?}");
    assert_eq!(failures[1]["line"], 6);
    let ac = failures[0]["state"]
        .as_array()
        .unwrap()
        .iter()
        .find(|pin| pin["name"] == "ac")
        .unwrap();
    assert_eq!(ac["role"], "internal");
    assert_eq!(ac["bits"], 1);
    // The output file stops at the first failing line, as the official
    // simulator's does.
    let out = host.get_text(&format!("{DIR}/Majority.out")).unwrap();
    assert_eq!(out.lines().count(), 3, "{out}");

    // With the buffer discarded the saved chip passes again.
    call(
        &mut engine,
        json!({"op": "overlay.clear", "path": format!("{DIR}/Majority.hdl")}),
    );
    let opened = call(
        &mut engine,
        json!({"op": "test.open", "path": format!("{DIR}/Majority.tst")}),
    );
    let (status, _) = run_to_end(&mut engine, &opened["session"]);
    assert_eq!(status["outcome"]["kind"], "passed");
}

#[test]
fn steps_through_a_script_statement_by_statement() {
    let (mut engine, _) = engine();
    let opened = call(
        &mut engine,
        json!({"op": "test.open", "path": format!("{DIR}/Accumulator.tst")}),
    );
    let session = &opened["session"];
    assert_eq!(opened["next"]["startLine"], 0);
    // The first `;` ends the four setup commands.
    let status = call(&mut engine, json!({"op": "test.step", "session": session}));
    assert_eq!(status["state"], "suspended");
    assert_eq!(status["reason"], "step");
    assert_eq!(status["at"]["startLine"], 3);
    assert_eq!(status["next"]["startLine"], 5);
    assert_eq!(status["events"][0]["type"], "output");
    // One command at a time: `set in 5`.
    let status = call(
        &mut engine,
        json!({"op": "test.step", "session": session, "unit": "command"}),
    );
    assert_eq!(status["at"]["startLine"], 5);
    let inspected = call(
        &mut engine,
        json!({"op": "test.inspect", "session": session}),
    );
    let pin = |name: &str| {
        inspected["state"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .cloned()
            .unwrap()
    };
    assert_eq!(pin("in")["bits"], 5);
    assert_eq!(pin("in")["role"], "input");
    assert_eq!(pin("sum")["role"], "internal");
    assert_eq!(inspected["expected"].as_array().unwrap().len(), 14);
    assert!(
        inspected["outputPath"]
            .as_str()
            .unwrap()
            .ends_with("/Accumulator.out")
    );
}

#[test]
fn presses_the_key_a_script_waits_for_and_says_so() {
    let (mut engine, _) = engine();
    let opened = call(
        &mut engine,
        json!({"op": "test.open", "path": format!("{DIR}/KeyLatch.tst")}),
    );
    let mut pressed = Vec::new();
    let mut echoes = Vec::new();
    let status = loop {
        let status = call(
            &mut engine,
            json!({"op": "test.run", "session": opened["session"], "budget": 50}),
        );
        for event in status["events"].as_array().unwrap() {
            match event["type"].as_str() {
                Some("key") => pressed.push(event["code"].as_u64().unwrap()),
                Some("echo") => echoes.push(event["text"].as_str().unwrap().to_string()),
                _ => {}
            }
        }
        if status["state"] == "finished" {
            break status;
        }
    };
    assert_eq!(status["outcome"]["kind"], "passed", "{status}");
    assert_eq!(pressed, [75]);
    assert_eq!(echoes.len(), 1);

    // Without it, the run is still going when the command limit is reached.
    let opened = call(
        &mut engine,
        json!({"op": "test.open", "path": format!("{DIR}/KeyLatch.tst"), "autoKeyboard": false, "maxCommands": 500}),
    );
    let (status, _) = run_to_end(&mut engine, &opened["session"]);
    assert_eq!(status["outcome"]["kind"], "limit");
    assert!(status["outcome"]["echo"].as_str().unwrap().contains("'K'"));
}

#[test]
fn an_interactive_chip_can_be_poked_and_clocked() {
    let (mut engine, _) = engine();
    let chip = call(
        &mut engine,
        json!({"op": "chip.open", "path": format!("{DIR}/Majority.hdl")}),
    );
    assert_eq!(chip["chip"], "Majority");
    assert_eq!(chip["clocked"], false);
    let session = &chip["session"];
    call(
        &mut engine,
        json!({"op": "chip.set", "session": session, "pin": "a", "value": 1}),
    );
    let state = call(
        &mut engine,
        json!({"op": "chip.set", "session": session, "pin": "c", "value": "1", "eval": true}),
    );
    let value = |state: &Value, name: &str| {
        state["pins"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .unwrap()["bits"]
            .clone()
    };
    assert_eq!(value(&state, "out"), 1);
    assert_eq!(value(&state, "ac"), 1);
    assert_eq!(value(&state, "ab"), 0);
    let error = call(
        &mut engine,
        json!({"op": "chip.set", "session": session, "pin": "a", "value": 2}),
    );
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("does not fit in 1 bit")
    );
    let error = call(
        &mut engine,
        json!({"op": "chip.set", "session": session, "pin": "out", "value": 1}),
    );
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no input pin named `out`")
    );

    let chip = call(
        &mut engine,
        json!({"op": "chip.open", "path": format!("{DIR}/Accumulator.hdl")}),
    );
    assert_eq!(chip["clocked"], true);
    let session = &chip["session"];
    call(
        &mut engine,
        json!({"op": "chip.set", "session": session, "pin": "in", "value": "0x10"}),
    );
    call(
        &mut engine,
        json!({"op": "chip.set", "session": session, "pin": "add", "value": 1}),
    );
    let state = call(&mut engine, json!({"op": "chip.tick", "session": session}));
    assert_eq!(state["time"], "0+");
    assert_eq!(value(&state, "out"), 0);
    let state = call(&mut engine, json!({"op": "chip.tock", "session": session}));
    assert_eq!(state["time"], "1");
    assert_eq!(value(&state, "out"), 16);
    let state = call(&mut engine, json!({"op": "chip.step", "session": session}));
    assert_eq!(state["time"], "2");
    assert_eq!(value(&state, "out"), 32);
    // Reset reloads the chip and keeps the inputs.
    let state = call(&mut engine, json!({"op": "chip.reset", "session": session}));
    assert_eq!(state["time"], "0");
    assert_eq!(value(&state, "out"), 0);
    assert_eq!(value(&state, "in"), 16);

    assert_eq!(engine.session_count(), 2);
    call(&mut engine, json!({"op": "close", "session": session}));
    assert_eq!(engine.session_count(), 1);
    let gone = call(&mut engine, json!({"op": "chip.state", "session": session}));
    assert!(gone["error"]["message"].as_str().unwrap().contains("ended"));
}

#[test]
fn a_small_combinational_chip_has_a_truth_table() {
    let (mut engine, _) = engine();
    let chip = call(
        &mut engine,
        json!({"op": "chip.open", "path": format!("{DIR}/Majority.hdl")}),
    );
    let session = &chip["session"];
    call(
        &mut engine,
        json!({"op": "chip.set", "session": session, "pin": "b", "value": 1, "eval": true}),
    );
    let table = call(&mut engine, json!({"op": "chip.table", "session": session}));
    let rows = table["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 8);
    assert_eq!(table["inputs"][0]["name"], "a");
    let outputs: Vec<u64> = rows
        .iter()
        .map(|r| r["outputs"][0].as_u64().unwrap())
        .collect();
    assert_eq!(outputs, [0, 0, 0, 1, 0, 1, 1, 1]);
    assert_eq!(rows[3]["inputs"], json!([0, 1, 1]));
    // The inputs the student had set are untouched.
    let state = call(&mut engine, json!({"op": "chip.state", "session": session}));
    let b = state["pins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "b")
        .unwrap();
    assert_eq!(b["bits"], 1);

    let clocked = call(
        &mut engine,
        json!({"op": "chip.open", "path": format!("{DIR}/Accumulator.hdl")}),
    );
    let table = call(
        &mut engine,
        json!({"op": "chip.table", "session": clocked["session"]}),
    );
    assert!(
        table["error"]["message"]
            .as_str()
            .unwrap()
            .contains("clocked")
    );
}

#[test]
fn runs_to_an_editor_breakpoint() {
    let (mut engine, _) = engine();
    let opened = call(
        &mut engine,
        json!({"op": "test.open", "path": format!("{DIR}/Majority.tst")}),
    );
    let session = &opened["session"];
    // Line 10 (zero-based) is the fourth row, `set a 0, set b 1, set c 1, …`.
    let status = call(
        &mut engine,
        json!({"op": "test.run", "session": session, "breakLines": [10]}),
    );
    assert_eq!(status["state"], "suspended", "{status}");
    assert_eq!(status["reason"], "breakpoint");
    assert_eq!(status["next"]["startLine"], 10);
    let outputs = status["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == "output")
        .count();
    assert_eq!(
        outputs, 4,
        "the header and three rows come before the breakpoint"
    );
    let status = call(
        &mut engine,
        json!({"op": "test.run", "session": session, "breakLines": [10]}),
    );
    assert_eq!(status["state"], "finished", "{status}");
    assert_eq!(status["outcome"]["kind"], "passed");
}

#[test]
fn a_chip_with_errors_explains_itself() {
    let (mut engine, host) = engine();
    host.insert(
        &format!("{DIR}/Bad.hdl"),
        "CHIP Bad {\n    IN a;\n    OUT out;\n    PARTS:\n    Nott(in=a, out=out);\n}\n",
    );
    let reply = call(
        &mut engine,
        json!({"op": "chip.open", "path": format!("{DIR}/Bad.hdl")}),
    );
    let message = reply["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("Bad.hdl:5:5: unknown chip `Nott`"),
        "{message}"
    );
    assert!(message.starts_with("cannot load `Bad.hdl`"), "{message}");
    let reply = call(
        &mut engine,
        json!({"op": "test.open", "path": format!("{DIR}/Missing.tst")}),
    );
    assert!(
        reply["error"]["message"]
            .as_str()
            .unwrap()
            .contains("cannot read `Missing.tst`")
    );
}

#[test]
fn speaks_lsp_through_the_same_call() {
    let (mut engine, _) = engine();
    let request =
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"capabilities": {}}});
    let reply = call(
        &mut engine,
        json!({"op": "lsp", "message": request.to_string()}),
    );
    let message: Value = serde_json::from_str(reply["messages"][0].as_str().unwrap()).unwrap();
    assert_eq!(message["id"], 1);
    assert_eq!(message["result"]["serverInfo"]["name"], "nandwright");
    assert_eq!(message["result"]["capabilities"]["hoverProvider"], true);

    // A document under a Windows path, as VS Code names it.
    let uri = "file:///c%3A/Users/me/OneDrive%20-%20Uni/nand2tetris/chips/Majority.hdl";
    let open = json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {
        "uri": uri, "languageId": "hdl", "version": 1,
        "text": fixture("chips/Majority.hdl").replace("Or(a=ab", "Orr(a=ab"),
    }}});
    let reply = call(
        &mut engine,
        json!({"op": "lsp", "message": open.to_string()}),
    );
    let published: Value = serde_json::from_str(reply["messages"][0].as_str().unwrap()).unwrap();
    assert_eq!(published["method"], "textDocument/publishDiagnostics");
    assert_eq!(published["params"]["uri"], uri);
    let diagnostics = published["params"]["diagnostics"].as_array().unwrap();
    assert!(
        diagnostics.iter().any(|d| d["code"] == "hdl-unknown-chip"),
        "{diagnostics:?}"
    );
}

#[test]
fn nonsense_requests_get_an_error_not_a_crash() {
    let (mut engine, _) = engine();
    for request in [
        "",
        "{",
        "null",
        "[]",
        "{\"op\": 5}",
        "{\"op\": \"nope\"}",
        "{\"op\": \"test.run\"}",
        "{\"op\": \"test.run\", \"session\": 99}",
        "{\"op\": \"chip.open\"}",
        "{\"op\": \"chip.open\", \"path\": \"\"}",
        "{\"op\": \"lsp\", \"message\": \"not json\"}",
        "{\"op\": \"lsp\", \"message\": \"{\\\"jsonrpc\\\":\\\"2.0\\\",\\\"id\\\":1,\\\"method\\\":\\\"textDocument/hover\\\"}\"}",
    ] {
        let reply: Value = serde_json::from_str(&engine.call(request)).unwrap();
        assert!(reply.is_object(), "{request} -> {reply}");
    }
}

#[test]
fn runs_from_the_real_file_system_too() {
    let dir = std::env::temp_dir().join(format!("n2t-wasm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for file in ["Majority.hdl", "Majority.tst", "Majority.cmp"] {
        std::fs::write(dir.join(file), fixture(&format!("chips/{file}"))).unwrap();
    }
    let mut engine = Engine::new(Arc::new(StdHost));
    let opened = call(
        &mut engine,
        json!({"op": "test.open", "path": dir.join("Majority.tst")}),
    );
    let (status, _) = run_to_end(&mut engine, &opened["session"]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(status["outcome"]["kind"], "passed", "{status}");
}
