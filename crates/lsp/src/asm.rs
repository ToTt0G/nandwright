//! Language features for Hack assembly (`.asm`) and machine code (`.hack`).

use std::collections::HashMap;

use lsp_types as lsp;
use lsp_types::Url;
use n2t_core::Span;
use n2t_hack::asm::{AsmLine, CInstruction, Item, Operand};
use n2t_hack::isa;
use n2t_hack::{Symbol, SymbolKind};

use crate::convert::Text;
use crate::workspace::{AsmState, Workspace};

fn markdown(value: String, range: Option<lsp::Range>) -> lsp::Hover {
    lsp::Hover {
        contents: lsp::HoverContents::Markup(lsp::MarkupContent {
            kind: lsp::MarkupKind::Markdown,
            value,
        }),
        range,
    }
}

/// The parsed line containing `offset` (a cursor at the end of a line
/// belongs to it).
fn line_at(state: &AsmState, offset: usize) -> Option<&AsmLine> {
    let i = state.parsed.lines.partition_point(|l| l.span.end < offset);
    state
        .parsed
        .lines
        .get(i)
        .filter(|l| l.span.start <= offset && offset <= l.span.end)
}

/// The symbol whose definition or reference covers `offset`, with that span.
fn symbol_at(state: &AsmState, offset: usize) -> Option<(&Symbol, Span)> {
    for s in &state.assembly.symbols {
        let spans = s
            .definition
            .iter()
            .chain(&s.other_definitions)
            .chain(&s.references);
        for span in spans {
            if span.start <= offset && offset <= span.end {
                return Some((s, *span));
            }
        }
    }
    None
}

/// Every span of a symbol: definitions first, then references (deduplicated:
/// a variable's definition is its first reference).
fn symbol_spans(symbol: &Symbol) -> Vec<(Span, bool)> {
    let mut spans: Vec<(Span, bool)> = Vec::new();
    let defined = symbol.kind == SymbolKind::Label;
    for span in symbol.definition.iter().chain(&symbol.other_definitions) {
        spans.push((*span, defined));
    }
    for span in &symbol.references {
        if !spans.iter().any(|(s, _)| s == span) {
            spans.push((*span, false));
        }
    }
    spans
}

/// What a predefined symbol stands for.
fn predefined_role(name: &str) -> &'static str {
    match name {
        "SP" => "the stack pointer (VM)",
        "LCL" => "the base of the `local` segment (VM)",
        "ARG" => "the base of the `argument` segment (VM)",
        "THIS" => "the base of the `this` segment (VM)",
        "THAT" => "the base of the `that` segment (VM)",
        "SCREEN" => "the first word of the screen memory map",
        "KBD" => "the keyboard register",
        _ => "a general-purpose register",
    }
}

fn describe_symbol(symbol: &Symbol) -> String {
    let uses = symbol.references.len();
    let uses = format!("{uses} use{}", if uses == 1 { "" } else { "s" });
    match symbol.kind {
        SymbolKind::Label => format!(
            "**label** `{}` = ROM[{}]\n\nThe address of the instruction after `({})`; {uses}.",
            symbol.name, symbol.address, symbol.name
        ),
        SymbolKind::Variable => format!(
            "**variable** `{}` at RAM[{}]\n\nVariables get RAM addresses from 16 upward in the \
             order of their first use; {uses}.",
            symbol.name, symbol.address
        ),
        SymbolKind::Predefined => format!(
            "**predefined symbol** `{}` = {}\n\n{}.",
            symbol.name,
            symbol.address,
            predefined_role(&symbol.name)
        ),
    }
}

/// `M` is the memory word addressed by A.
fn operand_words(text: &str) -> String {
    if text == "M" {
        "RAM[A]".to_string()
    } else {
        text.to_string()
    }
}

