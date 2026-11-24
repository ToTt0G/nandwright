//! End-to-end tests for `chip.diagram` and `chip.values` driven through `Engine::call`.

use std::sync::Arc;

use n2t_core::host::MemoryHost;
use n2t_wasm::Engine;
use serde_json::{Value, json};

const DIR: &str = "C:/Users/me/chips";

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

fn call(engine: &mut Engine, request: Value) -> Value {
    serde_json::from_str(&engine.call(&request.to_string())).unwrap()
}

const LEAF_PAIR_HDL: &str = "\
CHIP LeafPair {
    IN x, y;
    OUT sum, carry;

    PARTS:
    Xor(a=x, b=y, out=sum);
    And(a=x, b=y, out=carry);
}
";

const MID_STAGE_HDL: &str = "\
CHIP MidStage {
    IN a, b, c;
    OUT out, extra;

    PARTS:
    LeafPair(x=a, y=b, sum=abSum);
    HalfAdder(a=abSum, b=true, sum=toggled);
    LeafPair(
        x=c,
        y=false,
        sum=cCopy,
        carry=alwaysZero
    );
    Xor(a=toggled, b=cCopy, out=out);
    Or(a=alwaysZero, b=abSum, out=extra);
}
";

const TOP_ASSEMBLY_HDL: &str = "\
CHIP TopAssembly {
    IN p, q, r;
    OUT y;

    PARTS:
    MidStage(a=p, b=q, c=r, out=midOut);
    Not(in=midOut, out=y);
}
";

const BYTE_MIX_HDL: &str = "\
CHIP ByteMix {
    IN lo[8], hi[8];
    OUT out[16];

    PARTS:
    And16(a[0..7]=lo, a[8..15]=hi, b=true, out=out);
}
";

const BUILTIN_WRAP_HDL: &str = "\
CHIP BuiltinWrap {
    IN a, b;
    OUT out;

    BUILTIN Nand;
}
";

#[test]
fn root_diagram_of_composite_chip_matches_contract() {
    let host = Arc::new(MemoryHost::new().with(
        &format!("{DIR}/Majority.hdl"),
        fixture("chips/Majority.hdl"),
    ));
    let mut engine = Engine::new(host);

    let opened = call(
        &mut engine,
        json!({"op": "chip.open", "path": format!("{DIR}/Majority.hdl")}),
    );
    let session = &opened["session"];

    let diagram = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": session, "path": []}),
    );
    assert_eq!(opened["revision"], 1);
    assert_eq!(diagram["revision"], 1);
    assert_eq!(diagram["path"], json!([]));
    assert_eq!(diagram["values"]["revision"], 1);
    assert_eq!(diagram["values"]["path"], json!([]));
    assert_eq!(diagram["chip"], "Majority");
    assert_eq!(diagram["file"], format!("{DIR}/Majority.hdl"));
    assert_eq!(
        diagram["breadcrumb"],
        json!([{"chip": "Majority", "path": []}])
    );
    assert_eq!(
        diagram["inputs"],
        json!([
            {"name": "a", "width": 1},
            {"name": "b", "width": 1},
            {"name": "c", "width": 1}
        ])
    );
    assert_eq!(diagram["outputs"], json!([{"name": "out", "width": 1}]));
    assert_eq!(
        diagram["nets"],
        json!([
            {"name": "a", "width": 1, "kind": "input"},
            {"name": "b", "width": 1, "kind": "input"},
            {"name": "c", "width": 1, "kind": "input"},
            {"name": "out", "width": 1, "kind": "output"},
            {"name": "ab", "width": 1, "kind": "internal"},
            {"name": "ac", "width": 1, "kind": "internal"},
            {"name": "bc", "width": 1, "kind": "internal"},
            {"name": "abOrAc", "width": 1, "kind": "internal"}
        ])
    );

    let parts = diagram["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 5);
    assert_eq!(parts[0]["chip"], "And");
    assert_eq!(parts[0]["drill"], false);
    assert_eq!(parts[0]["builtin"], true);
    assert_eq!(parts[0]["sequential"], false);
    assert_eq!(parts[0]["deps"], json!([[0, 1]]));
    // Line 9 (0-based) is `    And(a=a, b=b, out=ab);` in Majority.hdl.
    assert_eq!(parts[0]["span"]["line"], 9);
    assert_eq!(parts[0]["span"]["endLine"], 9);
    assert_eq!(
        parts[0]["conns"],
        json!([
            {"dir": "in", "pin": 0, "pinBits": [0, 1], "net": 0, "netBits": [0, 1]},
            {"dir": "in", "pin": 1, "pinBits": [0, 1], "net": 1, "netBits": [0, 1]},
            {"dir": "out", "pin": 0, "pinBits": [0, 1], "net": 4, "netBits": [0, 1]}
        ])
    );

    // Combinational chip values: clocked = false, time = "".
    assert_eq!(diagram["values"]["clocked"], false);
    assert_eq!(diagram["values"]["time"], "");
    assert_eq!(diagram["values"]["nets"], json!([0, 0, 0, 0, 0, 0, 0, 0]));
    assert_eq!(diagram["values"]["parts"].as_array().unwrap().len(), 5);
}

