//! Stopping a script at a chosen output line, and runs that keep the output
//! file in memory: what inspecting one row of a test in an emulator needs.

use std::sync::Arc;

use n2t_core::host::{MemoryHost, SharedHost};
use n2t_simulator::runner::standard_machine;
use n2t_simulator::{CpuMachine, Outcome, RunnerOptions, ScriptRunner, SuspendReason};

const DIR: &str = "/p";

/// RAM[2] = RAM[0] + RAM[1], then loops forever.
const ADD_ASM: &str = "@R0\nD=M\n@R1\nD=D+M\n@R2\nM=D\n(END)\n@END\n0;JMP\n";

/// Three rows; the second stops halfway through the program.
fn script(compare: bool) -> String {
    format!(
        "load Add.asm,\noutput-file Add.out,\n{}output-list RAM[0]%D2.6.2 RAM[1]%D2.6.2 RAM[2]%D2.6.2;\n\
         set RAM[0] 1, set RAM[1] 2, set RAM[2] 0;\nrepeat 10 {{ ticktock; }}\noutput;\n\
         set PC 0, set RAM[0] 5, set RAM[1] 6;\nrepeat 3 {{ ticktock; }}\noutput;\n\
         set PC 0, set RAM[0] 7, set RAM[1] 8;\nrepeat 10 {{ ticktock; }}\noutput;\n",
        if compare { "compare-to Add.cmp,\n" } else { "" }
    )
}

fn host() -> Arc<MemoryHost> {
    let host = Arc::new(MemoryHost::new());
    host.insert(&format!("{DIR}/Add.asm"), ADD_ASM);
    host
}

fn runner(host: &Arc<MemoryHost>, compare: bool, options: RunnerOptions) -> ScriptRunner {
    let shared: SharedHost = host.clone();
    let mut runner =
        ScriptRunner::new(shared, &script(compare), DIR, options, &standard_machine).unwrap();
    runner.set_script_path(&format!("{DIR}/Add.tst"));
    runner
}

/// The output lines the script really produces.
fn actual_lines() -> Vec<String> {
    let host = host();
    let mut runner = runner(&host, false, RunnerOptions::default());
    assert_eq!(runner.run(), Outcome::CompletedUnchecked);
    let out = host.get_text(&format!("{DIR}/Add.out")).unwrap();
    out.lines().map(str::to_string).collect()
}

fn cpu(runner: &ScriptRunner) -> &CpuMachine {
    runner.machine().as_any().downcast_ref().unwrap()
}

fn options(target: usize) -> RunnerOptions {
    RunnerOptions {
        collect_all_mismatches: true,
        stop_at_output_line: Some(target),
        skip_output_write: true,
        ..Default::default()
    }
}

#[test]
fn defaults_keep_writing_the_output_file_and_never_stop_at_a_line() {
    let options = RunnerOptions::default();
    assert!(!options.skip_output_write);
    assert_eq!(options.stop_at_output_line, None);
    assert_eq!(actual_lines().len(), 4);
}

#[test]
fn stops_right_after_the_output_that_produced_the_target_line() {
    let lines = actual_lines();
    let host = host();
    host.insert(&format!("{DIR}/Add.cmp"), lines.join("\n") + "\n");
    let mut runner = runner(&host, true, options(3));
    let outcome = runner.run();
    let Outcome::Suspended { reason, span } = outcome else {
        panic!("expected a suspension, got {outcome:?}");
    };
    assert_eq!(reason, SuspendReason::TargetLine);
    assert_eq!(runner.target_span(), Some(span));
    assert_eq!(runner.current_span(), span);
    // The machine is as the second row saw it: three instructions in.
    let cpu = cpu(&runner).cpu();
    assert_eq!(cpu.pc(), 3);
    assert_eq!(cpu.d(), 5);
    assert_eq!(cpu.a(), 1);
    assert_eq!(runner.output_text().unwrap().lines().count(), 3);
    // Resuming finishes the run normally; nothing was written to disk.
    assert!(matches!(runner.run(), Outcome::Passed { .. }));
    assert!(host.get(&format!("{DIR}/Add.out")).is_none());
}

#[test]
fn the_header_is_line_one() {
    let host = host();
    let mut runner = runner(&host, false, options(1));
    let outcome = runner.run();
    assert!(
        matches!(
            outcome,
            Outcome::Suspended {
                reason: SuspendReason::TargetLine,
                ..
            }
        ),
        "{outcome:?}"
    );
    assert_eq!(runner.output_text().unwrap().lines().count(), 1);
}

#[test]
fn without_a_compare_file_lines_are_counted() {
    let host = host();
    let mut runner = runner(&host, false, options(4));
    let outcome = runner.run();
    assert!(
        matches!(
            outcome,
            Outcome::Suspended {
                reason: SuspendReason::TargetLine,
                ..
            }
        ),
        "{outcome:?}"
    );
    let cpu = cpu(&runner).cpu();
    assert_eq!(cpu.ram(2), 15);
    // A line past the end is never reached: the run just ends.
    let host = self::host();
    let mut runner = self::runner(&host, false, options(9));
    assert_eq!(runner.run(), Outcome::CompletedUnchecked);
}

