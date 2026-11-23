//! Regression tests for the findings of the end-of-project code review,
//! through the same JSON API the editor uses.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use n2t_core::host::{DirEntry, Host, HostError, MemoryHost};
use n2t_wasm::Engine;
use serde_json::{Value, json};

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

const DIR: &str = "/home/student/nand2tetris";

fn host() -> Arc<MemoryHost> {
    let host = Arc::new(MemoryHost::new());
    for file in ["Majority.hdl", "Majority.tst", "Majority.cmp"] {
        host.insert(&format!("{DIR}/{file}"), fixture(&format!("chips/{file}")));
    }
    host
}

fn call(engine: &mut Engine, request: Value) -> Value {
    serde_json::from_str(&engine.call(&request.to_string())).unwrap()
}

/// A program that copies the keyboard to RAM[1] after a few instructions.
const READ_KEYBOARD: &str =
    "@0\nD=A\n@0\nD=A\n@75\nD=A\n@24576\nD=M\n@1\nM=D\n(END)\n@END\n0;JMP\n";

fn memory_cell(inspection: &Value, name: &str) -> u64 {
    inspection["state"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["name"] == name)
        .unwrap_or_else(|| panic!("no {name} in {inspection}"))["bits"]
        .as_u64()
        .unwrap()
}

/// Runs a test session to its end and returns the last status and every
/// event along the way.
fn finish(engine: &mut Engine, session: &Value) -> (Value, Vec<Value>) {
    let mut events = Vec::new();
    for _ in 0..10_000 {
        let status = call(
            engine,
            json!({"op": "test.run", "session": session, "budget": 50}),
        );
        events.extend(status["events"].as_array().cloned().unwrap_or_default());
        if status["state"] == "finished" {
            return (status, events);
        }
    }
    panic!("the script did not finish");
}

// --- Replacing a student's file -----------------------------------------

#[test]
fn a_file_is_replaced_only_as_it_was_when_the_student_was_asked() {
    let host = host();
    let mut engine = Engine::new(host.clone());
    let asm = format!("{DIR}/Prog.asm");
    let hack = format!("{DIR}/Prog.hack");
    host.insert(&asm, "@2\nD=A\n");
    let right = "0000000000000010\n1110110000010000\n";
    // The student's own assembler wrote something else.
    host.insert(&hack, "0000000000000010\n1111111111111111\n");

    // The `.hack` is open in the editor with unsaved text that happens to be
    // right. That says nothing about the file a write would replace.
    call(
        &mut engine,
        json!({"op": "overlay.set", "path": hack, "text": right}),
    );
    let planned = call(&mut engine, json!({"op": "asm.assemble", "path": asm}));
    assert_eq!(planned["outputs"][0]["existing"], "different", "{planned}");
    assert_eq!(planned["outputs"][0]["text"], right);
    let state = planned["outputs"][0]["state"].clone();

    // A write that does not say what it replaces writes nothing.
    let refused = call(
        &mut engine,
        json!({"op": "asm.assemble", "path": asm, "write": true}),
    );
    assert_eq!(refused["written"], false, "{refused}");
    assert_eq!(refused["conflicts"], json!([hack]));
    assert!(host.get_text(&hack).unwrap().contains("1111111111111111"));

    // The file changes while the student is deciding (their assembler ran
    // again, or a sync client restored it): the approval no longer applies.
    host.insert(&hack, "0000000000000010\n0000000000000000\n");
    let stale = call(
        &mut engine,
        json!({"op": "asm.assemble", "path": asm, "write": true, "expect": {hack.as_str(): state}}),
    );
    assert_eq!(stale["written"], false, "{stale}");
    assert_eq!(stale["conflicts"], json!([hack]));
    assert_eq!(
        host.get_text(&hack).unwrap(),
        "0000000000000010\n0000000000000000\n"
    );

    // With the state of the file as it is now, it is replaced.
    let written = call(
        &mut engine,
        json!({"op": "asm.assemble", "path": asm, "write": true,
               "expect": {hack.as_str(): stale["outputs"][0]["state"]}}),
    );
    assert_eq!(written["written"], true, "{written}");
    assert_eq!(written["outputs"][0]["written"], true);
    assert_eq!(host.get_text(&hack).unwrap(), right);

    // A file that already says the same is not rewritten: its line endings
    // stay as they are.
    host.insert(&hack, right.replace('\n', "\r\n"));
    let again = call(
        &mut engine,
        json!({"op": "asm.assemble", "path": asm, "write": true}),
    );
    assert_eq!(again["outputs"][0]["existing"], "same", "{again}");
    assert_eq!(again["outputs"][0]["written"], false);
    assert_eq!(again["conflicts"], json!([]));
    assert_eq!(host.get_text(&hack).unwrap(), right.replace('\n', "\r\n"));
}