#[test]
fn drill_two_levels_down_and_read_values_at_depth_including_constants_and_unconnected_outputs() {
    let host = Arc::new(
        MemoryHost::new()
            .with(&format!("{DIR}/LeafPair.hdl"), LEAF_PAIR_HDL)
            .with(&format!("{DIR}/MidStage.hdl"), MID_STAGE_HDL)
            .with(&format!("{DIR}/TopAssembly.hdl"), TOP_ASSEMBLY_HDL),
    );
    let mut engine = Engine::new(host);

    let opened = call(
        &mut engine,
        json!({"op": "chip.open", "path": format!("{DIR}/TopAssembly.hdl")}),
    );
    let session = &opened["session"];

    // Root diagram: part 0 is composite MidStage (`drill: true, builtin: false`).
    let root = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": session, "path": []}),
    );
    assert_eq!(root["parts"][0]["chip"], "MidStage");
    assert_eq!(root["parts"][0]["drill"], true);
    assert_eq!(root["parts"][0]["builtin"], false);
    assert_eq!(root["parts"][1]["chip"], "Not");
    assert_eq!(root["parts"][1]["drill"], false);
    assert_eq!(root["parts"][1]["builtin"], true);

    // Drill level 1: MidStage at [0].
    let mid = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": session, "path": [0]}),
    );
    assert_eq!(mid["path"], json!([0]));
    assert_eq!(mid["chip"], "MidStage");
    assert_eq!(mid["file"], format!("{DIR}/MidStage.hdl"));
    assert_eq!(
        mid["breadcrumb"],
        json!([
            {"chip": "TopAssembly", "path": []},
            {"chip": "MidStage", "path": [0]}
        ])
    );
    // Check constant connection on part 1 (HalfAdder, b=true) and multi-line span on part 2.
    assert_eq!(
        mid["parts"][1]["conns"],
        json!([
            {"dir": "in", "pin": 0, "pinBits": [0, 1], "net": 5, "netBits": [0, 1]},
            {"dir": "in", "pin": 1, "pinBits": [0, 1], "constant": true},
            {"dir": "out", "pin": 0, "pinBits": [0, 1], "net": 6, "netBits": [0, 1]}
        ])
    );
    assert_eq!(mid["parts"][2]["span"]["line"], 7);
    assert_eq!(mid["parts"][2]["span"]["endLine"], 12);

    // Drill level 2: LeafPair at [0, 0].
    let leaf = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": session, "path": [0, 0]}),
    );
    assert_eq!(leaf["path"], json!([0, 0]));
    assert_eq!(leaf["chip"], "LeafPair");
    assert_eq!(leaf["file"], format!("{DIR}/LeafPair.hdl"));
    assert_eq!(
        leaf["breadcrumb"],
        json!([
            {"chip": "TopAssembly", "path": []},
            {"chip": "MidStage", "path": [0]},
            {"chip": "LeafPair", "path": [0, 0]}
        ])
    );
    assert_eq!(leaf["parts"][0]["span"]["line"], 5);
    assert_eq!(leaf["parts"][1]["span"]["line"], 6);

    // Set p=1, q=1, r=0 and evaluate.
    call(
        &mut engine,
        json!({"op": "chip.set", "session": session, "pin": "p", "value": 1}),
    );
    call(
        &mut engine,
        json!({"op": "chip.set", "session": session, "pin": "q", "value": 1, "eval": true}),
    );

    // Read live values at depth 2 ([0, 0] = LeafPair): x=1, y=1 -> sum=0, carry=1 (unconnected in MidStage!).
    let leaf_vals = call(
        &mut engine,
        json!({"op": "chip.values", "session": session, "path": [0, 0]}),
    );
    assert_eq!(leaf_vals["revision"], 1);
    assert_eq!(leaf_vals["path"], json!([0, 0]));
    assert_eq!(leaf_vals["clocked"], false);
    assert_eq!(leaf_vals["time"], "");
    assert_eq!(leaf_vals["nets"], json!([1, 1, 0, 1]));
    assert_eq!(
        leaf_vals["parts"],
        json!([
            {"inputs": [1, 1], "outputs": [0]},
            {"inputs": [1, 1], "outputs": [1]}
        ])
    );

    // Read live values at depth 1 ([0] = MidStage):
    // - part 0 (LeafPair): inputs [1, 1], outputs [0, 1] (unconnected carry = 1!)
    // - part 1 (HalfAdder): inputs [0, 1] (constant b=true!), outputs [1, 0]
    // - part 2 (LeafPair): inputs [0, 0] (constant y=false!), outputs [0, 0]
    let mid_vals = call(
        &mut engine,
        json!({"op": "chip.values", "session": session, "path": [0]}),
    );
    assert_eq!(mid_vals["revision"], 1);
    assert_eq!(mid_vals["path"], json!([0]));
    assert_eq!(
        mid_vals["parts"][0],
        json!({"inputs": [1, 1], "outputs": [0, 1]})
    );
    assert_eq!(
        mid_vals["parts"][1],
        json!({"inputs": [0, 1], "outputs": [1, 0]})
    );
    assert_eq!(
        mid_vals["parts"][2],
        json!({"inputs": [0, 0], "outputs": [0, 0]})
    );

    // Now set q=0 and eval -> HalfAdder(1, true) has unconnected carry = 1!
    call(
        &mut engine,
        json!({"op": "chip.set", "session": session, "pin": "q", "value": 0, "eval": true}),
    );
    let mid_vals = call(
        &mut engine,
        json!({"op": "chip.values", "session": session, "path": [0]}),
    );
    assert_eq!(
        mid_vals["parts"][1],
        json!({"inputs": [1, 1], "outputs": [0, 1]})
    );
    // MidStage's `extra` output net (index 4) is unconnected in TopAssembly, yet live = 1.
    assert_eq!(mid_vals["nets"][4], json!(1));
}

