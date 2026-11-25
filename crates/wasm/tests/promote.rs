//! `test.promote`: inspecting a reached row of a CPU or VM test in the Hack
//! computer or VM emulator panel, and `test.open`'s `writeOutput` and
//! `stopAtLine`.

use std::sync::Arc;

use n2t_core::host::MemoryHost;
use n2t_wasm::Engine;
use serde_json::{Value, json};

const DIR: &str = "C:/Users/me/nand2tetris/prog";

fn engine() -> (Engine, Arc<MemoryHost>) {
    let host = Arc::new(MemoryHost::new());
    (Engine::new(host.clone()), host)
}

fn call(engine: &mut Engine, request: Value) -> Value {
    serde_json::from_str(&engine.call(&request.to_string())).unwrap()
}

/// Runs a test session until it suspends at its target line or finishes.
fn run(engine: &mut Engine, session: &Value) -> Value {
    for _ in 0..10_000 {
        let status = call(
            engine,
            json!({"op": "test.run", "session": session, "budget": 50}),
        );
        if status["state"] == "finished" || status["reason"] == "targetLine" {
            return status;
        }
        assert_eq!(status["reason"], "budget", "{status}");
    }
    panic!("the script did not stop");
}

fn register(state: &Value, name: &str) -> Value {
    state["registers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == name)
        .unwrap_or_else(|| panic!("no register {name} in {state}"))["value"]
        .clone()
}

// ---------------------------------------------------------------------------
// CPU
// ---------------------------------------------------------------------------

/// RAM[2] = RAM[0] + RAM[1], then loops forever.
const ADD_ASM: &str = "@R0\nD=M\n@R1\nD=D+M\n@R2\nM=D\n(END)\n@END\n0;JMP\n";

fn cpu_script(compare: bool) -> String {
    format!(
        "load Add.asm,\noutput-file Add.out,\n{}output-list RAM[0]%D2.6.2 RAM[1]%D2.6.2 RAM[2]%D2.6.2;\n\
         set RAM[0] 1, set RAM[1] 2, set RAM[2] 0;\nrepeat 10 {{ ticktock; }}\noutput;\n\
         set PC 0, set RAM[0] 5, set RAM[1] 6;\nrepeat 3 {{ ticktock; }}\noutput;\n\
         set PC 0, set RAM[0] 7, set RAM[1] 8;\nrepeat 10 {{ ticktock; }}\noutput;\n",
        if compare { "compare-to Add.cmp,\n" } else { "" }
    )
}

/// The real output of the CPU script, line by line.
fn cpu_lines(engine: &mut Engine, host: &MemoryHost) -> Vec<String> {
    host.insert(&format!("{DIR}/Add.asm"), ADD_ASM);
    host.insert(&format!("{DIR}/Add.tst"), cpu_script(false));
    let opened = call(
        engine,
        json!({"op": "test.open", "path": format!("{DIR}/Add.tst")}),
    );
    let status = run(engine, &opened["session"]);
    assert_eq!(status["outcome"]["kind"], "unchecked", "{status}");
    call(engine, json!({"op": "close", "session": opened["session"]}));
    let out = host.get_text(&format!("{DIR}/Add.out")).unwrap();
    host.remove(&format!("{DIR}/Add.out"));
    host.insert(&format!("{DIR}/Add.tst"), cpu_script(true));
    out.lines().map(str::to_string).collect()
}

#[test]
fn write_output_false_keeps_the_out_file_in_memory() {
    let (mut engine, host) = engine();
    let lines = cpu_lines(&mut engine, &host);
    host.insert(&format!("{DIR}/Add.cmp"), lines.join("\n") + "\n");
    let opened = call(
        &mut engine,
        json!({"op": "test.open", "path": format!("{DIR}/Add.tst"), "writeOutput": false}),
    );
    let status = run(&mut engine, &opened["session"]);
    assert_eq!(status["outcome"]["kind"], "passed", "{status}");
    assert!(host.get(&format!("{DIR}/Add.out")).is_none());
    let inspected = call(
        &mut engine,
        json!({"op": "test.inspect", "session": opened["session"]}),
    );
    assert_eq!(inspected["output"], lines.join("\n") + "\n");

    // The default still writes it.
    let opened = call(
        &mut engine,
        json!({"op": "test.open", "path": format!("{DIR}/Add.tst")}),
    );
    run(&mut engine, &opened["session"]);
    assert!(host.get(&format!("{DIR}/Add.out")).is_some());
}

#[test]
fn a_cpu_test_is_promoted_at_a_failing_row() {
    let (mut engine, host) = engine();
    let mut lines = cpu_lines(&mut engine, &host);
    // Rows 1 and 2 fail; inspect row 2 (line 3 of the compare file).
    lines[1] = lines[1].replace('3', "4");
    lines[2] = lines[2].replace('5', "9");
    host.insert(&format!("{DIR}/Add.cmp"), lines.join("\n") + "\n");
    let opened = call(
        &mut engine,
        json!({"op": "test.open", "path": format!("{DIR}/Add.tst"),
               "writeOutput": false, "stopAtLine": 3, "collectAll": true}),
    );
    let test = opened["session"].clone();
    let status = run(&mut engine, &test);
    assert_eq!(status["state"], "suspended", "{status}");
    assert_eq!(status["reason"], "targetLine");
    let failed: Vec<&Value> = status["failures"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| &f["line"])
        .collect();
    assert_eq!(failed, [&json!(2), &json!(3)]);
    let at = status["at"].clone();

    let promoted = call(
        &mut engine,
        json!({"op": "test.promote", "session": test, "path": format!("{DIR}/Add.asm"),
               "ram": [0, 4], "screen": "force"}),
    );
    assert!(promoted["error"].is_null(), "{promoted}");
    assert_ne!(promoted["session"], test);
    // The `output` that produced the row.
    assert_eq!(promoted["scriptRange"], at);
    let source = cpu_script(true);
    let output_line = source
        .lines()
        .enumerate()
        .filter(|(_, l)| *l == "output;")
        .nth(1)
        .unwrap()
        .0;
    assert_eq!(promoted["scriptRange"]["startLine"], output_line);
    assert_eq!(
        promoted["loaded"],
        json!([{"path": format!("{DIR}/Add.asm"), "builtFromSource": false}])
    );
    let state = &promoted["state"];
    assert_eq!(state["kind"], "cpu");
    assert_eq!(state["status"], "paused");
    // Three instructions in: `@R1` is the last executed, `D=D+M` is next.
    assert_eq!(register(state, "A"), 1);
    assert_eq!(register(state, "D"), 5);
    assert_eq!(register(state, "PC"), 3);
    assert_eq!(register(state, "M"), 6);
    assert_eq!(state["ram"], json!({"start": 0, "values": [5, 6, 3, 0]}));
    assert_eq!(state["file"], format!("{DIR}/Add.asm"));
    assert_eq!(state["line"], 3);
    assert!(state["screen"].is_string());

    // The test session is gone; the promoted one runs on.
    let ended = call(&mut engine, json!({"op": "test.run", "session": test}));
    assert_eq!(ended["error"]["code"], "session-ended");
    let session = promoted["session"].clone();
    let stepped = call(
        &mut engine,
        json!({"op": "cpu.step", "session": session, "ram": [0, 4]}),
    );
    assert_eq!(register(&stepped, "D"), 11);
    assert_eq!(register(&stepped, "PC"), 4);
    assert_eq!(stepped["line"], 4);
    let stepped = call(
        &mut engine,
        json!({"op": "cpu.step", "session": session, "count": 2, "ram": [0, 4]}),
    );
    assert_eq!(stepped["ram"]["values"], json!([5, 6, 11, 0]));
    let state = call(&mut engine, json!({"op": "cpu.state", "session": session}));
    assert_eq!(register(&state, "PC"), 6);
    // Nothing was written by the inspection run.
    assert!(host.get(&format!("{DIR}/Add.out")).is_none());
    assert_eq!(engine.session_count(), 1);
}

// ---------------------------------------------------------------------------
// VM
// ---------------------------------------------------------------------------

const PROG_VM: &str =
    "// Two pushes and an add.\npush constant 7\npush constant 8\nadd\npush constant 1\n";

const PROG_TST: &str = "load Prog.vm,\noutput-file Prog.out,\ncompare-to Prog.cmp,\n\
     output-list RAM[0]%D2.6.2 RAM[256]%D2.6.2 RAM[257]%D2.6.2;\n\
     set sp 256, set local 300, set argument 400, set this 3000, set that 3010;\n\
     vmstep;\noutput;\nvmstep;\noutput;\nvmstep;\noutput;\n";

#[test]
fn a_vm_test_is_promoted_at_a_failing_row() {
    let (mut engine, host) = engine();
    host.insert(&format!("{DIR}/Prog.vm"), PROG_VM);
    host.insert(&format!("{DIR}/Prog.tst"), PROG_TST);
    // Every line fails, the header too.
    host.insert(&format!("{DIR}/Prog.cmp"), "nope\nnope\nnope\nnope\n");
    let opened = call(
        &mut engine,
        json!({"op": "test.open", "path": format!("{DIR}/Prog.tst"),
               "writeOutput": false, "stopAtLine": 3}),
    );
    assert_eq!(opened["dialect"], "vm");
    let status = run(&mut engine, &opened["session"]);
    assert_eq!(status["reason"], "targetLine", "{status}");
    assert_eq!(status["failures"].as_array().unwrap().len(), 3);

    let promoted = call(
        &mut engine,
        json!({"op": "test.promote", "session": opened["session"], "ram": [256, 3]}),
    );
    assert!(promoted["error"].is_null(), "{promoted}");
    assert_eq!(
        promoted["loaded"],
        json!([{"path": format!("{DIR}/Prog.vm"), "builtFromSource": false}])
    );
    let state = &promoted["state"];
    assert_eq!(state["kind"], "vm");
    assert_eq!(state["status"], "paused");
    assert_eq!(register(state, "SP"), 258);
    assert_eq!(register(state, "LCL"), 300);
    assert_eq!(register(state, "ARG"), 400);
    assert_eq!(register(state, "THIS"), 3000);
    assert_eq!(register(state, "THAT"), 3010);
    assert_eq!(state["stack"], json!([8, 7]));
    assert_eq!(state["ram"], json!({"start": 256, "values": [7, 8, 0]}));
    assert_eq!(state["file"], format!("{DIR}/Prog.vm"));
    // Next is `add`, on line 3 (zero-based) of the file.
    assert_eq!(state["line"], 3);
    assert_eq!(state["instruction"], "add");
    assert_eq!(state["fromJack"], false);

    let session = promoted["session"].clone();
    let stepped = call(
        &mut engine,
        json!({"op": "vm.step", "session": session, "ram": [256, 3]}),
    );
    assert_eq!(stepped["stack"], json!([15]));
    assert_eq!(register(&stepped, "SP"), 257);
    assert_eq!(stepped["line"], 4);
    assert_eq!(stepped["instruction"], "push constant 1");
    assert!(host.get(&format!("{DIR}/Prog.out")).is_none());
}

const MAIN_JACK: &str = "\
class Main {
    function void main() {
        var int x;
        let x = Main.twice(5);
        do Memory.poke(8000, x);
        let x = Main.twice(x);
        do Memory.poke(8001, x);
        return;
    }

    function int twice(int n) {
        return n + n;
    }
}
";