/// A computation in words: `D+M` is `D + RAM[A]`, `!A` is `NOT A`.
pub fn describe_comp(mnemonic: &str) -> String {
    if let Some(rest) = mnemonic.strip_prefix('!') {
        return format!("NOT {} (bitwise)", operand_words(rest));
    }
    if mnemonic.len() > 1
        && let Some(rest) = mnemonic.strip_prefix('-')
    {
        return format!("-{}", operand_words(rest));
    }
    for (op, word) in [('+', "+"), ('-', "-"), ('&', "AND"), ('|', "OR")] {
        if let Some((a, b)) = mnemonic.split_once(op) {
            return format!("{} {word} {}", operand_words(a), operand_words(b));
        }
    }
    operand_words(mnemonic)
}

fn jump_condition(jump: &str) -> Option<&'static str> {
    Some(match jump {
        "JGT" => "> 0",
        "JEQ" => "= 0",
        "JGE" => ">= 0",
        "JLT" => "< 0",
        "JNE" => "!= 0",
        "JLE" => "<= 0",
        _ => return None,
    })
}

/// What a C-instruction does, in words:
/// `D=M+1;JGT` is "D = RAM[A] + 1, then jump to A if the result > 0".
pub fn describe_c(dest: &str, comp: &str, jump: &str) -> String {
    let value = describe_comp(comp);
    let targets: Vec<&str> = dest
        .chars()
        .map(|c| match c {
            'A' => "A",
            'M' => "RAM[A]",
            _ => "D",
        })
        .collect();
    let mut text = if targets.is_empty() {
        String::new()
    } else {
        format!("{} = {value}", targets.join(", "))
    };
    let jumps = !jump.is_empty();
    match (text.is_empty(), jumps) {
        (true, false) => text = format!("compute {value} (the result is not stored)"),
        (true, true) => {
            text = match jump_condition(jump) {
                Some(cond) => format!("jump to A if {value} {cond}"),
                None => "jump to A".to_string(),
            }
        }
        (false, true) => {
            text.push_str(&match jump_condition(jump) {
                Some(cond) => format!(", then jump to A if the result {cond}"),
                None => ", then jump to A".to_string(),
            });
        }
        (false, false) => {}
    }
    if dest.contains('A') && dest.contains('M') {
        text.push_str(" (RAM[A] is the word at the old A)");
    }
    if dest.contains('A') && jumps {
        text.push_str(" (the jump goes to the old A)");
    }
    text
}

fn c_words(c: &CInstruction) -> Option<(String, String, String)> {
    let comp = c.comp.as_ref()?;
    let comp = isa::comp_mnemonic(comp.code?)?;
    let dest = match &c.dest {
        Some(f) => isa::DESTS.get(usize::from(f.code?))?,
        None => "",
    };
    let jump = match &c.jump {
        Some(f) => isa::JUMPS.get(usize::from(f.code?))?,
        None => "",
    };
    Some((dest.to_string(), comp.to_string(), jump.to_string()))
}

/// The machine word of an instruction line, if it is valid.
fn word_of(state: &AsmState, item: &Item) -> Option<u16> {
    match item {
        Item::A(a) => match a.operand.as_ref()? {
            Operand::Number { value, .. } => Some(*value as u16),
            Operand::Symbol(name) => state
                .assembly
                .symbols
                .iter()
                .find(|s| s.name == name.text)
                .map(|s| s.address),
        },
        Item::C(c) => c.encode(),
        Item::Label(_) => None,
    }
}