#[test]
fn sequential_chip_diagram_and_values_update_on_tick_tock_and_reset() {
    let host = Arc::new(MemoryHost::new().with(
        &format!("{DIR}/Accumulator.hdl"),
        fixture("chips/Accumulator.hdl"),
    ));
    let mut engine = Engine::new(host);

    let opened = call(
        &mut engine,
        json!({"op": "chip.open", "path": format!("{DIR}/Accumulator.hdl")}),
    );
    let session = &opened["session"];

    let diagram = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": session, "path": []}),
    );
    assert_eq!(diagram["values"]["clocked"], true);
    assert_eq!(diagram["values"]["time"], "0");
    // Part 3 is Register: sequential = true, combinational deps = [[]] (clocked out depends on no input combinationally).
    assert_eq!(diagram["parts"][3]["chip"], "Register");
    assert_eq!(diagram["parts"][3]["sequential"], true);
    assert_eq!(diagram["parts"][3]["deps"], json!([[]]));
    // Register has `load=true` constant and fans out `out` to both `total` and `out`.
    let reg_conns = diagram["parts"][3]["conns"].as_array().unwrap();
    assert!(
        reg_conns
            .iter()
            .any(|c| c["dir"] == "in" && c["constant"] == true)
    );
    assert_eq!(reg_conns.iter().filter(|c| c["dir"] == "out").count(), 2);

    call(
        &mut engine,
        json!({"op": "chip.set", "session": session, "pin": "in", "value": 10}),
    );
    call(
        &mut engine,
        json!({"op": "chip.set", "session": session, "pin": "add", "value": 1}),
    );

    // Tick: time becomes "0+", outputs still 0, Register inputs see stored=10, load=1.
    call(&mut engine, json!({"op": "chip.tick", "session": session}));
    let vals = call(
        &mut engine,
        json!({"op": "chip.values", "session": session, "path": []}),
    );
    assert_eq!(vals["clocked"], true);
    assert_eq!(vals["time"], "0+");
    assert_eq!(vals["parts"][3], json!({"inputs": [10, 1], "outputs": [0]}));

    // Tock: time becomes "1", Register outputs 10, Add16 computes sum=20, next=20, stored=20.
    call(&mut engine, json!({"op": "chip.tock", "session": session}));
    let vals = call(
        &mut engine,
        json!({"op": "chip.values", "session": session, "path": []}),
    );
    assert_eq!(vals["time"], "1");
    assert_eq!(
        vals["parts"][3],
        json!({"inputs": [20, 1], "outputs": [10]})
    );

    // Reset rebuilds probes cleanly and keeps inputs (in=10, add=1).
    call(&mut engine, json!({"op": "chip.reset", "session": session}));
    let vals = call(
        &mut engine,
        json!({"op": "chip.values", "session": session, "path": []}),
    );
    assert_eq!(vals["time"], "0");
    assert_eq!(vals["parts"][3], json!({"inputs": [10, 1], "outputs": [0]}));
}

#[test]
fn partial_bus_slices_and_constant_wide_pins() {
    let host = Arc::new(MemoryHost::new().with(&format!("{DIR}/ByteMix.hdl"), BYTE_MIX_HDL));
    let mut engine = Engine::new(host);
    let opened = call(
        &mut engine,
        json!({"op": "chip.open", "path": format!("{DIR}/ByteMix.hdl")}),
    );
    let session = &opened["session"];
    call(
        &mut engine,
        json!({"op": "chip.set", "session": session, "pin": "lo", "value": "0x34"}),
    );
    call(
        &mut engine,
        json!({"op": "chip.set", "session": session, "pin": "hi", "value": "0x12", "eval": true}),
    );
    let diagram = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": session, "path": []}),
    );
    assert_eq!(
        diagram["parts"][0]["conns"],
        json!([
            {"dir": "in", "pin": 0, "pinBits": [0, 8], "net": 0, "netBits": [0, 8]},
            {"dir": "in", "pin": 0, "pinBits": [8, 8], "net": 1, "netBits": [0, 8]},
            {"dir": "in", "pin": 1, "pinBits": [0, 16], "constant": true},
            {"dir": "out", "pin": 0, "pinBits": [0, 16], "net": 2, "netBits": [0, 16]}
        ])
    );
    assert_eq!(
        diagram["values"]["parts"][0],
        json!({"inputs": [0x1234, 0xFFFF], "outputs": [0x1234]})
    );
    assert_eq!(diagram["values"]["nets"], json!([0x34, 0x12, 0x1234]));
}

#[test]
fn invalid_paths_and_builtin_targets_return_errors() {
    let host = Arc::new(
        MemoryHost::new()
            .with(&format!("{DIR}/LeafPair.hdl"), LEAF_PAIR_HDL)
            .with(&format!("{DIR}/MidStage.hdl"), MID_STAGE_HDL)
            .with(&format!("{DIR}/TopAssembly.hdl"), TOP_ASSEMBLY_HDL)
            .with(&format!("{DIR}/BuiltinWrap.hdl"), BUILTIN_WRAP_HDL),
    );
    let mut engine = Engine::new(host);

    let opened = call(
        &mut engine,
        json!({"op": "chip.open", "path": format!("{DIR}/TopAssembly.hdl")}),
    );
    let session = &opened["session"];

    // Out-of-range part index.
    let err = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": session, "path": [99]}),
    );
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("out of range"),
        "{err}"
    );

    // Pointing at a built-in part (Not at [1]).
    let err = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": session, "path": [1]}),
    );
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("built-in"),
        "{err}"
    );

    // Descending through a built-in part.
    let err = call(
        &mut engine,
        json!({"op": "chip.values", "session": session, "path": [1, 0]}),
    );
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("built-in"),
        "{err}"
    );

    // Malformed path types.
    let err = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": session, "path": [-1]}),
    );
    assert!(err["error"]["message"].is_string(), "{err}");

    let err = call(
        &mut engine,
        json!({"op": "chip.values", "session": session, "path": "bad"}),
    );
    assert!(err["error"]["message"].is_string(), "{err}");

    // Root chip that is itself built-in cannot be diagrammed.
    let builtin_open = call(
        &mut engine,
        json!({"op": "chip.open", "path": format!("{DIR}/BuiltinWrap.hdl")}),
    );
    let err = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": builtin_open["session"], "path": []}),
    );
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("built-in"),
        "{err}"
    );
}

