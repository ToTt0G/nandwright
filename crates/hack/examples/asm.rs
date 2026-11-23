//! Assembles a file the way `tools/Assembler.sh` does: `PROG.asm` becomes
//! `PROG.hack` next to it, and nothing is written if there are errors.
//!
//! ```text
//! cargo run -p n2t-hack --example asm -- PROG.asm [OUT.hack]
//! ```
//!
//! Diagnostics go to stderr as `PATH:LINE:COL: severity[code]: message`.
//! On error the last line of stderr is `first-error-line: N`, the 0-based line
//! of the first error counting `\n`, `\r\n` and a lone `\r` as line ends (as
//! the official assembler does); the conformance script uses it. The exit
//! status is then 1.

use std::process::ExitCode;

use n2t_core::LineIndex;
use n2t_hack::{assemble, hack_text};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("usage: asm PROG.asm [OUT.hack]");
        return ExitCode::from(2);
    };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::from(2);
        }
    };
    // Like Java: invalid UTF-8 becomes U+FFFD, and a BOM is kept (and rejected).
    let text = String::from_utf8_lossy(&bytes);
    let assembly = assemble(&text);
    let index = LineIndex::new(&text);
    for d in &assembly.diagnostics {
        let (line, col) = d
            .span
            .map(|s| index.line_col_one_based(&text, s.start))
            .unwrap_or((0, 0));
        eprintln!("{path}:{line}:{col}: {d}");
    }
    if !assembly.is_ok() {
        let first = assembly
            .diagnostics
            .iter()
            .filter(|d| d.is_error())
            .filter_map(|d| d.span)
            .map(|s| s.start)
            .min()
            .unwrap_or(0);
        let before = &text.as_bytes()[..first.min(text.len())];
        let line = before
            .iter()
            .enumerate()
            .filter(|&(i, &b)| (b == b'\n' && (i == 0 || before[i - 1] != b'\r')) || b == b'\r')
            .count();
        eprintln!("first-error-line: {line}");
        return ExitCode::from(1);
    }
    let out = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| match path.strip_suffix(".asm") {
            Some(stem) => format!("{stem}.hack"),
            None => format!("{path}.hack"),
        });
    if let Err(e) = std::fs::write(&out, hack_text(&assembly.words)) {
        eprintln!("{out}: {e}");
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
}