#[test]
fn stops_at_the_target_even_after_an_earlier_line_failed() {
    let mut lines = actual_lines();
    lines[1] = lines[1].replace('3', "4");
    lines[2] = lines[2].replace('5', "9");
    let host = host();
    host.insert(&format!("{DIR}/Add.cmp"), lines.join("\n") + "\n");

    let mut runner = runner(&host, true, options(3));
    let outcome = runner.run();
    let Outcome::Suspended { reason, span } = outcome else {
        panic!("expected a suspension, got {outcome:?}");
    };
    assert_eq!(reason, SuspendReason::TargetLine);
    // Both rows failed, and the machine is at the second.
    let failed: Vec<usize> = runner.failures().iter().map(|f| f.mismatch.line).collect();
    assert_eq!(failed, [2, 3]);
    assert_eq!(runner.failures()[1].span, span);
    assert_eq!(cpu(&runner).cpu().pc(), 3);
    // Carrying on still reports the first failure, as the official tools do.
    let Outcome::FailedComparison { mismatch, .. } = runner.run() else {
        panic!("the run should fail");
    };
    assert_eq!(mismatch.line, 2);
    assert!(host.get(&format!("{DIR}/Add.out")).is_none());
}

#[test]
fn blank_compare_lines_are_numbered_as_the_comparator_numbers_them() {
    // A blank line after the first row: the second row is compared against
    // it (and fails), and the third row against line 4 of the compare file.
    let lines = actual_lines();
    let cmp = format!("{}\n{}\n\n{}\n{}\n", lines[0], lines[1], lines[2], lines[3]);
    let host = host();
    host.insert(&format!("{DIR}/Add.cmp"), cmp);

    let mut runner = runner(&host, true, options(4));
    let outcome = runner.run();
    assert!(
        matches!(
            outcome,
            Outcome::Suspended {
                reason: SuspendReason::TargetLine,
                ..
            }
        ),
        "{outcome:?}"
    );
    let failed: Vec<usize> = runner.failures().iter().map(|f| f.mismatch.line).collect();
    assert_eq!(failed, [3, 4]);
    // The third row: the program ran to the end.
    assert_eq!(cpu(&runner).cpu().ram(2), 15);
    // The output file stops at the first failure, as the official tools do.
    assert_eq!(runner.output_text().unwrap().lines().count(), 3);

    // Line 5 of the compare file is never met: the run just ends.
    let mut runner = self::runner(&host, true, options(5));
    assert!(matches!(runner.run(), Outcome::FailedComparison { .. }));
}

#[test]
fn skipping_the_output_write_leaves_the_disk_alone_even_on_failure() {
    let mut lines = actual_lines();
    lines[1] = lines[1].replace('3', "4");
    let host = host();
    host.insert(&format!("{DIR}/Add.cmp"), lines.join("\n") + "\n");
    host.insert(&format!("{DIR}/Add.out"), "from an earlier run\n");
    let mut runner = runner(
        &host,
        true,
        RunnerOptions {
            skip_output_write: true,
            ..Default::default()
        },
    );
    let outcome = runner.run();
    assert!(matches!(outcome, Outcome::FailedComparison { .. }));
    assert_eq!(
        host.get_text(&format!("{DIR}/Add.out")).unwrap(),
        "from an earlier run\n"
    );
    assert_eq!(runner.output_text().unwrap().lines().count(), 2);
}

#[test]
fn into_machine_hands_over_the_state_reached() {
    let host = host();
    let mut runner = runner(&host, false, options(3));
    runner.run();
    let mut machine = runner.into_machine();
    let cpu: &mut CpuMachine = machine.as_any_mut().downcast_mut().unwrap();
    assert_eq!(cpu.cpu().pc(), 3);
    assert!(cpu.program().unwrap().file.ends_with("Add.asm"));
}

#[test]
fn fail_fast_stops_at_failing_target_row_but_still_aborts_on_earlier_failure() {
    let mut lines = actual_lines();
    lines[1] = lines[1].replace('3', "4");
    lines[2] = lines[2].replace('5', "9");
    let host = host();
    host.insert(&format!("{DIR}/Add.cmp"), lines.join("\n") + "\n");

    // Target is the first failing row (line 2): suspends with TargetLine even
    // when collect_all_mismatches is false.
    let mut on_target = runner(
        &host,
        true,
        RunnerOptions {
            collect_all_mismatches: false,
            stop_at_output_line: Some(2),
            skip_output_write: true,
            ..Default::default()
        },
    );
    assert!(matches!(
        on_target.run(),
        Outcome::Suspended {
            reason: SuspendReason::TargetLine,
            ..
        }
    ));
    assert_eq!(cpu(&on_target).cpu().ram(2), 3);

    // Target is after the first failing row (line 3): fail-fast still stops
    // with FailedComparison at line 2.
    let mut past_failure = runner(
        &host,
        true,
        RunnerOptions {
            collect_all_mismatches: false,
            stop_at_output_line: Some(3),
            skip_output_write: true,
            ..Default::default()
        },
    );
    let Outcome::FailedComparison { mismatch, .. } = past_failure.run() else {
        panic!("expected FailedComparison on earlier row");
    };
    assert_eq!(mismatch.line, 2);
}