#[test]
fn reset_rebuilds_probes_after_overlay_edit() {
    let host = Arc::new(
        MemoryHost::new()
            .with(&format!("{DIR}/LeafPair.hdl"), LEAF_PAIR_HDL)
            .with(&format!("{DIR}/MidStage.hdl"), MID_STAGE_HDL)
            .with(&format!("{DIR}/TopAssembly.hdl"), TOP_ASSEMBLY_HDL),
    );
    let mut engine = Engine::new(host);
    let opened = call(
        &mut engine,
        json!({"op": "chip.open", "path": format!("{DIR}/TopAssembly.hdl")}),
    );
    let session = &opened["session"];
    call(
        &mut engine,
        json!({"op": "chip.set", "session": session, "pin": "p", "value": 1}),
    );
    call(
        &mut engine,
        json!({"op": "chip.set", "session": session, "pin": "q", "value": 1, "eval": true}),
    );

    // Edit LeafPair via an unsaved overlay: swap Xor for Or on `sum` and add a comment line.
    let edited_leaf = "\
// edited in memory
CHIP LeafPair {
    IN x, y;
    OUT sum, carry;

    PARTS:
    Or(a=x, b=y, out=sum);
    And(a=x, b=y, out=carry);
}
";
    call(
        &mut engine,
        json!({"op": "overlay.set", "path": format!("{DIR}/LeafPair.hdl"), "text": edited_leaf}),
    );
    let reset = call(&mut engine, json!({"op": "chip.reset", "session": session}));
    assert_eq!(reset["revision"], 2);

    let leaf = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": session, "path": [0, 0]}),
    );
    assert_eq!(leaf["revision"], 2);
    assert_eq!(leaf["values"]["revision"], 2);
    assert_eq!(leaf["values"]["path"], json!([0, 0]));
    assert_eq!(leaf["parts"][0]["chip"], "Or");
    // Extra leading comment line shifts part 0 from line 5 to line 6.
    assert_eq!(leaf["parts"][0]["span"]["line"], 6);
    // Inputs (p=1, q=1) were preserved across reset, so Or(1, 1) -> sum=1.
    assert_eq!(leaf["values"]["nets"], json!([1, 1, 1, 1]));
    assert_eq!(
        leaf["values"]["parts"][0],
        json!({"inputs": [1, 1], "outputs": [1]})
    );
}

#[test]
fn revision_increments_only_on_successful_reset_and_values_echoes_path() {
    let acc_path = format!("{DIR}/Accumulator.hdl");
    let host = Arc::new(MemoryHost::new().with(&acc_path, fixture("chips/Accumulator.hdl")));
    let mut engine = Engine::new(host);

    let opened = call(&mut engine, json!({"op": "chip.open", "path": acc_path}));
    let session = &opened["session"];
    assert_eq!(opened["revision"], 1);

    // State, set, eval, tick, tock, step, diagram, and values leave revision at 1.
    assert_eq!(
        call(&mut engine, json!({"op": "chip.state", "session": session}))["revision"],
        1
    );
    assert_eq!(
        call(
            &mut engine,
            json!({"op": "chip.set", "session": session, "pin": "in", "value": 7})
        )["revision"],
        1
    );
    assert_eq!(
        call(
            &mut engine,
            json!({"op": "chip.set", "session": session, "pin": "add", "value": 1})
        )["revision"],
        1
    );
    assert_eq!(
        call(&mut engine, json!({"op": "chip.eval", "session": session}))["revision"],
        1
    );
    assert_eq!(
        call(&mut engine, json!({"op": "chip.tick", "session": session}))["revision"],
        1
    );
    assert_eq!(
        call(&mut engine, json!({"op": "chip.tock", "session": session}))["revision"],
        1
    );
    assert_eq!(
        call(&mut engine, json!({"op": "chip.step", "session": session}))["revision"],
        1
    );

    let diag = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": session, "path": []}),
    );
    assert_eq!(diag["revision"], 1);
    assert_eq!(diag["values"]["revision"], 1);
    assert_eq!(diag["values"]["path"], json!([]));

    // Omitted path normalises to `[]` and is echoed by `chip.values`.
    let vals = call(
        &mut engine,
        json!({"op": "chip.values", "session": session}),
    );
    assert_eq!(vals["revision"], 1);
    assert_eq!(vals["path"], json!([]));

    // A successful reset rebuilds the circuit and increments revision to 2.
    let reset = call(&mut engine, json!({"op": "chip.reset", "session": session}));
    assert_eq!(reset["revision"], 2);
    assert_eq!(
        call(&mut engine, json!({"op": "chip.state", "session": session}))["revision"],
        2
    );
    let diag = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": session, "path": []}),
    );
    assert_eq!(diag["revision"], 2);
    assert_eq!(diag["values"]["revision"], 2);
    let vals = call(
        &mut engine,
        json!({"op": "chip.values", "session": session, "path": []}),
    );
    assert_eq!(vals["revision"], 2);
    assert_eq!(vals["path"], json!([]));

    // A reset that fails to elaborate keeps the old circuit and must NOT bump revision.
    let broken_acc = "\
CHIP Accumulator {
    IN in[16], add, reset;
    OUT out[16];

    PARTS:
    UnknownGate(in=in, out=out);
}
";
    call(
        &mut engine,
        json!({"op": "overlay.set", "path": acc_path, "text": broken_acc}),
    );
    let failed = call(&mut engine, json!({"op": "chip.reset", "session": session}));
    assert!(failed["error"]["message"].is_string(), "{failed}");

    let state = call(&mut engine, json!({"op": "chip.state", "session": session}));
    assert_eq!(state["revision"], 2);
    let diag = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": session, "path": []}),
    );
    assert_eq!(diag["revision"], 2);
    assert_eq!(diag["values"]["revision"], 2);
    let vals = call(
        &mut engine,
        json!({"op": "chip.values", "session": session, "path": []}),
    );
    assert_eq!(vals["revision"], 2);
    assert_eq!(vals["path"], json!([]));
    // Old circuit is still live with its preserved inputs (in=7, add=1, reset=0).
    assert_eq!(vals["nets"][0], json!(7));
    assert_eq!(vals["nets"][1], json!(1));
    assert_eq!(vals["nets"][2], json!(0));

    // Reordering `IN` pins changes net indices; a successful reset bumps revision to 3.
    let reordered_acc = "\
CHIP Accumulator {
    IN reset, add, in[16];
    OUT out[16];

    PARTS:
    Add16(a=total, b=in, out=sum);
    Mux16(a=total, b=sum, sel=add, out=next);
    Mux16(a=next, b=false, sel=reset, out=stored);
    Register(in=stored, load=true, out=total, out=out);
}
";
    call(
        &mut engine,
        json!({"op": "overlay.set", "path": acc_path, "text": reordered_acc}),
    );
    let reset = call(&mut engine, json!({"op": "chip.reset", "session": session}));
    assert_eq!(reset["revision"], 3);

    let diag = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": session, "path": []}),
    );
    assert_eq!(diag["revision"], 3);
    assert_eq!(diag["values"]["revision"], 3);
    assert_eq!(diag["values"]["path"], json!([]));
    assert_eq!(diag["nets"][0]["name"], "reset");
    assert_eq!(diag["nets"][1]["name"], "add");
    assert_eq!(diag["nets"][2]["name"], "in");

    let vals = call(
        &mut engine,
        json!({"op": "chip.values", "session": session, "path": []}),
    );
    assert_eq!(vals["revision"], 3);
    assert_eq!(vals["path"], json!([]));
    // Net order now matches reordered IN pins: [reset=0, add=1, in=7, ...].
    assert_eq!(vals["nets"][0], json!(0));
    assert_eq!(vals["nets"][1], json!(1));
    assert_eq!(vals["nets"][2], json!(7));
}