fn instruction_markdown(state: &AsmState, item: &Item) -> Option<String> {
    if !item.is_instruction() {
        return None;
    }
    let address = state.assembly.address_at(item.span().start);
    let word = word_of(state, item);
    let mut lines = Vec::new();
    match (word, address) {
        (Some(w), Some(a)) => lines.push(format!("`{w:016b}` at ROM[{a}]")),
        (Some(w), None) => lines.push(format!("`{w:016b}`")),
        (None, Some(a)) => lines.push(format!("ROM[{a}] (does not assemble)")),
        (None, None) => {}
    }
    match item {
        Item::A(a) => match a.operand.as_ref() {
            Some(Operand::Number { value, .. }) if *value >= 0 => {
                lines.push(format!("A = {value}"));
            }
            Some(Operand::Number { value, .. }) => lines.push(format!(
                "the word {:016b} has its top bit set and runs as a C-instruction (`{}`), \
                 not as `A = {value}`",
                *value as u16,
                isa::disassemble(*value as u16)
            )),
            Some(Operand::Symbol(name)) => {
                if let Some(w) = word {
                    lines.push(format!("A = {w} (`{}`)", name.text));
                }
            }
            None => {}
        },
        Item::C(c) => {
            if let Some((dest, comp, jump)) = c_words(c) {
                lines.push(describe_c(&dest, &comp, &jump));
            }
        }
        Item::Label(_) => {}
    }
    (!lines.is_empty()).then(|| lines.join("\n\n"))
}

pub fn hover(ws: &mut Workspace, uri: &Url, position: lsp::Position) -> Option<lsp::Hover> {
    let state = ws.asm_state(uri)?;
    let doc = ws.docs.get(uri)?;
    let text = doc.text();
    let offset = text.offset(position);
    let line = line_at(&state, offset)?;
    if line.comment.is_some_and(|c| c.start <= offset) {
        return None;
    }
    let item = line.item.as_ref()?;
    let symbol = symbol_at(&state, offset);
    let instruction = instruction_markdown(&state, item);
    let (value, span) = match (symbol, instruction) {
        (Some((s, span)), Some(i)) => (format!("{}\n\n---\n\n{i}", describe_symbol(s)), span),
        (Some((s, span)), None) => (describe_symbol(s), span),
        (None, Some(i)) => (i, item.span()),
        (None, None) => return None,
    };
    Some(markdown(value, Some(text.range(span))))
}

/// Hover on a line of a `.hack` file: the disassembled instruction.
pub fn hack_hover(ws: &Workspace, uri: &Url, position: lsp::Position) -> Option<lsp::Hover> {
    let doc = ws.docs.get(uri)?;
    let text = doc.text();
    let start = doc.lines.line_start(position.line)?;
    let rest = doc.text.get(start..)?;
    let end = start + rest.find(['\n', '\r']).unwrap_or(rest.len());
    let line = doc.text.get(start..end)?;
    let bytes = line.as_bytes();
    if bytes.len() != 16 || !bytes.iter().all(|b| matches!(b, b'0' | b'1')) {
        return None;
    }
    let word = bytes
        .iter()
        .fold(0u16, |acc, &b| (acc << 1) | u16::from(b - b'0'));
    let assembly = isa::disassemble(word);
    let mut value = format!("ROM[{}]: `{assembly}`", position.line);
    match isa::decode(word) {
        isa::Instruction::A(v) => value.push_str(&format!("\n\nA = {v}")),
        isa::Instruction::C { comp, dest, jump } => {
            if let Some(c) = isa::comp_mnemonic(comp) {
                let dest = isa::DESTS.get(usize::from(dest)).copied().unwrap_or("");
                let jump = isa::JUMPS.get(usize::from(jump)).copied().unwrap_or("");
                value.push_str(&format!("\n\n{}", describe_c(dest, c, jump)));
            } else {
                value.push_str("\n\nan undocumented computation: no assembly mnemonic produces it");
            }
        }
        isa::Instruction::Empty => {
            value.push_str("\n\nthe empty-ROM word: the CPU emulator skips it");
        }
        isa::Instruction::Illegal(_) => {
            value.push_str("\n\nnot a valid instruction: the CPU emulator stops here");
        }
    }
    Some(markdown(value, Some(text.range(Span::new(start, end)))))
}

