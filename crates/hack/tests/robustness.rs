//! Feeds random and mutated input to every entry point and checks that
//! nothing panics and basic invariants hold.

use std::path::Path;

use n2t_hack::{Cpu, assemble, disassemble, parse, parse_hack};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn check(text: &str) {
    let parsed = parse(text);
    for line in &parsed.lines {
        let _ = line.span.slice(text);
        if let Some(c) = line.comment {
            let _ = c.slice(text);
        }
    }
    let a = assemble(text);
    assert_eq!(
        a.words.is_empty() && !a.source_map.is_empty(),
        !a.is_ok() && !a.source_map.is_empty()
    );
    if a.is_ok() {
        assert_eq!(a.words.len(), a.source_map.len());
    }
    for d in &a.diagnostics {
        if let Some(s) = d.span {
            let _ = s.slice(text);
        }
    }
    for s in &a.symbols {
        for r in s.references.iter().chain(s.definition.iter()) {
            let _ = r.slice(text);
        }
    }
    for span in &a.source_map {
        let _ = span.slice(text);
    }
    let _ = parse_hack(text);
}

const ALPHABET: &[u8] = b"@()=;/ \t\r\nADM01-+!&|JGTEQLNPdamj#.$:_xyz\xc3\xa9\xef\xbb\xbf\xff";

#[test]
fn random_bytes() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..3000 {
        let len = rng.below(200);
        let bytes: Vec<u8> = (0..len)
            .map(|_| {
                if rng.below(4) == 0 {
                    rng.next() as u8
                } else {
                    ALPHABET[rng.below(ALPHABET.len())]
                }
            })
            .collect();
        check(&String::from_utf8_lossy(&bytes));
    }
}

#[test]
fn mutated_programs() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut sources = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "asm" || x == "hack") {
            sources.push(std::fs::read(p).unwrap());
        }
    }
    let mut rng = Rng(42);
    for i in 0..3000 {
        let mut bytes = sources[i % sources.len()].clone();
        for _ in 0..1 + rng.below(8) {
            let pos = rng.below(bytes.len() + 1);
            match rng.below(3) {
                0 if pos < bytes.len() => bytes[pos] = ALPHABET[rng.below(ALPHABET.len())],
                1 if pos < bytes.len() => {
                    bytes.remove(pos);
                }
                _ => bytes.insert(pos, ALPHABET[rng.below(ALPHABET.len())]),
            }
        }
        check(&String::from_utf8_lossy(&bytes));
    }
}

#[test]
fn every_word_disassembles_and_executes() {
    let mut cpu = Cpu::new();
    let words: Vec<u16> = (0..=u16::MAX).collect();
    for &w in &words {
        let _ = disassemble(w);
    }
    let mut rng = Rng(7);
    for chunk in words.chunks(32768) {
        cpu.load_rom(chunk);
        for _ in 0..2000 {
            cpu.set_pc(rng.next() as u16);
            cpu.set_a(rng.next() as u16);
            cpu.set_d(rng.next() as u16);
            let _ = cpu.run(50, &[rng.next() as u16]);
        }
    }
}