fn fnv1a_64_hex(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in text.as_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn pin_bits(state: &Value, name: &str) -> Value {
    state["pins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == name)
        .unwrap_or_else(|| panic!("missing pin `{name}` in {state}"))["bits"]
        .clone()
}

#[test]
fn expect_revision_guards_all_chip_ops_and_leaves_state_untouched() {
    let acc_path = format!("{DIR}/Accumulator.hdl");
    let maj_path = format!("{DIR}/Majority.hdl");
    let key_path = format!("{DIR}/KeyLatch.hdl");
    let host = Arc::new(
        MemoryHost::new()
            .with(&acc_path, fixture("chips/Accumulator.hdl"))
            .with(&maj_path, fixture("chips/Majority.hdl"))
            .with(&key_path, fixture("chips/KeyLatch.hdl")),
    );
    let mut engine = Engine::new(host);

    // --- Clocked chip: state, set, eval, tick, tock, step, reset, diagram, values ---
    let opened = call(&mut engine, json!({"op": "chip.open", "path": acc_path}));
    let acc = &opened["session"];
    assert_eq!(opened["revision"], 1);

    // Matching expectRevision works for set, eval, tick, tock, step, state, diagram, values.
    let set_in = call(
        &mut engine,
        json!({"op": "chip.set", "session": acc, "pin": "in", "value": 5, "expectRevision": 1}),
    );
    assert_eq!(set_in["revision"], 1);
    assert_eq!(pin_bits(&set_in, "in"), 5);
    assert_eq!(
        call(
            &mut engine,
            json!({"op": "chip.set", "session": acc, "pin": "add", "value": 1, "expectRevision": 1})
        )["revision"],
        1
    );
    assert_eq!(
        call(
            &mut engine,
            json!({"op": "chip.eval", "session": acc, "expectRevision": 1})
        )["revision"],
        1
    );
    let ticked = call(
        &mut engine,
        json!({"op": "chip.tick", "session": acc, "expectRevision": 1}),
    );
    assert_eq!(ticked["time"], "0+");
    let tocked = call(
        &mut engine,
        json!({"op": "chip.tock", "session": acc, "expectRevision": 1}),
    );
    assert_eq!(tocked["time"], "1");
    assert_eq!(pin_bits(&tocked, "out"), 5);
    let stepped = call(
        &mut engine,
        json!({"op": "chip.step", "session": acc, "expectRevision": 1}),
    );
    assert_eq!(stepped["time"], "2");
    assert_eq!(pin_bits(&stepped, "out"), 10);
    assert_eq!(
        call(
            &mut engine,
            json!({"op": "chip.state", "session": acc, "expectRevision": 1})
        )["revision"],
        1
    );
    assert_eq!(
        call(
            &mut engine,
            json!({"op": "chip.diagram", "session": acc, "path": [], "expectRevision": 1})
        )["revision"],
        1
    );
    assert_eq!(
        call(
            &mut engine,
            json!({"op": "chip.values", "session": acc, "path": [], "expectRevision": 1})
        )["revision"],
        1
    );

    // Matching expectRevision on chip.reset succeeds and bumps revision to 2.
    let reset = call(
        &mut engine,
        json!({"op": "chip.reset", "session": acc, "expectRevision": 1}),
    );
    assert_eq!(reset["revision"], 2);
    assert_eq!(reset["time"], "0");
    assert_eq!(pin_bits(&reset, "in"), 5);

    // Stale expectRevision (1) and future expectRevision (99) reject every op and leave state untouched.
    for bad_rev in [1u64, 99u64] {
        for req in [
            json!({"op": "chip.set", "session": acc, "pin": "in", "value": 999, "eval": true, "expectRevision": bad_rev}),
            json!({"op": "chip.eval", "session": acc, "expectRevision": bad_rev}),
            json!({"op": "chip.tick", "session": acc, "expectRevision": bad_rev}),
            json!({"op": "chip.tock", "session": acc, "expectRevision": bad_rev}),
            json!({"op": "chip.step", "session": acc, "expectRevision": bad_rev}),
            json!({"op": "chip.reset", "session": acc, "expectRevision": bad_rev}),
            json!({"op": "chip.state", "session": acc, "expectRevision": bad_rev}),
            json!({"op": "chip.diagram", "session": acc, "path": [], "expectRevision": bad_rev}),
            json!({"op": "chip.values", "session": acc, "path": [], "expectRevision": bad_rev}),
        ] {
            let err = call(&mut engine, req.clone());
            assert_eq!(err["error"]["code"], "stale-revision", "{req} -> {err}");
            assert_eq!(err["error"]["revision"], 2, "{req} -> {err}");
            assert!(err["error"]["message"].is_string(), "{req} -> {err}");
        }
    }

    // Verify none of the rejected ops mutated inputs, clock time, or revision.
    let state = call(&mut engine, json!({"op": "chip.state", "session": acc}));
    assert_eq!(state["revision"], 2);
    assert_eq!(state["time"], "0");
    assert_eq!(pin_bits(&state, "in"), 5);
    assert_eq!(pin_bits(&state, "out"), 0);

    // Explicit null expectRevision is treated as absent.
    let state_null = call(
        &mut engine,
        json!({"op": "chip.state", "session": acc, "expectRevision": null}),
    );
    assert_eq!(state_null["revision"], 2);

    // --- Combinational chip: chip.eval state untouched & chip.table guard ---
    let maj_open = call(&mut engine, json!({"op": "chip.open", "path": maj_path}));
    let maj = &maj_open["session"];
    // Set a=1, b=1 without eval so out is still 0.
    call(
        &mut engine,
        json!({"op": "chip.set", "session": maj, "pin": "a", "value": 1}),
    );
    call(
        &mut engine,
        json!({"op": "chip.set", "session": maj, "pin": "b", "value": 1}),
    );
    // Stale eval must NOT evaluate the circuit (out stays 0).
    let stale_eval = call(
        &mut engine,
        json!({"op": "chip.eval", "session": maj, "expectRevision": 2}),
    );
    assert_eq!(stale_eval["error"]["code"], "stale-revision");
    assert_eq!(stale_eval["error"]["revision"], 1);
    let before_eval = call(&mut engine, json!({"op": "chip.state", "session": maj}));
    assert_eq!(pin_bits(&before_eval, "out"), 0);

    // Matching eval computes out = 1.
    let after_eval = call(
        &mut engine,
        json!({"op": "chip.eval", "session": maj, "expectRevision": 1}),
    );
    assert_eq!(pin_bits(&after_eval, "out"), 1);

    // chip.table guarded by expectRevision.
    let stale_table = call(
        &mut engine,
        json!({"op": "chip.table", "session": maj, "expectRevision": 2}),
    );
    assert_eq!(stale_table["error"]["code"], "stale-revision");
    assert_eq!(stale_table["error"]["revision"], 1);
    let ok_table = call(
        &mut engine,
        json!({"op": "chip.table", "session": maj, "expectRevision": 1}),
    );
    assert_eq!(ok_table["rows"].as_array().unwrap().len(), 8);

    // --- Keyboard chip: key op guarded by expectRevision ---
    let key_open = call(&mut engine, json!({"op": "chip.open", "path": key_path}));
    let key_sess = &key_open["session"];
    let stale_key = call(
        &mut engine,
        json!({"op": "key", "session": key_sess, "code": 75, "expectRevision": 2}),
    );
    assert_eq!(stale_key["error"]["code"], "stale-revision");
    assert_eq!(stale_key["error"]["revision"], 1);
    // Stepping after a rejected key must still observe key = 0.
    let key_state = call(&mut engine, json!({"op": "chip.step", "session": key_sess}));
    assert_eq!(pin_bits(&key_state, "key"), 0);

    // Matching expectRevision accepts the key and latches it on the next step.
    let ok_key = call(
        &mut engine,
        json!({"op": "key", "session": key_sess, "code": 75, "expectRevision": 1}),
    );
    assert_eq!(ok_key["accepted"], true);
    let key_state = call(&mut engine, json!({"op": "chip.step", "session": key_sess}));
    assert_eq!(pin_bits(&key_state, "key"), 75);
}

#[test]
fn typed_error_codes_and_revisions_cover_every_error_category() {
    let wrap_child_composite = "\
CHIP WrapChild {
    IN a, b;
    OUT out;

    PARTS:
    Nand(a=a, b=b, out=out);
}
";
    let wrap_child_builtin = "\
CHIP WrapChild {
    IN a, b;
    OUT out;

    BUILTIN Nand;
}
";
    let wrap_parent = "\
CHIP WrapParent {
    IN a, b;
    OUT out;

    PARTS:
    WrapChild(a=a, b=b, out=out);
}
";
    let broken_hdl = "\
CHIP BrokenChip {
    IN a;
    OUT out;

    PARTS:
    DoesNotExist(in=a, out=out);
}
";
    let host = Arc::new(
        MemoryHost::new()
            .with(&format!("{DIR}/WrapChild.hdl"), wrap_child_composite)
            .with(&format!("{DIR}/WrapParent.hdl"), wrap_parent)
            .with(&format!("{DIR}/BrokenChip.hdl"), broken_hdl)
            .with(
                &format!("{DIR}/Majority.hdl"),
                fixture("chips/Majority.hdl"),
            )
            .with(
                &format!("{DIR}/Accumulator.hdl"),
                fixture("chips/Accumulator.hdl"),
            ),
    );
    let mut engine = Engine::new(host);

    // 1. build-failed on chip.open (no session -> no revision).
    let open_broken = call(
        &mut engine,
        json!({"op": "chip.open", "path": format!("{DIR}/BrokenChip.hdl")}),
    );
    assert_eq!(
        open_broken["error"]["code"], "build-failed",
        "{open_broken}"
    );
    assert!(
        open_broken["error"].get("revision").is_none(),
        "{open_broken}"
    );
    assert!(
        open_broken["error"]["message"]
            .as_str()
            .unwrap()
            .contains("cannot load `BrokenChip.hdl`"),
        "{open_broken}"
    );

    let open_missing = call(
        &mut engine,
        json!({"op": "chip.open", "path": format!("{DIR}/NoSuchChip.hdl")}),
    );
    assert_eq!(
        open_missing["error"]["code"], "build-failed",
        "{open_missing}"
    );
    assert!(
        open_missing["error"].get("revision").is_none(),
        "{open_missing}"
    );

    // chip.open missing `path` -> other (no revision).
    let open_no_path = call(&mut engine, json!({"op": "chip.open"}));
    assert_eq!(open_no_path["error"]["code"], "other", "{open_no_path}");
    assert!(
        open_no_path["error"].get("revision").is_none(),
        "{open_no_path}"
    );

    // 2. invalid-path including right after a rebuild turning the drilled part into a built-in.
    let opened = call(
        &mut engine,
        json!({"op": "chip.open", "path": format!("{DIR}/WrapParent.hdl")}),
    );
    let parent_sess = &opened["session"];
    assert_eq!(opened["revision"], 1);

    // Before rebuild: drilling into [0] (WrapChild) succeeds.
    let child_diag = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": parent_sess, "path": [0]}),
    );
    assert_eq!(child_diag["chip"], "WrapChild");
    assert_eq!(child_diag["revision"], 1);

    // Out-of-range and malformed paths carry invalid-path + current revision (1).
    for bad_path in [json!([99]), json!([-1]), json!("not-an-array")] {
        let err = call(
            &mut engine,
            json!({"op": "chip.diagram", "session": parent_sess, "path": bad_path}),
        );
        assert_eq!(err["error"]["code"], "invalid-path", "{err}");
        assert_eq!(err["error"]["revision"], 1, "{err}");
    }

    // Turn WrapChild into a built-in and rebuild -> revision 2.
    call(
        &mut engine,
        json!({
            "op": "overlay.set",
            "path": format!("{DIR}/WrapChild.hdl"),
            "text": wrap_child_builtin,
        }),
    );
    let rebuilt = call(
        &mut engine,
        json!({"op": "chip.reset", "session": parent_sess}),
    );
    assert_eq!(rebuilt["revision"], 2);

    // Right after rebuild, [0] is now a built-in -> invalid-path carrying revision 2.
    for op in ["chip.diagram", "chip.values"] {
        let err = call(
            &mut engine,
            json!({"op": op, "session": parent_sess, "path": [0]}),
        );
        assert_eq!(err["error"]["code"], "invalid-path", "{op}: {err}");
        assert_eq!(err["error"]["revision"], 2, "{op}: {err}");
        assert!(
            err["error"]["message"]
                .as_str()
                .unwrap()
                .contains("built-in"),
            "{op}: {err}"
        );
    }

    // Same path with stale expectRevision: 1 gets stale-revision before invalid-path.
    let stale_after_rebuild = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": parent_sess, "path": [0], "expectRevision": 1}),
    );
    assert_eq!(stale_after_rebuild["error"]["code"], "stale-revision");
    assert_eq!(stale_after_rebuild["error"]["revision"], 2);

    // 3. build-failed on chip.reset keeps old circuit and returns unchanged revision (2).
    call(
        &mut engine,
        json!({
            "op": "overlay.set",
            "path": format!("{DIR}/WrapParent.hdl"),
            "text": "CHIP WrapParent { IN a, b; OUT out; PARTS: MissingGate(a=a, out=out); }\n",
        }),
    );
    let failed_reset = call(
        &mut engine,
        json!({"op": "chip.reset", "session": parent_sess}),
    );
    assert_eq!(
        failed_reset["error"]["code"], "build-failed",
        "{failed_reset}"
    );
    assert_eq!(failed_reset["error"]["revision"], 2, "{failed_reset}");
    assert!(
        failed_reset["error"]["message"]
            .as_str()
            .unwrap()
            .contains("cannot load `WrapParent.hdl`"),
        "{failed_reset}"
    );
    assert_eq!(
        call(
            &mut engine,
            json!({"op": "chip.state", "session": parent_sess})
        )["revision"],
        2
    );

    // 4. invalid-input on chip.set (bad pin name, output pin, out-of-range, non-number, missing pin).
    let maj = call(
        &mut engine,
        json!({"op": "chip.open", "path": format!("{DIR}/Majority.hdl")}),
    )["session"]
        .clone();
    for req in [
        json!({"op": "chip.set", "session": maj, "pin": "nope", "value": 1}),
        json!({"op": "chip.set", "session": maj, "pin": "out", "value": 1}),
        json!({"op": "chip.set", "session": maj, "pin": "a", "value": 2}),
        json!({"op": "chip.set", "session": maj, "pin": "a", "value": "abc"}),
        json!({"op": "chip.set", "session": maj, "pin": "a", "value": 1.5}),
        json!({"op": "chip.set", "session": maj, "value": 1}),
    ] {
        let err = call(&mut engine, req.clone());
        assert_eq!(err["error"]["code"], "invalid-input", "{req} -> {err}");
        assert_eq!(err["error"]["revision"], 1, "{req} -> {err}");
    }

    // 5. other errors on a live chip session carry code "other" + current revision.
    let acc = call(
        &mut engine,
        json!({"op": "chip.open", "path": format!("{DIR}/Accumulator.hdl")}),
    )["session"]
        .clone();
    for req in [
        json!({"op": "chip.table", "session": acc}),
        json!({"op": "chip.tock", "session": acc}),
        json!({"op": "key", "session": maj, "code": 70000}),
        json!({"op": "chip.unknown", "session": maj}),
        json!({"op": "chip.state", "session": maj, "expectRevision": "1"}),
        json!({"op": "chip.state", "session": maj, "expectRevision": -1}),
    ] {
        let err = call(&mut engine, req.clone());
        assert_eq!(err["error"]["code"], "other", "{req} -> {err}");
        assert_eq!(err["error"]["revision"], 1, "{req} -> {err}");
    }

    // 6. session-ended (unknown session id AND closed session; no revision).
    let unknown_sess = call(&mut engine, json!({"op": "chip.state", "session": 999_999}));
    assert_eq!(
        unknown_sess["error"]["code"], "session-ended",
        "{unknown_sess}"
    );
    assert!(
        unknown_sess["error"].get("revision").is_none(),
        "{unknown_sess}"
    );
    assert_eq!(
        unknown_sess["error"]["message"],
        "this session has ended; start it again"
    );

    call(&mut engine, json!({"op": "close", "session": maj}));
    let closed_sess = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": maj, "path": []}),
    );
    assert_eq!(
        closed_sess["error"]["code"], "session-ended",
        "{closed_sess}"
    );
    assert!(
        closed_sess["error"].get("revision").is_none(),
        "{closed_sess}"
    );
    assert_eq!(
        closed_sess["error"]["message"],
        "this session has ended; start it again"
    );
}

