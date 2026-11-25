//! A Hack computer running a program: the CPU emulator behind the panel with
//! the screen, the keyboard, the registers and the memory.

use n2t_core::host::{SharedHost, path};
use n2t_hack::{Cpu, RunStatus};
use n2t_simulator::CpuMachine;
use n2t_simulator::machine::Files;
use serde_json::{Value, json};

use crate::chip::parse_value;
use crate::{Failure, Reply, text};

/// Instructions per `cpu.run` when the caller does not say.
const DEFAULT_BUDGET: u64 = 100_000;
/// Most memory cells returned by one request.
const MAX_WINDOW: u64 = 512;

pub struct ComputerSession {
    host: SharedHost,
    file: String,
    machine: CpuMachine,
    /// The error that stopped the program, until it is reset.
    stopped: Option<String>,
    /// The screen generation last sent to the caller.
    sent_screen: Option<u64>,
}

impl ComputerSession {
    /// `{"op":"cpu.open","path":…}` for a `.asm` or `.hack` file.
    pub fn open(host: &SharedHost, request: &Value) -> Result<Self, Failure> {
        let file = path::normalize(text(request, "path")?);
        let mut session = Self {
            host: host.clone(),
            file,
            machine: CpuMachine::new(),
            stopped: None,
            sent_screen: None,
        };
        session.load()?;
        Ok(session)
    }

    fn load(&mut self) -> Result<(), Failure> {
        let dir = path::dir(&self.file).to_string();
        let files = Files::plain(&self.host, &dir);
        let file = self.file.clone();
        self.machine.load_file(&files, &file)?;
        self.stopped = None;
        Ok(())
    }

    fn cpu(&mut self) -> &mut Cpu {
        self.machine.cpu_mut()
    }

    pub fn call(&mut self, op: &str, request: &Value) -> Reply {
        let mut status = "paused";
        match op {
            "cpu.state" => {}
            "cpu.run" | "cpu.step" => {
                if let Some(message) = &self.stopped {
                    return Err(format!("{message} Reset the program to run it again."));
                }
                let budget = if op == "cpu.step" {
                    crate::budget(request, "count", 1)
                } else {
                    crate::budget(request, "budget", DEFAULT_BUDGET)
                };
                let breakpoints = self.breakpoint_addresses(request);
                match self.cpu().run(budget, &breakpoints) {
                    RunStatus::Budget => {
                        status = if op == "cpu.step" {
                            "paused"
                        } else {
                            "running"
                        }
                    }
                    RunStatus::Breakpoint(_) => status = "breakpoint",
                    RunStatus::Error(e) => {
                        self.stopped = Some(e.to_string());
                        status = "error";
                    }
                }
            }
            "cpu.reset" => {
                // A fresh machine: the program is read again (so an edit
                // takes effect) and memory is cleared.
                let key = self.machine.cpu().key();
                self.machine = CpuMachine::new();
                self.sent_screen = None;
                self.load()?;
                self.cpu().set_key(key);
            }
            "cpu.restart" => {
                // What the reset button of the official emulator does: back
                // to the first instruction, memory kept.
                self.cpu().set_pc(0);
                self.stopped = None;
            }
            "cpu.set" => {
                let target = text(request, "target")?.to_string();
                let value = parse_value(&request["value"], 16)? as u16;
                self.set(&target, request["address"].as_u64(), value)?;
            }
            "key" => {
                let code = crate::key_code(request)?;
                self.cpu().set_key(code);
                return Ok(json!({ "accepted": true }));
            }
            other => return Err(format!("`{other}` is not an operation of the CPU emulator")),
        }
        Ok(self.state(status, request))
    }

    /// ROM addresses for the zero-based source lines in `breakLines`.
    fn breakpoint_addresses(&self, request: &Value) -> Vec<u16> {
        let Some(lines) = request["breakLines"].as_array() else {
            return Vec::new();
        };
        let Some(program) = self.machine.program() else {
            return Vec::new();
        };
        let mut addresses = Vec::new();
        for line in lines.iter().filter_map(Value::as_u64) {
            // A breakpoint on a line without an instruction (a label, a
            // comment) stops at the next instruction after it.
            if let Some(address) = program.lines.iter().position(|&l| u64::from(l) >= line) {
                addresses.push(address as u16);
            }
        }
        addresses
    }

    fn set(&mut self, target: &str, address: Option<u64>, value: u16) -> Result<(), Failure> {
        match target {
            "A" => self.cpu().set_a(value),
            "D" => self.cpu().set_d(value),
            "PC" => {
                if value > 0x7FFF {
                    return Err("PC must be a ROM address, 0 to 32767".into());
                }
                self.cpu().set_pc(value);
            }
            "RAM" => {
                let address = address.ok_or("which RAM address?")?;
                if address > 24576 || !self.cpu().set_ram(address as u16, value) {
                    return Err(format!(
                        "RAM[{address}] does not exist; the last address is 24576"
                    ));
                }
            }
            other => return Err(format!("`{other}` cannot be set")),
        }
        Ok(())
    }

    pub fn state(&mut self, status: &str, request: &Value) -> Value {
        let (line, file) = {
            let cpu = self.machine.cpu();
            match self.machine.program() {
                Some(program) => (
                    program.lines.get(cpu.pc() as usize).copied(),
                    Some(program.file.clone()),
                ),
                None => (None, None),
            }
        };
        let cpu = self.machine.cpu();
        let pc = cpu.pc();
        let a = cpu.a();
        let memory = match request["ram"].as_array() {
            Some(window) => {
                let start = window
                    .first()
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .min(24576);
                let count = window
                    .get(1)
                    .and_then(Value::as_u64)
                    .unwrap_or(16)
                    .min(MAX_WINDOW);
                let end = (start + count).min(24577);
                let values: Vec<u16> = (start..end)
                    .map(|address| cpu.ram(address as u16))
                    .collect();
                json!({ "start": start, "values": values })
            }
            None => Value::Null,
        };
        let generation = cpu.screen_generation();
        // `true` sends the screen if it changed since it was last sent;
        // `"force"` sends it regardless (a panel that has just opened).
        let force = request["screen"].as_str() == Some("force");
        let screen = if force
            || (request["screen"].as_bool().unwrap_or(false)
                && self.sent_screen != Some(generation))
        {
            Some(hex_words(cpu.screen()))
        } else {
            None
        };
        let state = json!({
            "kind": "cpu",
            "status": status,
            "error": self.stopped,
            "registers": [
                { "name": "A", "value": a },
                { "name": "D", "value": cpu.d() },
                { "name": "PC", "value": pc },
                { "name": "M", "value": if a <= 24576 { json!(cpu.ram(a)) } else { Value::Null } },
            ],
            "instruction": n2t_hack::disassemble(cpu.rom(pc)),
            "steps": cpu.cycles(),
            "programLength": cpu.rom_len(),
            "file": file,
            "line": line,
            "key": cpu.key(),
            "ram": memory,
            "screen": screen,
        });
        if screen_sent(&state) {
            self.sent_screen = Some(generation);
        }
        state
    }
}

fn screen_sent(state: &Value) -> bool {
    !state["screen"].is_null()
}

/// The screen as text: four hexadecimal digits per word. Smaller and faster
/// to produce and read than a JSON array of 8192 numbers.
fn hex_words(words: &[u16]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(words.len() * 4);
    for &word in words {
        for shift in [12, 8, 4, 0] {
            out.push(DIGITS[((word >> shift) & 0xF) as usize] as char);
        }
    }
    out
}