/// One row after each of the first 80 VM commands.
const JACK_TST: &str = "load,\noutput-file Main.out,\ncompare-to Main.cmp,\n\
     output-list RAM[0]%D2.6.2;\nrepeat 80 { vmstep; output; }\n";

fn line_of(needle: &str) -> u64 {
    MAIN_JACK.lines().position(|l| l.contains(needle)).unwrap() as u64
}

/// Promotes the first row of the Jack test produced inside `Main.twice`.
fn promote_inside_twice(engine: &mut Engine, overlay: Option<Value>) -> Value {
    for line in 2..=81u64 {
        let mut request = json!({"op": "test.open", "path": format!("{DIR}/Main.tst"),
               "writeOutput": false, "buildSources": true, "stopAtLine": line});
        if let Some(overlay) = &overlay {
            request["overlay"] = overlay.clone();
        }
        let opened = call(engine, request);
        assert!(opened["error"].is_null(), "{opened}");
        let status = run(engine, &opened["session"]);
        assert_eq!(status["reason"], "targetLine", "{status}");
        let promoted = call(
            engine,
            json!({"op": "test.promote", "session": opened["session"]}),
        );
        assert!(promoted["error"].is_null(), "{promoted}");
        if promoted["state"]["function"] == "Main.twice" {
            return promoted;
        }
        call(
            engine,
            json!({"op": "close", "session": promoted["session"]}),
        );
    }
    panic!("the program never entered Main.twice");
}