#[test]
fn source_hash_reflects_build_time_text_until_reset_rebuilds() {
    let leaf_path = format!("{DIR}/LeafPair.hdl");
    let mid_path = format!("{DIR}/MidStage.hdl");
    let top_path = format!("{DIR}/TopAssembly.hdl");
    let host = Arc::new(
        MemoryHost::new()
            .with(&leaf_path, LEAF_PAIR_HDL)
            .with(&mid_path, MID_STAGE_HDL)
            .with(&top_path, TOP_ASSEMBLY_HDL),
    );
    let mut engine = Engine::new(host.clone());

    let opened = call(&mut engine, json!({"op": "chip.open", "path": top_path}));
    let session = &opened["session"];

    let expected_top_v1 = fnv1a_64_hex(TOP_ASSEMBLY_HDL);
    let expected_mid_v1 = fnv1a_64_hex(MID_STAGE_HDL);
    let expected_leaf_v1 = fnv1a_64_hex(LEAF_PAIR_HDL);

    let root_v1 = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": session, "path": []}),
    );
    let mid_v1 = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": session, "path": [0]}),
    );
    let leaf_v1 = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": session, "path": [0, 0]}),
    );
    assert_eq!(root_v1["sourceHash"], expected_top_v1);
    assert_eq!(mid_v1["sourceHash"], expected_mid_v1);
    assert_eq!(leaf_v1["sourceHash"], expected_leaf_v1);

    // Edit LeafPair via overlay AND overwrite TopAssembly on the underlying host
    // WITHOUT calling chip.reset: diagram must NOT re-read, so sourceHash stays v1.
    let edited_leaf = format!("// overlay edit\n{LEAF_PAIR_HDL}");
    let edited_top = format!("// host edit\n{TOP_ASSEMBLY_HDL}");
    call(
        &mut engine,
        json!({"op": "overlay.set", "path": leaf_path, "text": edited_leaf}),
    );
    host.insert(&top_path, &edited_top);

    let root_before_reset = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": session, "path": []}),
    );
    let leaf_before_reset = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": session, "path": [0, 0]}),
    );
    assert_eq!(root_before_reset["revision"], 1);
    assert_eq!(root_before_reset["sourceHash"], expected_top_v1);
    assert_eq!(leaf_before_reset["revision"], 1);
    assert_eq!(leaf_before_reset["sourceHash"], expected_leaf_v1);

    // Rebuild via chip.reset: sourceHash updates for edited files, stays same for unedited MidStage.
    let reset = call(&mut engine, json!({"op": "chip.reset", "session": session}));
    assert_eq!(reset["revision"], 2);

    let root_v2 = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": session, "path": []}),
    );
    let mid_v2 = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": session, "path": [0]}),
    );
    let leaf_v2 = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": session, "path": [0, 0]}),
    );
    assert_eq!(root_v2["sourceHash"], fnv1a_64_hex(&edited_top));
    assert_ne!(root_v2["sourceHash"], expected_top_v1);
    assert_eq!(mid_v2["sourceHash"], expected_mid_v1);
    assert_eq!(leaf_v2["sourceHash"], fnv1a_64_hex(&edited_leaf));
    assert_ne!(leaf_v2["sourceHash"], expected_leaf_v1);

    // A failed reset after breaking LeafPair keeps revision 2 AND the revision-2 sourceHash.
    call(
        &mut engine,
        json!({
            "op": "overlay.set",
            "path": leaf_path,
            "text": "CHIP LeafPair { IN x, y; OUT sum, carry; PARTS: Broken(x=x, out=sum); }\n",
        }),
    );
    let failed = call(&mut engine, json!({"op": "chip.reset", "session": session}));
    assert_eq!(failed["error"]["code"], "build-failed");
    assert_eq!(failed["error"]["revision"], 2);

    let leaf_after_failed_reset = call(
        &mut engine,
        json!({"op": "chip.diagram", "session": session, "path": [0, 0]}),
    );
    assert_eq!(leaf_after_failed_reset["revision"], 2);
    assert_eq!(
        leaf_after_failed_reset["sourceHash"],
        fnv1a_64_hex(&edited_leaf)
    );
}
