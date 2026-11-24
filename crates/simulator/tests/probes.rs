//! Unit and integration tests for opt-in per-instance circuit probes.

use std::sync::Arc;

use n2t_core::builtins::BuiltinLibrary;
use n2t_core::hdl::Elaborator;
use n2t_core::hdl::resolve::MemorySourceProvider;
use n2t_core::host::MemoryHost;
use n2t_core::source::Span;
use n2t_simulator::{
    BuiltinRegistry, Circuit, HardwareMachine, Outcome, RunnerOptions, ScriptRunner,
};

const LEAF_PAIR_HDL: &str = r#"
CHIP LeafPair {
    IN x, y;
    OUT sum, carry;

    PARTS:
    Xor(a=x, b=y, out=sum);
    And(a=x, b=y, out=carry);
}
"#;

const MID_STAGE_HDL: &str = r#"
CHIP MidStage {
    IN a, b, c;
    OUT out, extra;

    PARTS:
    // Part 0: composite child with one unconnected output (`carry`).
    LeafPair(x=a, y=b, sum=abSum);
    // Part 1: built-in HalfAdder with one constant-fed input (`true`) and unconnected `carry`.
    HalfAdder(a=abSum, b=true, sum=toggled);
    // Part 2: composite child fed by a constant `false` on `y`.
    LeafPair(x=c, y=false, sum=cCopy, carry=alwaysZero);
    // Part 3 & 4: combine into outputs.
    Xor(a=toggled, b=cCopy, out=out);
    Or(a=alwaysZero, b=abSum, out=extra);
}
"#;

const TOP_ASSEMBLY_HDL: &str = r#"
CHIP TopAssembly {
    IN p, q, r;
    OUT y;

    PARTS:
    // Part 0: composite MidStage with `extra` left unconnected at root.
    MidStage(a=p, b=q, c=r, out=midOut);
    Not(in=midOut, out=y);
}
"#;

const STAGE_LATCH_HDL: &str = r#"
CHIP StageLatch {
    IN d[16], en;
    OUT q[16];

    PARTS:
    Register(in=d, load=en, out=q);
}
"#;

const TOP_COUNTER_HDL: &str = r#"
CHIP TopCounter {
    IN step[16], enable;
    OUT value[16];

    PARTS:
    Add16(a=current, b=step, out=nextVal);
    StageLatch(d=nextVal, en=enable, q=current);
    Or16(a=current, b=false, out=value);
}
"#;

const EMPTY_STUB_HDL: &str = r#"
CHIP EmptyStub {
    IN a, b[4];
    OUT out[4];

    PARTS:
}
"#;

fn compile(provider: MemorySourceProvider, name: &str) -> Arc<n2t_core::hdl::ChipDefinition> {
    let mut elab = Elaborator::new(provider, Arc::new(BuiltinLibrary::standard()));
    elab.definition(name)
        .unwrap_or_else(|u| panic!("failed to compile {name}: {:?}", u.diagnostics))
}