pub fn definition(ws: &mut Workspace, uri: &Url, position: lsp::Position) -> Option<lsp::Location> {
    let state = ws.asm_state(uri)?;
    let doc = ws.docs.get(uri)?;
    let text = doc.text();
    let (symbol, _) = symbol_at(&state, text.offset(position))?;
    let span = symbol.definition?;
    Some(lsp::Location::new(uri.clone(), text.range(span)))
}

pub fn references(
    ws: &mut Workspace,
    uri: &Url,
    position: lsp::Position,
    include_declaration: bool,
) -> Vec<lsp::Location> {
    let Some(state) = ws.asm_state(uri) else {
        return Vec::new();
    };
    let Some(doc) = ws.docs.get(uri) else {
        return Vec::new();
    };
    let text = doc.text();
    let Some((symbol, _)) = symbol_at(&state, text.offset(position)) else {
        return Vec::new();
    };
    symbol_spans(symbol)
        .into_iter()
        .filter(|(_, is_def)| include_declaration || !is_def)
        .map(|(span, _)| lsp::Location::new(uri.clone(), text.range(span)))
        .collect()
}

pub fn highlights(
    ws: &mut Workspace,
    uri: &Url,
    position: lsp::Position,
) -> Vec<lsp::DocumentHighlight> {
    let Some(state) = ws.asm_state(uri) else {
        return Vec::new();
    };
    let Some(doc) = ws.docs.get(uri) else {
        return Vec::new();
    };
    let text = doc.text();
    let Some((symbol, _)) = symbol_at(&state, text.offset(position)) else {
        return Vec::new();
    };
    symbol_spans(symbol)
        .into_iter()
        .map(|(span, is_def)| lsp::DocumentHighlight {
            range: text.range(span),
            kind: Some(if is_def {
                lsp::DocumentHighlightKind::WRITE
            } else {
                lsp::DocumentHighlightKind::READ
            }),
        })
        .collect()
}

pub fn prepare_rename(
    ws: &mut Workspace,
    uri: &Url,
    position: lsp::Position,
) -> Option<lsp::PrepareRenameResponse> {
    let state = ws.asm_state(uri)?;
    let doc = ws.docs.get(uri)?;
    let text = doc.text();
    let (symbol, span) = symbol_at(&state, text.offset(position))?;
    (symbol.kind != SymbolKind::Predefined)
        .then(|| lsp::PrepareRenameResponse::Range(text.range(span)))
}

/// The book's rule for symbols: letters, digits, `_ . $ :`, not starting
/// with a digit. (The official assembler accepts more, with warnings.)
pub fn is_valid_symbol(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    let ok = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '$' | ':');
    ok(first) && !first.is_ascii_digit() && chars.all(ok)
}

pub fn rename(
    ws: &mut Workspace,
    uri: &Url,
    position: lsp::Position,
    new_name: &str,
) -> Result<Option<lsp::WorkspaceEdit>, String> {
    let Some(state) = ws.asm_state(uri) else {
        return Ok(None);
    };
    let Some(doc) = ws.docs.get(uri) else {
        return Ok(None);
    };
    let text = doc.text();
    let Some((symbol, _)) = symbol_at(&state, text.offset(position)) else {
        return Ok(None);
    };
    if symbol.kind == SymbolKind::Predefined {
        return Err(format!(
            "`{}` is a predefined symbol and cannot be renamed",
            symbol.name
        ));
    }
    if !is_valid_symbol(new_name) {
        return Err(format!(
            "`{new_name}` is not a valid symbol: use letters, digits, `_`, `.`, `$` and `:`, \
             not starting with a digit"
        ));
    }
    if new_name != symbol.name {
        if isa::predefined(new_name).is_some() {
            return Err(format!("`{new_name}` is a predefined symbol"));
        }
        if let Some(other) = state.assembly.symbols.iter().find(|s| s.name == new_name) {
            let what = match other.kind {
                SymbolKind::Label => "label",
                SymbolKind::Variable => "variable",
                SymbolKind::Predefined => "predefined symbol",
            };
            return Err(format!(
                "a {what} named `{new_name}` already exists: the rename would merge them"
            ));
        }
    }
    let edits: Vec<lsp::TextEdit> = symbol_spans(symbol)
        .into_iter()
        .map(|(span, _)| lsp::TextEdit::new(text.range(span), new_name.to_string()))
        .collect();
    let mut changes = HashMap::new();
    changes.insert(uri.clone(), edits);
    Ok(Some(lsp::WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    }))
}

