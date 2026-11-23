//! Driver binary for `n2t-vm`: loads a `.vm` file or directory, applies initial
//! variable assignments, runs for a step budget (or until halt), optionally drives
//! keyboard events at fixed step counts, and dumps RAM cells, variables, or the
//! entire 8192-word screen in a stable text format.
//!
//! Usage:
//!   vmrun <file.vm | dir> [options]
//!
//! Options:
//!   --set <name>=<val>         Set a script variable before running (e.g. `sp=256`, `RAM[0]=256`, `argument[0]=3`)
//!   --steps <N>                Run up to N steps (default: 1000000)
//!   --until-halt               Run until halted, error, or --steps limit
//!   --step-ms <M>              Virtual ms per step for Sys.wait (default: 1)
//!   --key-at <STEP>:<CODE>     Set RAM[24576] = CODE before executing step STEP (0-based; repeatable)
//!   --dump <var>               Dump a variable or RAM address after running (e.g. `RAM[256]`, `sp`, `RAM[8000..8014]`)
//!   --dump-screen              Dump all 8192 screen words (`RAM[16384..24576]`), 16 words per row
//!   --status                   Print final status (`STATUS: ...`)
//!   --bench <N>                Measure execution speed over N steps and print Msteps/s

use std::env;
use std::fs;
use std::path::Path;
use std::process;
use std::time::Instant;

use n2t_core::host::decode_text;
use n2t_vm::{Breakpoints, ParsedVm, Program, RunStatus, StepStatus, Vm, parse};

fn parse_var_spec(spec: &str) -> (&str, Option<i64>) {
    if let Some(open) = spec.find('[')
        && let Some(close) = spec.rfind(']')
        && close > open
    {
        let name = &spec[..open];
        if let Ok(idx) = spec[open + 1..close].trim().parse::<i64>() {
            return (name, Some(idx));
        }
    }
    (spec, None)
}