#[test]
fn compiling_a_folder_writes_every_file_or_none() {
    let host = host();
    let mut engine = Engine::new(host.clone());
    for file in ["Main.jack", "Counter.jack"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../jack/tests/fixtures/Counter")
            .join(file);
        host.insert(&format!("{DIR}/{file}"), std::fs::read(path).unwrap());
    }
    // One of the two outputs exists with other content.
    host.insert(&format!("{DIR}/Main.vm"), "function Main.main 0\n");
    let refused = call(
        &mut engine,
        json!({"op": "jack.compile", "path": DIR, "write": true}),
    );
    assert_eq!(refused["written"], false, "{refused}");
    assert_eq!(refused["conflicts"], json!([format!("{DIR}/Main.vm")]));
    assert!(
        host.get(&format!("{DIR}/Counter.vm")).is_none(),
        "the other file must not be written either"
    );
}

// --- Stepping must never block the engine --------------------------------

#[test]
fn a_step_over_a_loop_that_never_ends_comes_back() {
    let host = host();
    host.insert(
        &format!("{DIR}/Forever.tst"),
        "load Majority.hdl;\nwhile out <> 1 { eval, }\n",
    );
    let mut engine = Engine::new(host);
    let opened = call(
        &mut engine,
        json!({"op": "test.open", "path": format!("{DIR}/Forever.tst")}),
    );
    let session = &opened["session"];
    let status = call(&mut engine, json!({"op": "test.step", "session": session}));
    assert_eq!(status["reason"], "step", "{status}");
    // The loop has no `;` and never exits. The step stops at its budget, and
    // asking again carries on from there.
    let status = call(
        &mut engine,
        json!({"op": "test.step", "session": session, "budget": 1000}),
    );
    assert_eq!(status["state"], "suspended", "{status}");
    assert_eq!(status["reason"], "budget");
    assert_eq!(status["commands"], 1001);
    let status = call(
        &mut engine,
        json!({"op": "test.step", "session": session, "budget": 1000}),
    );
    assert_eq!(status["reason"], "budget");
    assert_eq!(status["commands"], 2001);
}

// --- Output files ----------------------------------------------------------

/// A host whose writes can be made to fail, like a disk that is full or a
/// file another program holds open.
struct Flaky {
    inner: Arc<MemoryHost>,
    fail_writes: AtomicBool,
}

impl Host for Flaky {
    fn read(&self, path: &str) -> Result<Vec<u8>, HostError> {
        self.inner.read(path)
    }
    fn list(&self, dir: &str) -> Result<Vec<DirEntry>, HostError> {
        self.inner.list(dir)
    }
    fn write(&self, path: &str, data: &[u8]) -> Result<(), HostError> {
        if self.fail_writes.load(Ordering::Relaxed) {
            return Err(HostError::Other("the disk is full".into()));
        }
        self.inner.write(path, data)
    }
}

