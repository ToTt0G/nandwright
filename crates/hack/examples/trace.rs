//! Runs a CPU-emulator test script and prints the rows that its `output`
//! commands produce, for comparison with the official CPU emulator (see
//! `conformance/hack`).
//!
//! ```text
//! cargo run -p n2t-hack --example trace -- SCRIPT.tst
//! ```
//!
//! Only the subset of the test-script language that the conformance script
//! generates is understood: `load P.asm|P.hack`, `output-file`, `compare-to`
//! (ignored), `output-list A D PC RAM[n] ROM[n]` (formats are ignored),
//! `set A|D|PC|RAM[n]|ROM[n] value`, `ticktock`, `output`, `echo` and
//! `repeat N { ... }`.
//!
//! Each `output` prints the listed values as signed decimals separated by
//! spaces. If an instruction fails the script stops, as the official one does,
//! after printing `ERROR <message>`.

use std::process::ExitCode;

use n2t_hack::{Cpu, assemble, parse_hack};

#[derive(Debug, Clone)]
enum Cmd {
    Load(String),
    OutputList(Vec<String>),
    Set(String, i32),
    Ticktock,
    Output,
    Repeat(u64, Vec<Cmd>),
    Ignore,
}

fn tokenize(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in src.lines() {
        let line = line.split("//").next().unwrap_or("");
        let mut cur = String::new();
        let mut in_quote = false;
        for c in line.chars() {
            if c == '"' {
                in_quote = !in_quote;
                cur.push(c);
            } else if in_quote {
                cur.push(c);
            } else if c.is_whitespace() {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            } else if matches!(c, ',' | ';' | '{' | '}') {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                out.push(c.to_string());
            } else {
                cur.push(c);
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
    }
    out
}

fn parse(tokens: &[String], i: &mut usize) -> Result<Vec<Cmd>, String> {
    let mut cmds = Vec::new();
    while *i < tokens.len() {
        let t = tokens[*i].as_str();
        *i += 1;
        let mut args = Vec::new();
        let cmd = match t {
            "}" => return Ok(cmds),
            "," | ";" => continue,
            "repeat" => {
                let n = tokens.get(*i).and_then(|s| s.parse().ok()).ok_or("repeat count")?;
                *i += 1;
                if tokens.get(*i).map(String::as_str) != Some("{") {
                    return Err("expected {".into());
                }
                *i += 1;
                Cmd::Repeat(n, parse(tokens, i)?)
            }
            _ => {
                while *i < tokens.len() && !matches!(tokens[*i].as_str(), "," | ";" | "{" | "}") {
                    args.push(tokens[*i].clone());
                    *i += 1;
                }
                match t {
                    "load" => Cmd::Load(args.first().cloned().ok_or("load what?")?),
                    "output-list" => Cmd::OutputList(
                        args.iter()
                            .map(|a| a.split('%').next().unwrap_or("").to_string())
                            .collect(),
                    ),
                    "set" => {
                        let name = args.first().cloned().ok_or("set what?")?;
                        let v = args.get(1).and_then(|s| s.parse().ok()).ok_or("set value")?;
                        Cmd::Set(name, v)
                    }
                    "ticktock" => Cmd::Ticktock,
                    "output" => Cmd::Output,
                    "output-file" | "compare-to" | "echo" => Cmd::Ignore,
                    other => return Err(format!("unsupported command {other}")),
                }
            }
        };
        cmds.push(cmd);
    }
    Ok(cmds)
}

fn index(name: &str, prefix: &str) -> Option<u16> {
    name.strip_prefix(prefix)?.strip_suffix(']')?.parse().ok()
}

struct Runner {
    cpu: Cpu,
    dir: String,
    list: Vec<String>,
    out: String,
}

impl Runner {
    fn value(&self, name: &str) -> i16 {
        (match name {
            "A" => self.cpu.a(),
            "D" => self.cpu.d(),
            "PC" => self.cpu.pc(),
            _ => {
                if let Some(i) = index(name, "RAM[") {
                    self.cpu.ram(i)
                } else if let Some(i) = index(name, "ROM[") {
                    self.cpu.rom(i)
                } else {
                    0
                }
            }
        }) as i16
    }

    /// Returns `Err` with a message when the script must stop.
    fn exec(&mut self, cmds: &[Cmd]) -> Result<(), String> {
        for cmd in cmds {
            match cmd {
                Cmd::Load(file) => {
                    let path = format!("{}/{file}", self.dir);
                    let bytes = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
                    let text = String::from_utf8_lossy(&bytes);
                    let words = if file.ends_with(".hack") {
                        parse_hack(&text).map_err(|d| format!("{path}: {}", d[0].message))?
                    } else {
                        let a = assemble(&text);
                        if !a.is_ok() {
                            return Err(format!("{path}: does not assemble"));
                        }
                        a.words
                    };
                    self.cpu.load_rom(&words);
                }
                Cmd::OutputList(l) => self.list = l.clone(),
                Cmd::Set(name, v) => {
                    let v = *v as u16;
                    match name.as_str() {
                        "A" => self.cpu.set_a(v),
                        "D" => self.cpu.set_d(v),
                        "PC" => self.cpu.set_pc(v),
                        _ => {
                            if let Some(i) = index(name, "RAM[") {
                                self.cpu.set_ram(i, v);
                            } else if let Some(i) = index(name, "ROM[") {
                                self.cpu.set_rom(i, v);
                            } else {
                                return Err(format!("cannot set {name}"));
                            }
                        }
                    }
                }
                Cmd::Ticktock => {
                    if let Err(e) = self.cpu.step() {
                        self.out.push_str(&format!("ERROR {e}\n"));
                        return Err(String::new());
                    }
                }
                Cmd::Output => {
                    let row: Vec<String> =
                        self.list.iter().map(|n| self.value(n).to_string()).collect();
                    self.out.push_str(&row.join(" "));
                    self.out.push('\n');
                }
                Cmd::Repeat(n, body) => {
                    for _ in 0..*n {
                        self.exec(body)?;
                    }
                }
                Cmd::Ignore => {}
            }
        }
        Ok(())
    }
}

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: trace SCRIPT.tst");
        return ExitCode::from(2);
    };
    let src = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::from(2);
        }
    };
    let tokens = tokenize(&src);
    let cmds = match parse(&tokens, &mut 0) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::from(2);
        }
    };
    let dir = match path.rfind('/') {
        Some(i) => path[..i].to_string(),
        None => ".".to_string(),
    };
    let mut runner = Runner {
        cpu: Cpu::new(),
        dir,
        list: Vec::new(),
        out: String::new(),
    };
    let result = runner.exec(&cmds);
    print!("{}", runner.out);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.is_empty() => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}
