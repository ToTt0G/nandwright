//! Loading Hack programs from files.

use n2t_core::host::{SharedHost, path, read_text};
use n2t_core::source::LineIndex;

/// Loads a program as machine words: a `.hack` file as it is, a `.asm` file
/// through the assembler. Errors are sentences for the student.
pub fn load_program(host: &SharedHost, file: &str) -> Result<Vec<u16>, String> {
    let name = path::file_name(file);
    let text = read_text(host.as_ref(), file).map_err(|e| format!("cannot read `{name}`: {e}"))?;
    if path::has_extension(file, "hack") {
        return parse_hack(&text.text).map_err(|m| format!("`{name}`: {m}"));
    }
    if path::has_extension(file, "asm") {
        return assemble(name, &text.text);
    }
    Err(format!(
        "`{name}` is not a program; expected a `.hack` or `.asm` file"
    ))
}

fn assemble(name: &str, source: &str) -> Result<Vec<u16>, String> {
    let assembly = n2t_hack::assemble(source);
    match assembly.diagnostics.iter().find(|d| d.is_error()) {
        None => Ok(assembly.words),
        Some(d) => {
            let at = d.span.map_or(String::new(), |span| {
                let (line, column) = LineIndex::new(source).line_col_one_based(source, span.start);
                format!(":{line}:{column}")
            });
            Err(format!(
                "`{name}` does not assemble:\n  {name}{at}: {}",
                d.message
            ))
        }
    }
}

/// Parses a `.hack` program: one 16-digit binary word per non-empty line.
pub fn parse_hack(text: &str) -> Result<Vec<u16>, String> {
    let mut words = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let word = (line.len() == 16 && line.bytes().all(|b| b == b'0' || b == b'1'))
            .then(|| u16::from_str_radix(line, 2).ok())
            .flatten()
            .ok_or_else(|| format!("line {} is not a 16-bit binary word", number + 1))?;
        words.push(word);
    }
    if words.len() > 32768 {
        return Err(format!(
            "the program has {} words; the ROM holds 32768",
            words.len()
        ));
    }
    Ok(words)
}