#[test]
fn an_output_file_that_cannot_be_written_fails_the_run_at_once() {
    let flaky = Arc::new(Flaky {
        inner: host(),
        fail_writes: AtomicBool::new(true),
    });
    let mut engine = Engine::new(flaky.clone());
    let opened = call(
        &mut engine,
        json!({"op": "test.open", "path": format!("{DIR}/Majority.tst")}),
    );
    // The first slice pauses at its budget, long before the script ends. The
    // failed write must not be forgotten there.
    let status = call(
        &mut engine,
        json!({"op": "test.run", "session": opened["session"], "budget": 5}),
    );
    assert_eq!(status["state"], "finished", "{status}");
    assert_eq!(status["outcome"]["kind"], "error");
    let message = status["outcome"]["message"].as_str().unwrap();
    assert!(message.contains("cannot write `Majority.out`"), "{message}");
    // It stays failed even if the disk recovers: there is no passing result
    // without the output file.
    flaky.fail_writes.store(false, Ordering::Relaxed);
    let status = call(
        &mut engine,
        json!({"op": "test.run", "session": opened["session"], "budget": 5000}),
    );
    assert_eq!(status["outcome"]["kind"], "error", "{status}");
}

#[test]
fn after_the_first_failure_no_other_output_file_is_touched() {
    let host = host();
    host.insert(&format!("{DIR}/Bad.cmp"), "| a |out|\n| 0 | 1 |\n");
    host.insert(
        &format!("{DIR}/Two.tst"),
        "load Majority.hdl,\noutput-file First.out,\ncompare-to Bad.cmp,\noutput-list a%B1.1.1 out%B1.1.1;\n\
         set a 0, set b 0, set c 0, eval, output;\n\
         output-file Second.out,\noutput-list a%B1.1.1;\neval, output;\n",
    );
    // Something the student wants to keep.
    host.insert(&format!("{DIR}/Second.out"), "keep me\n");
    let mut engine = Engine::new(host.clone());
    let opened = call(
        &mut engine,
        json!({"op": "test.open", "path": format!("{DIR}/Two.tst")}),
    );
    let (status, _) = finish(&mut engine, &opened["session"]);
    assert_eq!(status["outcome"]["kind"], "failed", "{status}");
    assert_eq!(status["outcome"]["line"], 2);
    // The official simulator stops at the failure and never reaches the
    // second `output-file`. Neither does anything that touches the disk.
    assert_eq!(
        host.get_text(&format!("{DIR}/Second.out")).unwrap(),
        "keep me\n"
    );
    assert_eq!(
        host.get_text(&format!("{DIR}/First.out")).unwrap(),
        "| a |out|\n| 0 | 0 |\n"
    );
}

// --- Keys are pressed only for scripts that wait for the keyboard --------

#[test]
fn a_loop_that_waits_for_a_register_does_not_get_a_key_pressed() {
    let host = host();
    host.insert(&format!("{DIR}/Keys.asm"), READ_KEYBOARD);
    host.insert(
        &format!("{DIR}/Register.tst"),
        "load Keys.asm;\nwhile A <> 75 { ticktock; }\nrepeat 6 { ticktock; }\n",
    );
    host.insert(
        &format!("{DIR}/Keyboard.tst"),
        "load Keys.asm;\nwhile RAM[24576] <> 75 { ticktock; }\nrepeat 12 { ticktock; }\n",
    );
    let mut engine = Engine::new(host);

    // `A` reaches 75 by itself. The program then reads the keyboard, which
    // nobody has touched.
    let opened = call(
        &mut engine,
        json!({"op": "test.open", "path": format!("{DIR}/Register.tst")}),
    );
    let (_, events) = finish(&mut engine, &opened["session"]);
    assert!(events.iter().all(|e| e["type"] != "key"), "{events:?}");
    let inspection = call(
        &mut engine,
        json!({"op": "test.inspect", "session": opened["session"]}),
    );
    assert_eq!(memory_cell(&inspection, "RAM[1]"), 0, "{inspection}");

    // A loop on the keyboard's own memory cell is a wait for a key.
    let opened = call(
        &mut engine,
        json!({"op": "test.open", "path": format!("{DIR}/Keyboard.tst")}),
    );
    let (_, events) = finish(&mut engine, &opened["session"]);
    let keys: Vec<&Value> = events.iter().filter(|e| e["type"] == "key").collect();
    assert_eq!(keys.len(), 1, "{events:?}");
    assert_eq!(keys[0]["code"], 75);
}

