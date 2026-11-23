//! The Hack assembler, `.hack` reader/writer and disassembler.
//!
//! # Compatibility with the official assembler
//!
//! [`assemble`] accepts exactly the programs the official Java assembler
//! (`tools/Assembler.sh`) accepts and produces the same words. Its
//! diagnostics are more precise, and many surprising-but-accepted constructs
//! get a warning. Behaviour established by experiment:
//!
//! - Spaces and tabs are deleted from a line before it is read, wherever they
//!   are: `D = M` is fine, `@5 6` is `@56`, `@x y` is the symbol `xy`.
//! - Lines end at `\n`, `\r\n` or a lone `\r`. A missing final newline is fine.
//! - A UTF-8 byte order mark at the start of the file is *rejected* (Java reads
//!   it as part of the first line). Callers must therefore pass the text with
//!   its BOM, and should decode invalid UTF-8 lossily (Java substitutes U+FFFD).
//! - Mnemonics are case-sensitive. Dest letters must be in the order of
//!   `AMD` (`MD` yes, `DM` no). `A+D`, `A&D`, `A|D`, `M+D`, `M&D`, `M|D` are
//!   accepted as aliases; `1+D`, `D+D` and the like are not.
//! - `dest=` may be empty (`=M`) and so may `;jump` (`D;`).
//! - `@n` is a number if `n` is a 16-bit signed decimal (`-32768..=32767`,
//!   with an optional sign, Unicode digits allowed). `@-1` assembles to
//!   `1111111111111111`. Anything else after `@` is a symbol, including
//!   `40000`, `1abc`, `a+b` and a lone punctuation character such as `@;`.
//! - Symbols are case-sensitive and may contain `+ - ! & |` and any character
//!   at or above U+0100; a label name may be any single character too (`(;)`).
//! - A label may be defined more than once; the last definition wins. A label
//!   may redefine a predefined symbol (`(SP)`). A number always wins over a
//!   label of the same name.
//! - Variables are allocated from 16 upward without limit, past the screen
//!   and past 32767 (where the address sets the top bit of the word).
//! - At most 32767 instructions: with 32768 the official assembler crashes
//!   while writing the file, and with more it reports "Program too large".

mod lexer;
mod parse;

use std::collections::HashMap;

use n2t_core::{Diagnostic, DiagnosticCode, Span};

use crate::isa;
pub use crate::isa::disassemble;
pub use parse::{
    AInstruction, AsmLine, CInstruction, Field, Item, LabelDef, Name, Operand, ParsedAsm, parse,
};

/// The largest number of instructions the official assembler can output.
pub const MAX_INSTRUCTIONS: usize = 32767;

/// The result of [`assemble`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assembly {
    /// The machine code. Empty if there is any error.
    pub words: Vec<u16>,
    /// For each ROM address, the span of the instruction at that address.
    /// Filled even when there are errors (every line that is not a label or
    /// blank occupies an address), so an editor can still show addresses.
    pub source_map: Vec<Span>,
    /// Labels, variables and the predefined symbols the program uses.
    pub symbols: Vec<Symbol>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    /// `(NAME)`: the value is a ROM address.
    Label,
    /// Allocated from RAM[16] on first use.
    Variable,
    /// `SP`, `R0`…`R15`, `SCREEN`, `KBD`, … (and not redefined by a label).
    Predefined,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    /// ROM address for a label, RAM address otherwise.
    pub address: u16,
    pub kind: SymbolKind,
    /// The label name in its effective `(NAME)` (the last one, if defined more
    /// than once), or the first use of a variable. `None` for predefined symbols.
    pub definition: Option<Span>,
    /// Earlier, overridden definitions of a label defined more than once.
    pub other_definitions: Vec<Span>,
    /// Every `@NAME` that refers to this symbol.
    pub references: Vec<Span>,
}

impl Assembly {
    /// Whether assembly succeeded.
    pub fn is_ok(&self) -> bool {
        !n2t_core::diagnostic::has_errors(&self.diagnostics)
    }

    /// The ROM address of the instruction whose span contains `offset`.
    pub fn address_at(&self, offset: usize) -> Option<u16> {
        let i = self.source_map.partition_point(|s| s.end < offset);
        let span = self.source_map.get(i)?;
        span.contains(offset).then_some(i as u16)
    }