#[test]
fn a_compiled_jack_test_is_promoted_with_jack_lines_and_call_stack() {
    let (mut engine, host) = engine();
    host.insert(&format!("{DIR}/Main.jack"), MAIN_JACK);
    host.insert(&format!("{DIR}/Main.tst"), JACK_TST);
    host.insert(&format!("{DIR}/Main.cmp"), "x\n".repeat(81));

    let promoted = promote_inside_twice(&mut engine, None);
    assert_eq!(engine.session_count(), 1);
    assert_eq!(
        promoted["loaded"],
        json!([{"path": format!("{DIR}/Main.jack"), "builtFromSource": true}])
    );
    let state = &promoted["state"];
    assert_eq!(state["fromJack"], true);
    assert_eq!(state["file"], format!("{DIR}/Main.jack"));
    assert_eq!(state["line"], line_of("return n + n;"));
    let frames = state["callStack"].as_array().unwrap();
    assert!(frames.len() >= 2, "{state}");
    assert_eq!(frames[0]["function"], "Main.twice");
    assert_eq!(frames[0]["arguments"], json!([5]));
    let main = frames
        .iter()
        .find(|f| f["function"] == "Main.main")
        .unwrap();
    assert_eq!(main["file"], format!("{DIR}/Main.jack"));
    assert_eq!(main["line"], line_of("let x = Main.twice(5);"));
    assert!(state["registers"].as_array().unwrap().len() == 5);
    assert!(!state["stack"].as_array().unwrap().is_empty() || register(state, "SP") != 0);

    // Step a Jack line: back in `Main.main`, on the next line.
    let session = promoted["session"].clone();
    let stepped = call(
        &mut engine,
        json!({"op": "vm.step", "session": session, "unit": "line", "ram": [8000, 2]}),
    );
    assert_eq!(stepped["status"], "paused", "{stepped}");
    assert_eq!(stepped["function"], "Main.main");
    // Run on to the end: the promoted program finishes correctly.
    let mut state = stepped;
    for _ in 0..1000 {
        state = call(
            &mut engine,
            json!({"op": "vm.run", "session": session, "budget": 10_000, "ram": [8000, 2]}),
        );
        if state["status"] != "running" {
            break;
        }
    }
    assert_eq!(state["status"], "halted", "{state}");
    assert_eq!(state["ram"]["values"], json!([10, 20]));
    // `vm.reset` reloads the program from the folder the script loaded.
    let reset = call(&mut engine, json!({"op": "vm.reset", "session": session}));
    assert!(reset["error"].is_null(), "{reset}");
    assert_eq!(reset["fromJack"], true);
    assert!(host.get(&format!("{DIR}/Main.vm")).is_none());
    assert!(host.get(&format!("{DIR}/Main.out")).is_none());
}