fn item(
    label: String,
    kind: lsp::CompletionItemKind,
    detail: String,
    range: lsp::Range,
    sort: usize,
) -> lsp::CompletionItem {
    lsp::CompletionItem {
        filter_text: Some(label.clone()),
        text_edit: Some(lsp::CompletionTextEdit::Edit(lsp::TextEdit::new(
            range,
            label.clone(),
        ))),
        label,
        kind: Some(kind),
        detail: Some(detail),
        sort_text: Some(format!("{sort:03}")),
        ..Default::default()
    }
}

pub fn completion(
    ws: &mut Workspace,
    uri: &Url,
    position: lsp::Position,
) -> Vec<lsp::CompletionItem> {
    let Some(state) = ws.asm_state(uri) else {
        return Vec::new();
    };
    let Some(doc) = ws.docs.get(uri) else {
        return Vec::new();
    };
    let text = doc.text();
    let offset = text.offset(position);
    let Some(line) = line_at(&state, offset) else {
        // An empty document has no lines: offer instructions.
        return mnemonic_items(&text, offset, offset, "");
    };
    if line.comment.is_some_and(|c| c.start < offset) {
        return Vec::new();
    }
    let Some(before) = doc.text.get(line.span.start..offset) else {
        return Vec::new();
    };
    let trimmed = before.trim_start_matches([' ', '\t']);
    let start = offset - trimmed.len();
    if let Some(operand) = trimmed.strip_prefix('@') {
        let range = text.range(Span::new(start + 1, offset));
        let _ = operand;
        let mut items = Vec::new();
        for s in &state.assembly.symbols {
            // The name being typed is itself a (new) variable: skip it.
            if s.kind == SymbolKind::Variable
                && s.references.len() == 1
                && s.references
                    .iter()
                    .all(|r| r.start <= offset && offset <= r.end)
            {
                continue;
            }
            let (kind, detail, sort) = match s.kind {
                SymbolKind::Label => (
                    lsp::CompletionItemKind::REFERENCE,
                    format!("label, ROM[{}]", s.address),
                    0,
                ),
                SymbolKind::Variable => (
                    lsp::CompletionItemKind::VARIABLE,
                    format!("variable, RAM[{}]", s.address),
                    1,
                ),
                SymbolKind::Predefined => continue,
            };
            items.push(item(s.name.clone(), kind, detail, range, sort));
        }
        for (name, value) in isa::PREDEFINED {
            if items.iter().any(|i| i.label == name) {
                continue;
            }
            items.push(item(
                name.to_string(),
                lsp::CompletionItemKind::CONSTANT,
                format!("predefined, {value}: {}", predefined_role(name)),
                range,
                2,
            ));
        }
        return items;
    }
    if trimmed.starts_with('(') {
        return Vec::new();
    }
    mnemonic_items(&text, start, offset, trimmed)
}