    /// The symbol defined or referenced at `offset`.
    pub fn symbol_at(&self, offset: usize) -> Option<&Symbol> {
        self.symbols.iter().find(|s| {
            s.definition.is_some_and(|d| d.contains(offset))
                || s.other_definitions.iter().any(|d| d.contains(offset))
                || s.references.iter().any(|r| r.contains(offset))
        })
    }
}

fn code(c: &'static str) -> DiagnosticCode {
    DiagnosticCode::Other(c)
}

/// Assembles Hack assembly. Never panics; `words` is empty if there are errors.
pub fn assemble(source: &str) -> Assembly {
    let parsed = parse(source);
    let mut diagnostics = parsed.diagnostics;

    // Pass 1: ROM addresses and labels.
    let mut source_map = Vec::new();
    let mut labels: HashMap<&str, usize> = HashMap::new(); // name -> index in `symbols`
    let mut symbols: Vec<Symbol> = Vec::new();
    let mut too_large = false;
    for line in &parsed.lines {
        let Some(item) = &line.item else { continue };
        match item {
            Item::Label(LabelDef {
                name: Some(name),
                close: Some(_),
                ..
            }) => {
                let address = source_map.len().min(u16::MAX as usize) as u16;
                if let Some(&i) = labels.get(name.text.as_str()) {
                    let sym: &mut Symbol = &mut symbols[i];
                    if let Some(old) = sym.definition.replace(name.span) {
                        sym.other_definitions.push(old);
                    }
                    sym.address = address;
                } else {
                    labels.insert(&name.text, symbols.len());
                    symbols.push(Symbol {
                        name: name.text.clone(),
                        address,
                        kind: SymbolKind::Label,
                        definition: Some(name.span),
                        other_definitions: Vec::new(),
                        references: Vec::new(),
                    });
                }
            }
            Item::Label(_) => {}
            _ => {
                if source_map.len() == MAX_INSTRUCTIONS && !too_large {
                    too_large = true;
                    diagnostics.push(Diagnostic::error(
                        code("asm-program-too-large"),
                        format!(
                            "this is instruction number {}; the official assembler cannot \
                             output more than {MAX_INSTRUCTIONS} instructions",
                            MAX_INSTRUCTIONS + 1
                        ),
                        item.span(),
                    ));
                }
                source_map.push(item.span());
            }
        }
    }

    for sym in &symbols {
        let def = sym.definition.unwrap_or_default();
        for &other in &sym.other_definitions {
            diagnostics.push(
                Diagnostic::warning(
                    code("asm-duplicate-label"),
                    format!(
                        "label `{}` is defined more than once; the official assembler uses the \
                         last definition (ROM[{}]) everywhere",
                        sym.name, sym.address
                    ),
                    other,
                )
                .with_related_span("the definition that is used", def),
            );
        }
        if let Some(value) = isa::predefined(&sym.name) {
            diagnostics.push(Diagnostic::warning(
                code("asm-label-redefines-predefined"),
                format!(
                    "label `{}` replaces the predefined symbol {} (= {value}): `@{}` now means \
                     ROM[{}]. Choose another name",
                    sym.name, sym.name, sym.name, sym.address
                ),
                def,
            ));
        }
        if let Some(n) = lexer::parse_number(&sym.name) {
            diagnostics.push(Diagnostic::warning(
                code("asm-unreachable-label"),
                format!(
                    "label `{}` can never be used: `@{}` is always the number {n}",
                    sym.name, sym.name
                ),
                def,
            ));
        }
    }

    // Pass 2: resolve symbols, allocate variables, emit code.
    let mut words = Vec::with_capacity(source_map.len());
    let mut others: HashMap<String, usize> = HashMap::new(); // variables and predefined
    let mut next_variable: u32 = 16;
    for line in &parsed.lines {
        match &line.item {
            Some(Item::A(a)) => match &a.operand {
                Some(Operand::Number { value, .. }) => words.push(*value as u16),
                Some(Operand::Symbol(name)) => {
                    let index = if let Some(&i) = labels.get(name.text.as_str()) {
                        i
                    } else if let Some(&i) = others.get(&name.text) {
                        i
                    } else {
                        let (address, kind, definition) = match isa::predefined(&name.text) {
                            Some(v) => (v, SymbolKind::Predefined, None),
                            None => {
                                let address = next_variable as u16;
                                variable_warnings(
                                    &name.text,
                                    next_variable,
                                    name.span,
                                    &mut diagnostics,
                                );
                                next_variable += 1;
                                (address, SymbolKind::Variable, Some(name.span))
                            }
                        };
                        others.insert(name.text.clone(), symbols.len());
                        symbols.push(Symbol {
                            name: name.text.clone(),
                            address,
                            kind,
                            definition,
                            other_definitions: Vec::new(),
                            references: Vec::new(),
                        });
                        symbols.len() - 1
                    };
                    symbols[index].references.push(name.span);
                    words.push(symbols[index].address);
                }
                None => words.push(0),
            },
            Some(Item::C(c)) => words.push(c.encode().unwrap_or(0)),
            _ => {}
        }
    }

    if n2t_core::diagnostic::has_errors(&diagnostics) {
        words.clear();
    }
    Assembly {
        words,
        source_map,
        symbols,
        diagnostics,
    }
}