#[test]
fn an_unsaved_jack_buffer_is_promoted_with_its_own_lines() {
    let (mut engine, host) = engine();
    host.insert(&format!("{DIR}/Main.jack"), MAIN_JACK);
    host.insert(&format!("{DIR}/Main.tst"), JACK_TST);
    host.insert(&format!("{DIR}/Main.cmp"), "x\n".repeat(81));
    // The editor's buffer has two more lines at the top than the disk file.
    let edited = format!("// edited\n// in the editor\n{MAIN_JACK}");
    let overlay = json!([{"path": format!("{DIR}/Main.jack"), "text": edited}]);
    let promoted = promote_inside_twice(&mut engine, Some(overlay));
    let state = &promoted["state"];
    assert_eq!(state["file"], format!("{DIR}/Main.jack"));
    assert_eq!(state["line"], line_of("return n + n;") + 2, "{state}");
    let main = state["callStack"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["function"] == "Main.main")
        .unwrap();
    assert_eq!(main["line"], line_of("let x = Main.twice(5);") + 2);
    // Stepping by line follows the buffer's lines too.
    let stepped = call(
        &mut engine,
        json!({"op": "vm.step", "session": promoted["session"], "unit": "line"}),
    );
    assert_eq!(stepped["function"], "Main.main");
    // Back from the call: the `pop` that finishes the `let` is next.
    assert_eq!(stepped["instruction"], "pop local 0");
    assert_eq!(
        stepped["line"],
        line_of("let x = Main.twice(5);") + 2,
        "{stepped}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[test]
fn a_hardware_test_cannot_be_promoted() {
    let (mut engine, host) = engine();
    host.insert(
        &format!("{DIR}/Not.tst"),
        "load Not.hdl,\noutput-file Not.out,\noutput-list in out;\nset in 0, eval, output;\n",
    );
    let opened = call(
        &mut engine,
        json!({"op": "test.open", "path": format!("{DIR}/Not.tst"),
               "writeOutput": false, "stopAtLine": 2}),
    );
    let status = run(&mut engine, &opened["session"]);
    assert_eq!(status["reason"], "targetLine", "{status}");
    let reply = call(
        &mut engine,
        json!({"op": "test.promote", "session": opened["session"]}),
    );
    assert_eq!(
        reply["error"]["message"],
        "only CPU and VM test scripts can be inspected in the Hack computer or VM emulator"
    );
    // The run is untouched and still usable.
    let inspected = call(
        &mut engine,
        json!({"op": "test.inspect", "session": opened["session"]}),
    );
    assert_eq!(inspected["dialect"], "hardware");
    assert_eq!(engine.session_count(), 1);
}

#[test]
fn a_vm_test_that_has_loaded_nothing_cannot_be_promoted() {
    let (mut engine, host) = engine();
    host.insert(&format!("{DIR}/Prog.vm"), PROG_VM);
    host.insert(&format!("{DIR}/Prog.tst"), PROG_TST);
    let opened = call(
        &mut engine,
        json!({"op": "test.open", "path": format!("{DIR}/Prog.tst"), "writeOutput": false}),
    );
    let reply = call(
        &mut engine,
        json!({"op": "test.promote", "session": opened["session"]}),
    );
    assert_eq!(
        reply["error"]["message"],
        "the test script has not loaded a VM program yet"
    );
    assert_eq!(engine.session_count(), 1);
}

#[test]
fn promoting_a_missing_or_non_test_session_is_refused() {
    let (mut engine, host) = engine();
    let reply = call(&mut engine, json!({"op": "test.promote", "session": 99}));
    assert_eq!(reply["error"]["code"], "session-ended");
    host.insert(&format!("{DIR}/Add.asm"), ADD_ASM);
    let cpu = call(
        &mut engine,
        json!({"op": "cpu.open", "path": format!("{DIR}/Add.asm")}),
    );
    let reply = call(
        &mut engine,
        json!({"op": "test.promote", "session": cpu["session"]}),
    );
    assert!(reply["error"]["message"].is_string(), "{reply}");
    assert_eq!(engine.session_count(), 1);
}

#[test]
fn promoted_missing_vm_compiled_from_jack_can_reset() {
    let (mut engine, host) = engine();
    host.insert(&format!("{DIR}/Main.jack"), MAIN_JACK);
    host.insert(
        &format!("{DIR}/Main.tst"),
        "load Main.vm,\noutput-file Main.out,\ncompare-to Main.cmp,\noutput-list RAM[0]%D2.6.2;\nrepeat 5 { vmstep; }\noutput;\n",
    );
    host.insert(&format!("{DIR}/Main.cmp"), "x\nx\n");

    let opened = call(
        &mut engine,
        json!({
            "op": "test.open",
            "path": format!("{DIR}/Main.tst"),
            "writeOutput": false,
            "buildSources": true,
            "stopAtLine": 2
        }),
    );
    assert!(opened["error"].is_null(), "{opened}");
    let status = run(&mut engine, &opened["session"]);
    assert_eq!(status["reason"], "targetLine", "{status}");

    let promoted = call(
        &mut engine,
        json!({"op": "test.promote", "session": opened["session"]}),
    );
    assert!(promoted["error"].is_null(), "{promoted}");
    assert_eq!(promoted["state"]["fromJack"], true);

    let reset = call(
        &mut engine,
        json!({"op": "vm.reset", "session": promoted["session"]}),
    );
    assert!(reset["error"].is_null(), "{reset}");
    assert_eq!(reset["steps"], 0);
    assert_eq!(reset["fromJack"], true);
}