/// Dest, comp or jump mnemonics, depending on what precedes the cursor in
/// a C-instruction.
fn mnemonic_items(
    text: &Text<'_>,
    start: usize,
    offset: usize,
    before: &str,
) -> Vec<lsp::CompletionItem> {
    let comps = |range: lsp::Range, sort: usize| -> Vec<lsp::CompletionItem> {
        isa::COMPS
            .iter()
            .enumerate()
            .map(|(i, (m, _))| {
                item(
                    m.to_string(),
                    lsp::CompletionItemKind::OPERATOR,
                    describe_comp(m),
                    range,
                    sort + i,
                )
            })
            .collect()
    };
    if let Some(i) = before.rfind(';') {
        let range = text.range(Span::new(start + i + 1, offset));
        return isa::JUMPS
            .iter()
            .skip(1)
            .enumerate()
            .map(|(i, j)| {
                let detail = match jump_condition(j) {
                    Some(c) => format!("jump to A if the result {c}"),
                    None => "jump to A".to_string(),
                };
                item(
                    j.to_string(),
                    lsp::CompletionItemKind::KEYWORD,
                    detail,
                    range,
                    i,
                )
            })
            .collect();
    }
    if let Some(i) = before.find('=') {
        return comps(text.range(Span::new(start + i + 1, offset)), 0);
    }
    // At the start of an instruction: a destination or a computation.
    let range = text.range(Span::new(start, offset));
    let mut items: Vec<lsp::CompletionItem> = isa::DESTS
        .iter()
        .skip(1)
        .enumerate()
        .map(|(i, d)| {
            let targets = describe_c(d, "0", "");
            let mut it = item(
                format!("{d}="),
                lsp::CompletionItemKind::KEYWORD,
                format!("store into {}", targets.trim_end_matches(" = 0")),
                range,
                i,
            );
            it.filter_text = Some(d.to_string());
            it
        })
        .collect();
    items.extend(comps(range, 10));
    items
}

#[allow(deprecated)] // `DocumentSymbol::deprecated` must still be initialised.
pub fn document_symbols(ws: &mut Workspace, uri: &Url) -> Vec<lsp::DocumentSymbol> {
    let Some(state) = ws.asm_state(uri) else {
        return Vec::new();
    };
    let Some(doc) = ws.docs.get(uri) else {
        return Vec::new();
    };
    let text = doc.text();
    sections(&state)
        .into_iter()
        .map(|(name, name_span, section)| {
            let address = state
                .assembly
                .symbols
                .iter()
                .find(|s| s.name == name && s.kind == SymbolKind::Label)
                .map(|s| format!("ROM[{}]", s.address));
            lsp::DocumentSymbol {
                name,
                detail: address,
                kind: lsp::SymbolKind::FUNCTION,
                tags: None,
                deprecated: None,
                range: text.range(section),
                selection_range: text.range(name_span),
                children: None,
            }
        })
        .collect()
}

/// Each label with the stretch of lines it starts (up to the next label).
fn sections(state: &AsmState) -> Vec<(String, Span, Span)> {
    let labels: Vec<(usize, String, Span)> = state
        .parsed
        .lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| match &l.item {
            Some(Item::Label(def)) => {
                let name = def.name.as_ref()?;
                Some((i, name.text.clone(), name.span))
            }
            _ => None,
        })
        .collect();
    let lines = &state.parsed.lines;
    labels
        .iter()
        .enumerate()
        .filter_map(|(k, (i, name, span))| {
            let first = lines.get(*i)?.span;
            let end_line = labels.get(k + 1).map_or(lines.len(), |(j, ..)| *j);
            // The last non-blank line of the section.
            let last = lines
                .get(*i..end_line)?
                .iter()
                .rev()
                .find(|l| l.item.is_some() || l.comment.is_some())
                .map_or(first, |l| l.span);
            Some((name.clone(), *span, Span::new(first.start, last.end)))
        })
        .collect()
}

pub fn folding_ranges(ws: &mut Workspace, uri: &Url) -> Vec<lsp::FoldingRange> {
    let Some(state) = ws.asm_state(uri) else {
        return Vec::new();
    };
    let Some(doc) = ws.docs.get(uri) else {
        return Vec::new();
    };
    let text = doc.text();
    sections(&state)
        .into_iter()
        .filter_map(|(_, _, span)| {
            let range = text.range(span);
            (range.end.line > range.start.line).then(|| lsp::FoldingRange {
                start_line: range.start.line,
                end_line: range.end.line,
                kind: Some(lsp::FoldingRangeKind::Region),
                ..Default::default()
            })
        })
        .collect()
}
