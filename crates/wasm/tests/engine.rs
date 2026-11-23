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
fn the_next_command_after_a_loop_is_the_one_after_it() {
    let (mut engine, _) = engine();
    let opened = call(
        &mut engine,
        json!({"op": "test.open", "path": format!("{DIR}/Accumulator.tst")}),
    );
    let session = &opened["session"];
    // Line 22 (zero-based) is `set reset 1,`, right after `repeat 3 { … }`.
    let status = call(
        &mut engine,
        json!({"op": "test.run", "session": session, "breakLines": [22]}),
    );
    assert_eq!(status["reason"], "breakpoint", "{status}");
    assert_eq!(status["next"]["startLine"], 22);
    // The loop body ran three times before it.
    let rows = status["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == "output")
        .count();
    assert_eq!(rows, 10);
    // A breakpoint inside the loop is hit once per iteration.
    let opened = call(
        &mut engine,
        json!({"op": "test.open", "path": format!("{DIR}/Accumulator.tst")}),
    );
    let session = &opened["session"];
    let mut hits = 0;
    loop {
        let status = call(
            &mut engine,
            json!({"op": "test.run", "session": session, "breakLines": [19]}),
        );
        if status["state"] == "finished" {
            break;
        }
        assert_eq!(status["next"]["startLine"], 19);
        hits += 1;
    }
    assert_eq!(hits, 3);
}

#[test]
fn a_program_runs_on_the_hack_computer() {
    let (mut engine, host) = engine();
    for file in ["Countdown.asm", "Diagonal.asm", "KeyLog.asm"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../hack/tests/fixtures")
            .join(file);
        host.insert(&format!("{DIR}/{file}"), std::fs::read(path).unwrap());
    }
    let ram = |state: &Value, address: u64| {
        let start = state["ram"]["start"].as_u64().unwrap();
        state["ram"]["values"][(address - start) as usize]
            .as_u64()
            .unwrap()
    };
    let register = |state: &Value, name: &str| {
        state["registers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap()["value"]
            .clone()
    };

    let opened = call(
        &mut engine,
        json!({"op": "cpu.open", "path": format!("{DIR}/Countdown.asm")}),
    );
    let session = &opened["session"];
    assert_eq!(opened["kind"], "cpu");
    assert_eq!(
        opened["line"], 2,
        "the first instruction is on the third line"
    );
    assert_eq!(opened["instruction"], "@100");
    call(
        &mut engine,
        json!({"op": "cpu.set", "session": session, "target": "RAM", "address": 0, "value": 3}),
    );
    // One instruction at a time.
    let state = call(&mut engine, json!({"op": "cpu.step", "session": session}));
    assert_eq!(register(&state, "A"), 100);
    assert_eq!(register(&state, "PC"), 1);
    assert_eq!(state["instruction"], "D=A");
    // Run to a breakpoint on the line of `(DONE)`: the next instruction after it.
    let source = String::from_utf8(host.get(&format!("{DIR}/Countdown.asm")).unwrap()).unwrap();
    let done_line = source.lines().position(|l| l.trim() == "(DONE)").unwrap();
    let state = call(
        &mut engine,
        json!({"op": "cpu.run", "session": session, "budget": 100000, "breakLines": [done_line], "ram": [100, 4]}),
    );
    assert_eq!(state["status"], "breakpoint", "{state}");
    assert_eq!(state["line"].as_u64().unwrap() as usize, done_line + 1);
    assert_eq!(
        [ram(&state, 100), ram(&state, 101), ram(&state, 102)],
        [3, 2, 1]
    );
    // An endless loop just uses up the budget.
    let state = call(
        &mut engine,
        json!({"op": "cpu.run", "session": session, "budget": 1000, "ram": [0, 2]}),
    );
    assert_eq!(state["status"], "running");
    assert_eq!(ram(&state, 1), 3);
    // Reset reloads the program and clears memory.
    let state = call(
        &mut engine,
        json!({"op": "cpu.reset", "session": session, "ram": [0, 2]}),
    );
    assert_eq!(register(&state, "PC"), 0);
    assert_eq!(ram(&state, 1), 0);

    // The screen is sent once, then only when it changes.
    let drawing = call(
        &mut engine,
        json!({"op": "cpu.open", "path": format!("{DIR}/Diagonal.asm"), "screen": true}),
    );
    let blank = drawing["screen"].as_str().unwrap();
    assert_eq!(blank.len(), 8192 * 4);
    assert!(blank.bytes().all(|b| b == b'0'));
    let session = &drawing["session"];
    let state = call(
        &mut engine,
        json!({"op": "cpu.run", "session": session, "budget": 200000, "screen": true}),
    );
    let drawn = state["screen"].as_str().unwrap();
    assert!(drawn.bytes().any(|b| b != b'0'), "the program drew nothing");
    let state = call(
        &mut engine,
        json!({"op": "cpu.state", "session": session, "screen": true}),
    );
    assert!(
        state["screen"].is_null(),
        "an unchanged screen is not sent again"
    );

    // The keyboard is memory-mapped at 24576.
    let keys = call(
        &mut engine,
        json!({"op": "cpu.open", "path": format!("{DIR}/KeyLog.asm")}),
    );
    call(
        &mut engine,
        json!({"op": "key", "session": keys["session"], "code": 75}),
    );
    let state = call(
        &mut engine,
        json!({"op": "cpu.state", "session": keys["session"], "ram": [24576, 1]}),
    );
    assert_eq!(ram(&state, 24576), 75);
    assert_eq!(state["key"], 75);
}

/// Copies the Jack fixture program (two classes) into the in-memory folder.
fn with_jack_program(host: &MemoryHost) -> String {
    let dir = format!("{DIR}/counter");
    for file in ["Main.jack", "Counter.jack"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../jack/tests/fixtures/Counter")
            .join(file);
        host.insert(&format!("{dir}/{file}"), std::fs::read(path).unwrap());
    }
    dir
}

#[test]
fn a_jack_program_runs_on_the_vm_without_writing_vm_files() {
    let (mut engine, host) = engine();
    let dir = with_jack_program(&host);
    let opened = call(
        &mut engine,
        json!({"op": "vm.open", "path": format!("{dir}/Main.jack"), "screen": true}),
    );
    assert_eq!(opened["kind"], "vm", "{opened}");
    assert_eq!(opened["fromJack"], true);
    let session = &opened["session"];
    // Run to the end in slices, as the panel does.
    let mut state = opened.clone();
    let mut drawn = None;
    for _ in 0..10_000 {
        state = call(
            &mut engine,
            json!({"op": "vm.run", "session": session, "budget": 500, "now": 0, "screen": true}),
        );
        if let Some(screen) = state["screen"].as_str() {
            drawn = Some(screen.to_string());
        }
        if state["status"] != "running" {
            break;
        }
    }
    assert_eq!(state["status"], "halted", "{state}");
    // The program printed text with the built-in Output: the screen is not blank.
    let drawn = drawn.expect("the screen was never sent");
    assert!(drawn.bytes().any(|b| b != b'0'));
    // Nothing was written next to the sources.
    assert!(host.get(&format!("{dir}/Main.vm")).is_none());
    assert!(host.get(&format!("{dir}/Counter.vm")).is_none());
}

#[test]
fn a_jack_program_stops_at_a_jack_breakpoint_and_steps_by_jack_line() {
    let (mut engine, host) = engine();
    let dir = with_jack_program(&host);
    let main = format!("{dir}/Main.jack");
    let source = String::from_utf8(host.get(&main).unwrap()).unwrap();
    let line_of = |needle: &str| source.lines().position(|l| l.contains(needle)).unwrap();
    let opened = call(&mut engine, json!({"op": "vm.open", "path": main}));
    let session = &opened["session"];
    let add = line_of("do c.add(25);");
    let state = call(
        &mut engine,
        json!({"op": "vm.run", "session": session, "budget": 1_000_000, "now": 0,
               "breakpoints": [{"path": main, "lines": [add]}]}),
    );
    assert_eq!(state["status"], "breakpoint", "{state}");
    assert_eq!(state["line"].as_u64().unwrap() as usize, add);
    assert!(state["file"].as_str().unwrap().ends_with("/Main.jack"));
    assert_eq!(state["function"], "Main.main");
    assert_eq!(state["callStack"][0]["function"], "Main.main");
    // One step of a Jack line runs the whole call and lands on the next line.
    let state = call(
        &mut engine,
        json!({"op": "vm.step", "session": session, "unit": "line", "now": 0}),
    );
    assert_eq!(state["status"], "paused", "{state}");
    assert_eq!(
        state["line"].as_u64().unwrap() as usize,
        line_of("do d.increment();")
    );
    // A single VM command is a much smaller step.
    let before = state["steps"].as_u64().unwrap();
    let state = call(
        &mut engine,
        json!({"op": "vm.step", "session": session, "now": 0}),
    );
    assert_eq!(state["steps"].as_u64().unwrap(), before + 1);
    assert!(!state["instruction"].as_str().unwrap().is_empty());
}

#[test]
fn compiling_jack_writes_the_vm_files_the_official_compiler_writes() {
    let (mut engine, host) = engine();
    let dir = with_jack_program(&host);
    // Without `write` nothing is written: the reply says what would be.
    let reply = call(&mut engine, json!({"op": "jack.compile", "path": dir}));
    assert_eq!(reply["written"], false);
    assert_eq!(reply["outputs"][0]["existing"], "missing");
    assert!(host.get(&format!("{dir}/Main.vm")).is_none());
    let reply = call(
        &mut engine,
        json!({"op": "jack.compile", "path": dir, "write": true}),
    );
    assert_eq!(reply["problems"].as_array().unwrap().len(), 0, "{reply}");
    assert_eq!(reply["outputs"].as_array().unwrap().len(), 2);
    let reply = call(&mut engine, json!({"op": "jack.compile", "path": dir}));
    assert_eq!(reply["outputs"][0]["existing"], "same");
    for file in ["Main.vm", "Counter.vm"] {
        // The goldens next to the fixture were produced by the Java compiler.
        let golden = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../jack/tests/fixtures/Counter")
                .join(file),
        )
        .unwrap();
        assert_eq!(
            host.get_text(&format!("{dir}/{file}")).unwrap(),
            golden.replace("\r\n", "\n")
        );
    }
    // The compiled files run as a VM program too.
    let opened = call(
        &mut engine,
        json!({"op": "vm.open", "path": format!("{dir}/Main.vm")}),
    );
    assert_eq!(opened["fromJack"], false, "{opened}");
    assert!(opened["file"].as_str().unwrap().ends_with(".vm"));

    // A class with an error: nothing is written, and the problem is located.
    host.insert(
        &format!("{dir}/Broken.jack"),
        "class Broken {\n    function void f() {\n        let x = 1;\n        return;\n    }\n}\n",
    );
    host.remove(&format!("{dir}/Main.vm"));
    let reply = call(
        &mut engine,
        json!({"op": "jack.compile", "path": format!("{dir}/Main.jack")}),
    );
    let problems = reply["problems"].as_array().unwrap();
    assert_eq!(problems.len(), 1, "{reply}");
    assert!(
        problems[0]["path"]
            .as_str()
            .unwrap()
            .ends_with("Broken.jack")
    );
    assert_eq!(problems[0]["range"]["startLine"], 2);
    assert!(host.get(&format!("{dir}/Main.vm")).is_none());
}

#[test]
fn a_students_own_tool_output_is_compared_with_the_reference_not_overwritten() {
    let (mut engine, host) = engine();
    let dir = with_jack_program(&host);
    let countdown = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../hack/tests/fixtures");
    host.insert(
        &format!("{dir}/Countdown.asm"),
        std::fs::read(countdown.join("Countdown.asm")).unwrap(),
    );
    let golden_hack = std::fs::read_to_string(countdown.join("Countdown.hack")).unwrap();

    // Project 6: the student's assembler wrote a .hack with one wrong word.
    let mut theirs: Vec<&str> = golden_hack.lines().collect();
    theirs[3] = "1111111111111111";
    host.insert(&format!("{dir}/Countdown.hack"), theirs.join("\r\n"));
    let reply = call(
        &mut engine,
        json!({"op": "reference", "path": format!("{dir}/Countdown.hack")}),
    );
    assert_eq!(
        reply["text"].as_str().unwrap(),
        golden_hack.replace("\r\n", "\n"),
        "{reply}"
    );
    assert!(reply["from"].as_str().unwrap().ends_with("Countdown.asm"));
    // Assembling says the file exists and differs, and leaves it alone.
    let reply = call(
        &mut engine,
        json!({"op": "asm.assemble", "path": format!("{dir}/Countdown.asm")}),
    );
    assert_eq!(reply["outputs"][0]["existing"], "different", "{reply}");
    assert_eq!(
        host.get_text(&format!("{dir}/Countdown.hack"))
            .unwrap()
            .lines()
            .nth(3),
        Some("1111111111111111")
    );
    // Only an explicit write that names what it replaces does so.
    let hack = format!("{dir}/Countdown.hack");
    let written = call(
        &mut engine,
        json!({"op": "asm.assemble", "path": format!("{dir}/Countdown.asm"), "write": true,
               "expect": {hack.as_str(): reply["outputs"][0]["state"]}}),
    );
    assert_eq!(written["written"], true, "{written}");
    assert_eq!(
        host.get_text(&format!("{dir}/Countdown.hack")).unwrap(),
        golden_hack.replace("\r\n", "\n")
    );

    // Project 11: the reference .vm for one class of a folder.
    let reply = call(
        &mut engine,
        json!({"op": "reference", "path": format!("{dir}/Counter.vm")}),
    );
    assert!(
        reply["text"]
            .as_str()
            .unwrap()
            .starts_with("function Counter.new 0\n"),
        "{reply}"
    );
    // Project 10: tokens and parse tree.
    let tokens = call(
        &mut engine,
        json!({"op": "reference", "path": format!("{dir}/MainT.xml")}),
    );
    assert!(
        tokens["text"].as_str().unwrap().starts_with("<tokens>"),
        "{tokens}"
    );
    let tree = call(
        &mut engine,
        json!({"op": "reference", "path": format!("{dir}/Main.xml")}),
    );
    assert!(
        tree["text"].as_str().unwrap().starts_with("<class>"),
        "{tree}"
    );
    // Anything else explains itself.
    let reply = call(
        &mut engine,
        json!({"op": "reference", "path": format!("{dir}/Nothing.hack")}),
    );
    assert!(
        reply["error"]["message"]
            .as_str()
            .unwrap()
            .contains("there is no `Nothing.asm`")
    );
}

#[test]
fn a_program_that_goes_wrong_stops_with_an_explanation() {
    let (mut engine, host) = engine();
    host.insert(&format!("{DIR}/Wild.asm"), "@30000\nM=1\n");
    host.insert(&format!("{DIR}/Typo.asm"), "@1\nD=Q\n");
    let opened = call(
        &mut engine,
        json!({"op": "cpu.open", "path": format!("{DIR}/Wild.asm")}),
    );
    let state = call(
        &mut engine,
        json!({"op": "cpu.run", "session": opened["session"], "budget": 10}),
    );
    assert_eq!(state["status"], "error");
    assert!(
        state["error"]
            .as_str()
            .unwrap()
            .contains("not a RAM address"),
        "{state}"
    );
    let again = call(
        &mut engine,
        json!({"op": "cpu.run", "session": opened["session"], "budget": 10}),
    );
    assert!(
        again["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Reset the program")
    );
    let typo = call(
        &mut engine,
        json!({"op": "cpu.open", "path": format!("{DIR}/Typo.asm")}),
    );
    assert!(
        typo["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Typo.asm:2"),
        "{typo}"
    );
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