fn variable_warnings(name: &str, address: u32, span: Span, diagnostics: &mut Vec<Diagnostic>) {
    if address == u32::from(isa::SCREEN) {
        diagnostics.push(Diagnostic::warning(
            code("asm-too-many-variables"),
            format!(
                "variable `{name}` is allocated at RAM[{address}], the first word of the screen: \
                 the program has more variables than fit in RAM[16..16383]"
            ),
            span,
        ));
    } else if address == 32768 {
        diagnostics.push(Diagnostic::warning(
            code("asm-too-many-variables"),
            format!(
                "variable `{name}` is allocated at address {address}, which does not fit in an \
                 A-instruction: the word gets its top bit set and runs as a C-instruction"
            ),
            span,
        ));
    }
}

/// Renders machine code as `.hack` text: one line of 16 binary digits per
/// word, each terminated by `\n` (as the official assembler writes on Linux
/// and macOS).
pub fn hack_text(words: &[u16]) -> String {
    let mut s = String::with_capacity(words.len() * 17);
    for &w in words {
        for bit in (0..16).rev() {
            s.push(if w >> bit & 1 == 1 { '1' } else { '0' });
        }
        s.push('\n');
    }
    s
}

/// The most words a `.hack` file may hold (the size of the ROM).
pub const MAX_HACK_WORDS: usize = 32768;

/// Reads `.hack` text. Every line must be exactly 16 binary digits; `\r\n`
/// line endings and blank lines at the end of the file are accepted.
/// Returns one diagnostic per bad line (at most 100).
///
/// This is stricter than the official CPU emulator, which also accepts lines
/// of other lengths and blank lines in the middle of the file (loaded as 0).
pub fn parse_hack(text: &str) -> Result<Vec<u16>, Vec<Diagnostic>> {
    const MAX_DIAGNOSTICS: usize = 100;
    let lines = lexer::split_lines(text);
    let last_nonblank = lines
        .iter()
        .rposition(|s| !text[s.start..s.end].trim().is_empty())
        .map_or(0, |i| i + 1);
    let mut words = Vec::with_capacity(last_nonblank.min(MAX_HACK_WORDS));
    let mut diagnostics = Vec::new();
    for (i, &span) in lines.iter().take(last_nonblank).enumerate() {
        if i == MAX_HACK_WORDS {
            diagnostics.push(Diagnostic::error(
                code("asm-hack-too-large"),
                format!(
                    "line {}: the program has more than {MAX_HACK_WORDS} words and does not \
                     fit in ROM",
                    i + 1
                ),
                span,
            ));
            break;
        }
        let line = &text[span.start..span.end];
        let bytes = line.as_bytes();
        if bytes.len() == 16 && bytes.iter().all(|b| matches!(b, b'0' | b'1')) {
            let w = bytes
                .iter()
                .fold(0u16, |acc, &b| (acc << 1) | u16::from(b - b'0'));
            words.push(w);
        } else if diagnostics.len() < MAX_DIAGNOSTICS {
            let found = if line.trim().is_empty() {
                "a blank line".to_string()
            } else if line.starts_with('\u{feff}') {
                "a byte order mark; save the file as UTF-8 without BOM".to_string()
            } else {
                let shown: String = line.chars().take(40).collect();
                let more = if line.chars().count() > 40 { "…" } else { "" };
                format!("`{shown}{more}` ({} characters)", line.chars().count())
            };
            diagnostics.push(Diagnostic::error(
                code("asm-invalid-hack-line"),
                format!(
                    "line {}: expected exactly 16 binary digits (0 or 1), found {found}",
                    i + 1
                ),
                span,
            ));
        }
    }
    if diagnostics.is_empty() {
        Ok(words)
    } else {
        Err(diagnostics)
    }
}

#[cfg(test)]
mod tests;