#[test]
fn default_circuit_and_script_runner_do_not_allocate_probes() {
    let provider = MemorySourceProvider::new()
        .with("LeafPair", LEAF_PAIR_HDL)
        .with("MidStage", MID_STAGE_HDL)
        .with("TopAssembly", TOP_ASSEMBLY_HDL);
    let def = compile(provider.clone(), "TopAssembly");

    let plain = Circuit::new(def.clone(), &BuiltinRegistry::standard()).unwrap();
    assert!(!plain.has_probes());
    assert!(plain.instance(&[]).is_none());

    let probed = Circuit::with_probes(def, &BuiltinRegistry::standard()).unwrap();
    assert!(probed.has_probes());
    assert!(probed.instance(&[]).is_some());

    // HardwareMachine::new does not enable probes.
    let mut plain_machine = HardwareMachine::new(
        Box::new(provider.clone()),
        Arc::new(BuiltinLibrary::standard()),
    );
    plain_machine
        .load_chip("TopAssembly", "TopAssembly.hdl", Span::default())
        .unwrap();
    assert!(!plain_machine.circuit().unwrap().has_probes());

    // HardwareMachine::with_probes enables probes.
    let mut probed_machine =
        HardwareMachine::with_probes(Box::new(provider), Arc::new(BuiltinLibrary::standard()));
    probed_machine
        .load_chip("TopAssembly", "TopAssembly.hdl", Span::default())
        .unwrap();
    assert!(probed_machine.circuit().unwrap().has_probes());

    // ScriptRunner (used by test runs) must never allocate probes.
    let host = Arc::new(
        MemoryHost::new()
            .with("/p/LeafPair.hdl", LEAF_PAIR_HDL)
            .with(
                "/p/LeafPair.tst",
                "load LeafPair.hdl;\nset x 1, set y 1, eval;\n",
            ),
    );
    let mut runner = ScriptRunner::open(host, "/p/LeafPair.tst", RunnerOptions::default()).unwrap();
    assert_eq!(runner.run(), Outcome::CompletedUnchecked);
    let circuit = runner
        .hardware()
        .expect("hardware dialect")
        .circuit()
        .expect("loaded circuit");
    assert!(
        !circuit.has_probes(),
        "test runs must not allocate circuit probes"
    );
}

#[test]
fn probes_read_nested_nets_constant_inputs_and_unconnected_outputs() {
    let provider = MemorySourceProvider::new()
        .with("LeafPair", LEAF_PAIR_HDL)
        .with("MidStage", MID_STAGE_HDL)
        .with("TopAssembly", TOP_ASSEMBLY_HDL);
    let def = compile(provider, "TopAssembly");
    let mut circuit = Circuit::with_probes(def, &BuiltinRegistry::standard()).unwrap();

    // Set p=1, q=1, r=0:
    // - MidStage part 0 (LeafPair): x=1, y=1 -> sum=0 (abSum=0), carry=1 (unconnected!)
    // - MidStage part 1 (HalfAdder): a=0, b=true(1) -> sum=1 (toggled=1), carry=0 (unconnected)
    // - MidStage part 2 (LeafPair): x=0, y=false(0) -> sum=0 (cCopy=0), carry=0 (alwaysZero=0)
    // - MidStage part 3 (Xor): a=1, b=0 -> out=1 (midOut=1)
    // - MidStage part 4 (Or): a=0, b=0 -> extra=0 (unconnected in TopAssembly)
    // - TopAssembly part 1 (Not): in=1 -> y=0
    circuit.set("p", 1);
    circuit.set("q", 1);
    circuit.set("r", 0);
    circuit.eval();

    let root = circuit.instance(&[]).unwrap();
    assert_eq!(root.definition().name, "TopAssembly");
    assert!(root.is_composite());
    assert_eq!(root.input_values(), vec![1, 1, 0]);
    assert_eq!(root.output_values(), vec![0]);
    // Root nets: p=1, q=1, r=0, y=0, midOut=1.
    assert_eq!(root.net_values(), Some(vec![1, 1, 0, 0, 1]));

    // Drill 1 level down: MidStage at [0].
    let mid = circuit.instance(&[0]).unwrap();
    assert_eq!(mid.definition().name, "MidStage");
    assert_eq!(mid.input_values(), vec![1, 1, 0]);
    assert_eq!(mid.output_values(), vec![1, 0]);
    // MidStage part 0 is composite LeafPair: unconnected `carry` must still read 1!
    let leaf0 = mid.part(0).unwrap();
    assert_eq!(leaf0.definition().name, "LeafPair");
    assert_eq!(leaf0.input_values(), vec![1, 1]);
    assert_eq!(leaf0.output_values(), vec![0, 1]);

    // Drill 2 levels down: LeafPair at [0, 0], and inspect its built-in parts.
    let deep_leaf = circuit.instance(&[0, 0]).unwrap();
    assert_eq!(deep_leaf.definition().name, "LeafPair");
    assert_eq!(deep_leaf.net_values(), Some(vec![1, 1, 0, 1]));
    let deep_and = circuit.instance(&[0, 0, 1]).unwrap();
    assert_eq!(deep_and.definition().name, "And");
    assert!(!deep_and.is_composite());
    assert_eq!(deep_and.net_values(), None);
    assert_eq!(deep_and.input_values(), vec![1, 1]);
    assert_eq!(deep_and.output_values(), vec![1]);

    // MidStage part 1 is built-in HalfAdder with constant `b=true` and unconnected `carry`.
    let ha = mid.part(1).unwrap();
    assert_eq!(ha.definition().name, "HalfAdder");
    assert_eq!(ha.input_values(), vec![0, 1]);
    assert_eq!(ha.output_values(), vec![1, 0]);

    // Now flip q=0 so abSum=1 -> HalfAdder(1, true) produces sum=0, carry=1 (unconnected!)
    // and MidStage's `extra` (unconnected in TopAssembly!) becomes 1!
    circuit.set("q", 0);
    circuit.eval();

    let mid = circuit.instance(&[0]).unwrap();
    assert_eq!(mid.output_values(), vec![0, 1]);
    assert_eq!(mid.output_value(1), Some(1));
    let ha = mid.part(1).unwrap();
    assert_eq!(ha.input_values(), vec![1, 1]);
    assert_eq!(ha.output_values(), vec![0, 1]);
    let leaf2 = mid.part(2).unwrap();
    assert_eq!(leaf2.input_values(), vec![0, 0]);
    assert_eq!(leaf2.input_value(1), Some(0));

    // Invalid paths return None.
    assert!(circuit.instance(&[2]).is_none());
    assert!(circuit.instance(&[1, 0]).is_none()); // Not is built-in
    assert!(circuit.instance(&[0, 99]).is_none());
    assert!(circuit.instance(&[0, 0, 1, 0]).is_none()); // And is built-in
}