fn load_inputs(target: &Path) -> Result<Vec<ParsedVm>, String> {
    if target.is_file() {
        let stem = target
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("invalid file name: {}", target.display()))?;
        let bytes = fs::read(target).map_err(|e| format!("read {}: {e}", target.display()))?;
        let text = decode_text(bytes).text;
        return Ok(vec![parse(stem, &text)]);
    }
    if target.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(target)
            .map_err(|e| format!("read_dir {}: {e}", target.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case("vm"))
            })
            .collect();
        entries.sort();
        if entries.is_empty() {
            return Err(format!("no .vm files found in {}", target.display()));
        }
        let mut parsed = Vec::with_capacity(entries.len());
        for p in entries {
            let stem = p
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| format!("invalid file name: {}", p.display()))?;
            let bytes = fs::read(&p).map_err(|e| format!("read {}: {e}", p.display()))?;
            let text = decode_text(bytes).text;
            parsed.push(parse(stem, &text));
        }
        return Ok(parsed);
    }
    Err(format!("path does not exist: {}", target.display()))
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!(
            "usage: vmrun <file.vm|dir> [--set K=V] [--steps N] [--until-halt] [--key-at S:C] [--dump V] [--dump-screen] [--status] [--bench N]"
        );
        process::exit(2);
    }

    let input_path = Path::new(&args[0]);
    let mut sets: Vec<(String, i64)> = Vec::new();
    let mut dumps: Vec<String> = Vec::new();
    let mut key_events: Vec<(u64, u16)> = Vec::new();
    let mut steps: u64 = 1_000_000;
    let mut step_ms: u64 = 1;
    let mut dump_screen = false;
    let mut show_status = false;
    let mut bench_steps: Option<u64> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--set" => {
                i += 1;
                let kv = args.get(i).unwrap_or_else(|| {
                    eprintln!("--set requires NAME=VAL");
                    process::exit(2);
                });
                let (k, v) = kv.split_once('=').unwrap_or_else(|| {
                    eprintln!("invalid --set argument: {kv}");
                    process::exit(2);
                });
                let val: i64 = v.trim().parse().unwrap_or_else(|_| {
                    eprintln!("invalid integer in --set: {v}");
                    process::exit(2);
                });
                sets.push((k.trim().to_string(), val));
            }
            "--steps" => {
                i += 1;
                steps = args.get(i).and_then(|s| s.parse().ok()).unwrap_or_else(|| {
                    eprintln!("--steps requires a non-negative integer");
                    process::exit(2);
                });
            }
            "--until-halt" => {}
            "--step-ms" => {
                i += 1;
                step_ms = args.get(i).and_then(|s| s.parse().ok()).unwrap_or_else(|| {
                    eprintln!("--step-ms requires an integer");
                    process::exit(2);
                });
            }
            "--key-at" => {
                i += 1;
                let spec = args.get(i).unwrap_or_else(|| {
                    eprintln!("--key-at requires STEP:CODE");
                    process::exit(2);
                });
                let (s_str, c_str) = spec.split_once(':').unwrap_or_else(|| {
                    eprintln!("invalid --key-at: {spec}");
                    process::exit(2);
                });
                let at: u64 = s_str.parse().unwrap_or_else(|_| {
                    eprintln!("invalid step in --key-at: {s_str}");
                    process::exit(2);
                });
                let code: u16 = c_str.parse().unwrap_or_else(|_| {
                    eprintln!("invalid key code in --key-at: {c_str}");
                    process::exit(2);
                });
                key_events.push((at, code));
            }
            "--dump" => {
                i += 1;
                let spec = args.get(i).unwrap_or_else(|| {
                    eprintln!("--dump requires a variable or range");
                    process::exit(2);
                });
                dumps.push(spec.clone());
            }
            "--dump-screen" => dump_screen = true,
            "--status" => show_status = true,
            "--bench" => {
                i += 1;
                bench_steps = Some(args.get(i).and_then(|s| s.parse().ok()).unwrap_or_else(|| {
                    eprintln!("--bench requires an integer");
                    process::exit(2);
                }));
            }
            other => {
                eprintln!("unknown argument: {other}");
                process::exit(2);
            }
        }
        i += 1;
    }

    let parsed = load_inputs(input_path).unwrap_or_else(|e| {
        eprintln!("load error: {e}");
        process::exit(1);
    });
    for f in &parsed {
        for d in &f.diagnostics {
            if d.is_error() {
                eprintln!("parse error in {}.vm: {d}", f.file_stem);
                process::exit(1);
            }
        }
    }
    let (program, link_diags) = Program::link(parsed);
    for d in &link_diags {
        if d.is_error() {
            eprintln!("link error: {d}");
            process::exit(1);
        }
    }

    let mut vm = Vm::new(program);
    for (k, v) in &sets {
        let (name, idx) = parse_var_spec(k);
        if let Err(e) = vm.set_var(name, idx, *v) {
            eprintln!("set error ({k}={v}): {e}");
            process::exit(1);
        }
    }

    if let Some(n) = bench_steps {
        let bp = Breakpoints::new();
        let t0 = Instant::now();
        let status = vm.run(n, 0, &bp);
        let dt = t0.elapsed().as_secs_f64();
        let mops = (vm.steps() as f64) / dt / 1_000_000.0;
        println!(
            "BENCH: {} steps in {:.4}s ({:.2} Msteps/s), status={status:?}",
            vm.steps(),
            dt,
            mops
        );
        return;
    }

    key_events.sort_by_key(|&(at, _)| at);
    let mut next_key_idx = 0usize;
    let mut now_ms: u64 = 0;
    let bp = Breakpoints::new();

    let final_status: String = if key_events.is_empty() {
        let mut last = RunStatus::Budget;
        while vm.steps() < steps {
            let remaining = steps - vm.steps();
            last = vm.run(remaining, now_ms, &bp);
            match &last {
                RunStatus::Budget => break,
                RunStatus::Sleeping { until_ms } => {
                    // Advance virtual clock in script mode so Sys.wait completes in the same vmstep.
                    now_ms = *until_ms;
                }
                RunStatus::Halted
                | RunStatus::Error(_)
                | RunStatus::WaitingForKey
                | RunStatus::Breakpoint { .. } => break,
            }
        }
        format!("{last:?}")
    } else {
        let mut last = StepStatus::Stepped;
        for step_i in 0..steps {
            while next_key_idx < key_events.len() && key_events[next_key_idx].0 == step_i {
                vm.set_key(key_events[next_key_idx].1);
                next_key_idx += 1;
            }
            loop {
                last = vm.step(now_ms);
                match last {
                    StepStatus::Sleeping { until_ms } => {
                        // Complete Sys.wait within this single vmstep (matching Java VMEmulator).
                        now_ms = until_ms;
                        continue;
                    }
                    StepStatus::Stepped | StepStatus::WaitingForKey => {
                        now_ms = now_ms.saturating_add(step_ms);
                        break;
                    }
                    StepStatus::Halted | StepStatus::Error(_) => break,
                }
            }
            if matches!(last, StepStatus::Halted | StepStatus::Error(_)) {
                break;
            }
        }
        format!("{last:?}")
    };

    if show_status {
        println!("STATUS: {final_status}");
    }

    for spec in &dumps {
        if let Some(rest) = spec.strip_prefix("RAM[").and_then(|s| s.strip_suffix(']'))
            && let Some((lo_s, hi_s)) = rest.split_once("..")
            && let (Ok(lo), Ok(hi)) = (lo_s.parse::<u16>(), hi_s.parse::<u16>())
        {
            for addr in lo..hi {
                println!("RAM[{addr}]={}", vm.ram(addr));
            }
            continue;
        }
        let (name, idx) = parse_var_spec(spec);
        if idx.is_none() && (name == "currentFunction" || name == "line") {
            match vm.get_var_text(name) {
                Ok(val) => println!("{spec}={val}"),
                Err(e) => {
                    eprintln!("dump error ({spec}): {e}");
                    process::exit(1);
                }
            }
        } else {
            match vm.get_var(name, idx) {
                Ok(val) => println!("{spec}={val}"),
                Err(e) => {
                    eprintln!("dump error ({spec}): {e}");
                    process::exit(1);
                }
            }
        }
    }

    if dump_screen {
        let screen = vm.screen_slice();
        for (row, chunk) in screen.chunks(16).enumerate() {
            let base = 16384 + row * 16;
            print!("SCREEN[{base}]:");
            for w in chunk {
                print!(" {w}");
            }
            println!();
        }
    }
}