#[test]
fn a_key_code_that_is_not_a_key_is_refused() {
    let host = host();
    host.insert(&format!("{DIR}/Keys.asm"), READ_KEYBOARD);
    let mut engine = Engine::new(host);
    let cpu = call(
        &mut engine,
        json!({"op": "cpu.open", "path": format!("{DIR}/Keys.asm")}),
    );
    // 65611 is 75 after losing its high bits; it must not become a K.
    for code in [
        json!(65611),
        json!(-1),
        json!(1.5),
        json!("75"),
        Value::Null,
    ] {
        let reply = call(
            &mut engine,
            json!({"op": "key", "session": cpu["session"], "code": code}),
        );
        assert!(
            reply["error"]["message"].is_string(),
            "{code} was accepted: {reply}"
        );
    }
    let state = call(
        &mut engine,
        json!({"op": "cpu.state", "session": cpu["session"]}),
    );
    assert_eq!(state["key"], 0);
    let reply = call(
        &mut engine,
        json!({"op": "key", "session": cpu["session"], "code": 75}),
    );
    assert_eq!(reply["accepted"], true);
}

// --- Unsaved buffers --------------------------------------------------------

#[test]
fn a_buffer_shadows_only_the_file_it_belongs_to() {
    let host = host();
    // Two files, on a file system where case matters.
    host.insert(&format!("{DIR}/overwrite.asm"), "@1\n");
    host.insert(&format!("{DIR}/Overwrite.asm"), "@2\n");
    let mut engine = Engine::new(host);
    let lower = format!("{DIR}/overwrite.asm");
    let upper = format!("{DIR}/Overwrite.asm");
    call(
        &mut engine,
        json!({"op": "overlay.replace", "files": [{"path": lower, "text": "@7\n", "ignoreCase": false}]}),
    );
    let opened = call(&mut engine, json!({"op": "cpu.open", "path": upper}));
    assert_eq!(opened["instruction"], "@2", "{opened}");
    let opened = call(&mut engine, json!({"op": "cpu.open", "path": lower}));
    assert_eq!(opened["instruction"], "@7", "{opened}");

    // Where the file system ignores case, `load and.hdl` must find the
    // unsaved text of `And.hdl`; the editor says which it is.
    call(
        &mut engine,
        json!({"op": "overlay.replace", "files": [{"path": lower, "text": "@7\n", "ignoreCase": true}]}),
    );
    let opened = call(&mut engine, json!({"op": "cpu.open", "path": upper}));
    assert_eq!(opened["instruction"], "@7", "{opened}");

    // A Windows path ignores case unless told otherwise.
    call(
        &mut engine,
        json!({"op": "overlay.replace", "files": [{"path": r"c:\n2t\prog.asm", "text": "@9\n"}]}),
    );
    let opened = call(
        &mut engine,
        json!({"op": "cpu.open", "path": "C:/n2t/Prog.asm"}),
    );
    assert_eq!(opened["instruction"], "@9", "{opened}");
    // Replacing the buffers dropped the earlier ones in the same step.
    let opened = call(&mut engine, json!({"op": "cpu.open", "path": lower}));
    assert_eq!(opened["instruction"], "@1", "{opened}");
}