#[test]
fn probes_track_sequential_state_across_tick_and_tock() {
    let provider = MemorySourceProvider::new()
        .with("StageLatch", STAGE_LATCH_HDL)
        .with("TopCounter", TOP_COUNTER_HDL);
    let def = compile(provider, "TopCounter");
    let mut circuit = Circuit::with_probes(def, &BuiltinRegistry::standard()).unwrap();

    circuit.set("step", 7);
    circuit.set("enable", 1);
    circuit.eval();

    // Before tick: nextVal=7 at StageLatch input, q=0 at StageLatch output.
    let latch = circuit.instance(&[1]).unwrap();
    assert_eq!(latch.input_values(), vec![7, 1]);
    assert_eq!(latch.output_values(), vec![0]);

    circuit.tick().unwrap();
    // After tick (before tock): Register output has not updated yet.
    let reg = circuit.instance(&[1, 0]).unwrap();
    assert_eq!(reg.definition().name, "Register");
    assert_eq!(reg.input_values(), vec![7, 1]);
    assert_eq!(reg.output_values(), vec![0]);

    circuit.tock().unwrap();
    // After tock: Register publishes 7, Add16 computes nextVal = 14.
    let latch = circuit.instance(&[1]).unwrap();
    assert_eq!(latch.input_values(), vec![14, 1]);
    assert_eq!(latch.output_values(), vec![7]);
    let reg = circuit.instance(&[1, 0]).unwrap();
    assert_eq!(reg.input_values(), vec![14, 1]);
    assert_eq!(reg.output_values(), vec![7]);
}

#[test]
fn probes_handle_empty_composite_without_panicking() {
    let provider = MemorySourceProvider::new().with("EmptyStub", EMPTY_STUB_HDL);
    let def = compile(provider, "EmptyStub");
    let mut circuit = Circuit::with_probes(def, &BuiltinRegistry::standard()).unwrap();
    circuit.set("a", 1);
    circuit.set("b", 0b1010);
    circuit.eval();

    let root = circuit.instance(&[]).unwrap();
    assert!(root.is_composite());
    assert_eq!(root.net_values(), Some(vec![1, 0b1010, 0]));
    assert_eq!(root.parts().count(), 0);
    assert!(root.part(0).is_none());
}