#[test]
fn a_test_run_keeps_the_editor_text_it_started_with() {
    let host = host();
    let mut engine = Engine::new(host);
    let chip = format!("{DIR}/Majority.hdl");
    let broken =
        fixture("chips/Majority.hdl").replace("And(a=a, b=c, out=ac);", "Or(a=a, b=c, out=ac);");
    call(
        &mut engine,
        json!({"op": "overlay.replace", "files": [{"path": chip, "text": broken}]}),
    );
    let opened = call(
        &mut engine,
        json!({"op": "test.open", "path": format!("{DIR}/Majority.tst")}),
    );
    // Something else syncs the editor's buffers before the script reaches
    // its `load` (another run starting, or the student saving).
    call(&mut engine, json!({"op": "overlay.replace", "files": []}));
    let (status, _) = finish(&mut engine, &opened["session"]);
    assert_eq!(
        status["outcome"]["kind"], "failed",
        "the run must test the text it was started on: {status}"
    );
    // A run started now sees the saved file.
    let opened = call(
        &mut engine,
        json!({"op": "test.open", "path": format!("{DIR}/Majority.tst")}),
    );
    let (status, _) = finish(&mut engine, &opened["session"]);
    assert_eq!(status["outcome"]["kind"], "passed", "{status}");
}

// --- Breakpoints and sessions ---------------------------------------------

#[test]
fn run_leaves_the_breakpoint_it_is_standing_on_and_stops_at_it_next_time_round() {
    let host = host();
    host.insert(
        &format!("{DIR}/Sys.vm"),
        "function Sys.init 0\npush constant 0\npop temp 0\nlabel LOOP\npush temp 0\npush constant 1\nadd\npop temp 0\ngoto LOOP\n",
    );
    let mut engine = Engine::new(host);
    let file = format!("{DIR}/Sys.vm");
    let opened = call(&mut engine, json!({"op": "vm.open", "path": file}));
    let session = &opened["session"];
    // A breakpoint on `add`, inside the loop.
    let breakpoints = json!([{"path": file, "lines": [6]}]);
    let run = |engine: &mut Engine, resume: bool| {
        call(
            engine,
            json!({"op": "vm.run", "session": session, "budget": 1000, "now": 0,
                   "breakpoints": breakpoints, "resume": resume}),
        )
    };
    let first = run(&mut engine, true);
    assert_eq!(first["status"], "breakpoint", "{first}");
    assert_eq!(first["line"], 6);
    // Pressing Run again executes `add` and comes back to it one turn of
    // the loop later, instead of standing still.
    let second = run(&mut engine, true);
    assert_eq!(second["status"], "breakpoint", "{second}");
    assert_eq!(second["line"], 6);
    assert!(
        second["steps"].as_u64().unwrap() > first["steps"].as_u64().unwrap(),
        "{second}"
    );
    // A later slice of the same run does stop on arriving there.
    let third = run(&mut engine, false);
    assert_eq!(third["steps"], second["steps"], "{third}");
}

#[test]
fn a_restarted_engine_does_not_reuse_session_numbers() {
    let host = host();
    let mut engine = Engine::new(host.clone());
    let chip = format!("{DIR}/Majority.hdl");
    let old = call(&mut engine, json!({"op": "chip.open", "path": chip}));
    assert_eq!(old["session"], 1);

    // The engine crashed and was replaced; its sessions start elsewhere.
    let mut engine = Engine::new(host);
    call(
        &mut engine,
        json!({"op": "start", "firstSession": 4_294_967_297u64}),
    );
    let new = call(&mut engine, json!({"op": "chip.open", "path": chip}));
    assert_eq!(new["session"], 4_294_967_297u64);
    // A late request from the owner of the old session finds nothing, and
    // closing it closes nothing.
    let reply = call(
        &mut engine,
        json!({"op": "chip.eval", "session": old["session"]}),
    );
    assert!(
        reply["error"]["message"]
            .as_str()
            .unwrap()
            .contains("ended"),
        "{reply}"
    );
    call(
        &mut engine,
        json!({"op": "close", "session": old["session"]}),
    );
    assert_eq!(engine.session_count(), 1);
    // `start` only applies to a fresh engine.
    call(&mut engine, json!({"op": "start", "firstSession": 1}));
    let next = call(&mut engine, json!({"op": "chip.open", "path": chip}));
    assert_eq!(next["session"], 4_294_967_298u64);
}
