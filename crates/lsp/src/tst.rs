//! Language features for `.tst` documents.

use lsp_types as lsp;
use lsp_types::Url;
use n2t_core::builtins::{BuiltinLibrary, StateShape, WritePolicy};
use n2t_core::hdl::definition::NetKind;
use n2t_core::hdl::interface::PinDirection;
use n2t_core::host::path;
use n2t_core::tst::Dialect;
use n2t_core::tst::ast::{ArgumentKind, Command, Operand, Script, Statement, Variable};
use n2t_core::tst::emulator::{self, EmulatorVariable};
use n2t_core::tst::lexer::{Token, TokenKind};
use n2t_core::tst::semantic::{LoadInfo, PinRole, ScriptTarget, VariableTarget};

use crate::hdl::{interface_markdown, pin_text};
use crate::workspace::Workspace;

const COMMAND_DOCS: &[(&str, &str, &str)] = &[
    (
        "load",
        "load ${1:Chip}.hdl",
        "Loads what the script tests: a chip (`X.hdl`, from the script's folder or the \
         built-in library), a program for the CPU Emulator (`X.asm` or `X.hack`), or VM code \
         for the VM Emulator (`X.vm`, a folder, or nothing for the script's folder).",
    ),
    (
        "output-file",
        "output-file ${1:Chip}.out",
        "Creates the output file that `output` writes to.",
    ),
    (
        "compare-to",
        "compare-to ${1:Chip}.cmp",
        "Compares every output line with the given expected-results file.",
    ),
    (
        "output-list",
        "output-list ${1:in%B1.1.1}",
        "Sets the output schema (whitespace-separated `name%Fpad.len.pad` items) and writes its header.",
    ),
    (
        "set",
        "set ${1:pin} ${2:0}",
        "Sets an input pin or built-in state value. Does not evaluate.",
    ),
    ("eval", "eval", "Evaluates the chip's combinational logic."),
    (
        "output",
        "output",
        "Writes one row of the output schema's current values.",
    ),
    (
        "tick",
        "tick",
        "First half of a clock cycle: clocked parts capture their inputs.",
    ),
    (
        "tock",
        "tock",
        "Second half of a clock cycle: captured state becomes visible.",
    ),
    (
        "ticktock",
        "ticktock",
        "CPU Emulator: executes one instruction (a full clock cycle).",
    ),
    ("vmstep", "vmstep", "VM Emulator: executes one VM command."),
    ("echo", "echo \"${1}\"", "Shows a status message."),
    ("clear-echo", "clear-echo", "Clears the status message."),
    (
        "breakpoint",
        "breakpoint ${1:pin} ${2:0}",
        "Pauses when the variable equals the value.",
    ),
    (
        "clear-breakpoints",
        "clear-breakpoints",
        "Removes all breakpoints.",
    ),
    (
        "repeat",
        "repeat ${1:n} {\n    $0\n}",
        "Repeats the body `n` times (forever without a count).",
    ),
    (
        "while",
        "while ${1:pin} ${2:<>} ${3:0} {\n    $0\n}",
        "Repeats the body while the condition holds.",
    ),
];

fn markdown(value: String, range: Option<lsp::Range>) -> lsp::Hover {
    lsp::Hover {
        contents: lsp::HoverContents::Markup(lsp::MarkupContent {
            kind: lsp::MarkupKind::Markdown,
            value,
        }),
        range,
    }
}

/// The simulator a command belongs to (`None`: every simulator).
fn command_dialect(name: &str) -> Option<Dialect> {
    match name {
        "eval" | "tick" | "tock" => Some(Dialect::Hardware),
        "ticktock" => Some(Dialect::Cpu),
        "vmstep" => Some(Dialect::Vm),
        _ => None,
    }
}

/// The script's dialect, if anything in it decides it (a `load` of a known
/// file type or a stepping command); `None` while it is still open.
fn known_dialect(script: &Script) -> Option<Dialect> {
    let decided = script.simple_statements().any(|s| match &s.command {
        // A `load` decides once its file has a known extension (a bare or
        // half-typed name does not).
        Command::Load(f) => f.extension().is_some_and(|e| {
            ["hdl", "asm", "hack", "vm"]
                .iter()
                .any(|k| e.eq_ignore_ascii_case(k))
        }),
        Command::Eval
        | Command::Tick
        | Command::Tock
        | Command::TickTock
        | Command::VmStep
        | Command::BuiltinMethod { .. } => true,
        _ => false,
    });
    decided.then(|| script.dialect())
}

/// The CPU or VM emulator variable written at `offset`.
fn emulator_variable_at(script: &Script, offset: usize) -> Option<Variable> {
    let mut found = None;
    let mut check = |v: &Variable| {
        if v.span.start <= offset && offset <= v.span.end {
            found = Some(v.clone());
        }
    };
    for statement in &script.statements {
        if let Statement::While(w) = statement {
            for operand in [&w.condition.left, &w.condition.right] {
                if let Operand::Variable(v) = operand {
                    check(v);
                }
            }
        }
    }
    for statement in script.simple_statements() {
        match &statement.command {
            Command::Set { variable, .. } | Command::Breakpoint { variable, .. } => check(variable),
            Command::OutputList(items) => items.iter().for_each(|i| check(&i.variable)),
            _ => {}
        }
    }
    found
}

fn describe_emulator_variable(dialect: Dialect, v: &EmulatorVariable) -> String {
    let written = if v.indexed && !v.plain {
        format!("{}[i]", v.name)
    } else if v.indexed {
        format!("{0} or {0}[i]", v.name)
    } else {
        v.name.to_string()
    };
    let access = if v.writable { "writable" } else { "read-only" };
    let kind = if v.text {
        ", text (output with `%S`)"
    } else {
        ""
    };
    format!(
        "`{written}` — {} ({}: {access}{kind})",
        v.description,
        dialect.tool()
    )
}

fn emulator_variable_items(dialect: Dialect, writable_only: bool) -> Vec<lsp::CompletionItem> {
    emulator::variables(dialect)
        .iter()
        .filter(|v| !writable_only || v.writable)
        .flat_map(|v| {
            let mut labels = Vec::new();
            if v.plain {
                labels.push(v.name.to_string());
            }
            if v.indexed {
                labels.push(format!("{}[0]", v.name));
            }
            labels.into_iter().map(move |label| lsp::CompletionItem {
                label,
                kind: Some(lsp::CompletionItemKind::VARIABLE),
                detail: Some(v.description.to_string()),
                ..Default::default()
            })
        })
        .collect()
}

/// The file named at `offset`: `load X`, `output-file X`, `compare-to X`,
/// or a file argument of a built-in chip method (`ROM32K load X.hack`).
fn file_name_at(script: &Script, offset: usize) -> Option<(String, n2t_core::Span)> {
    let inside = |s: n2t_core::Span| s.start <= offset && offset <= s.end;
    for statement in script.simple_statements() {
        match &statement.command {
            Command::Load(f) | Command::OutputFile(f) | Command::CompareTo(f)
                if inside(f.span) && !f.name.is_empty() =>
            {
                return Some((f.name.clone(), f.span));
            }
            Command::BuiltinMethod { arguments, .. } => {
                for a in arguments {
                    if let ArgumentKind::Word(w) | ArgumentKind::String(w) = &a.kind
                        && inside(a.span)
                        && w.contains('.')
                    {
                        return Some((w.clone(), a.span));
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn describe_variable(target: &VariableTarget) -> String {
    match target {
        VariableTarget::Time => {
            "`time` — the simulated clock as text (`0`, `0+`, `1`, …). Read-only; output with `%S`."
                .into()
        }
        VariableTarget::Pin {
            chip,
            name,
            role,
            width,
        } => {
            let role = match role {
                PinRole::Input => "input pin (writable)",
                PinRole::Output => "output pin (read-only)",
                PinRole::Internal => "internal pin (read-only)",
            };
            format!(
                "`{name}` — {role} of `{chip}`, {width} bit{}",
                if *width == 1 { "" } else { "s" }
            )
        }
        VariableTarget::BuiltinState { chip, width, write } => {
            let access = match write {
                WritePolicy::ReadOnly => "read-only",
                WritePolicy::Immediate => "writable (takes effect immediately)",
                WritePolicy::NextTock => "writable (takes effect at the next tock)",
            };
            format!("state of built-in `{chip}` — {width}-bit, {access}")
        }
    }
}

pub fn hover(ws: &mut Workspace, uri: &Url, position: lsp::Position) -> Option<lsp::Hover> {
    let dir = ws.docs.get(uri)?.dir.clone();
    let offset = ws.docs.get(uri)?.text().offset(position);
    let (reference, keyword, emulator_var) = {
        let (parsed, analysis) = ws.tst_state(uri)?;
        let dialect = parsed.script.dialect();
        let emulator_var = (dialect != Dialect::Hardware)
            .then(|| emulator_variable_at(&parsed.script, offset))
            .flatten()
            .map(|v| (dialect, v));
        let reference = analysis.reference_at(offset).cloned();
        let keyword = parsed
            .script
            .simple_statements()
            .map(|s| &s.keyword)
            .find(|k| k.span.contains(offset))
            .cloned()
            .or_else(|| word_at(&parsed.tokens, offset));
        (reference, keyword, emulator_var)
    };
    let text = ws.docs.get(uri)?.text();
    if let Some((dialect, variable)) = emulator_var {
        let value = match emulator::lookup(dialect, &variable) {
            Some(known) => describe_emulator_variable(dialect, known),
            None => format!(
                "the {} has no variable `{}`",
                dialect.tool(),
                variable.name.name
            ),
        };
        return Some(markdown(value, Some(text.range(variable.span))));
    }
    if let Some(reference) = reference {
        let range = Some(text.range(reference.span));
        let value = match &reference.target {
            ScriptTarget::Chip(name) => {
                let unit = ws.project(&dir).elaborator.compile(name);
                let interface = unit.interface.clone()?;
                interface_markdown(&interface, Some(&unit))
            }
            ScriptTarget::Variable(target) => describe_variable(target),
        };
        return Some(markdown(value, range));
    }
    let keyword = keyword?;
    let lower = keyword.name.to_ascii_lowercase();
    let (_, _, doc) = COMMAND_DOCS.iter().find(|(c, ..)| *c == lower)?;
    Some(markdown(
        format!("**{lower}** — {doc}"),
        Some(text.range(keyword.span)),
    ))
}

fn word_at(tokens: &[Token], offset: usize) -> Option<n2t_core::tst::ast::Ident> {
    tokens.iter().find_map(|t| match &t.kind {
        TokenKind::Word(w) if t.span.contains(offset) => Some(n2t_core::tst::ast::Ident {
            name: w.clone(),
            span: t.span,
        }),
        _ => None,
    })
}

pub fn definition(ws: &mut Workspace, uri: &Url, position: lsp::Position) -> Option<lsp::Location> {
    let dir = ws.docs.get(uri)?.dir.clone();
    let offset = ws.docs.get(uri)?.text().offset(position);
    let (parsed, analysis) = ws.tst_state(uri)?;
    let reference = analysis.reference_at(offset).cloned();
    let file = file_name_at(&parsed.script, offset);
    let Some(reference) = reference else {
        let (name, _) = file?;
        return file_location(ws, &dir, &name);
    };
    match reference.target {
        ScriptTarget::Chip(name) => {
            let unit = ws.project(&dir).elaborator.compile(&name);
            let span = unit.interface.as_ref()?.name_span?;
            ws.unit_location(&unit, span)
        }
        ScriptTarget::Variable(VariableTarget::Pin {
            chip, name, role, ..
        }) => {
            let unit = ws.project(&dir).elaborator.compile(&chip);
            let span = match role {
                PinRole::Input => unit.interface.as_ref()?.input(&name)?.1.span?,
                PinRole::Output => unit.interface.as_ref()?.output(&name)?.1.span?,
                PinRole::Internal => unit.analysis.as_ref()?.signal(&name)?.1.declaration,
            };
            ws.unit_location(&unit, span)
        }
        ScriptTarget::Variable(VariableTarget::BuiltinState { chip, .. }) => {
            let unit = ws.project(&dir).elaborator.compile(&chip);
            let span = unit.interface.as_ref()?.name_span?;
            ws.unit_location(&unit, span)
        }
        ScriptTarget::Variable(VariableTarget::Time) => None,
    }
}

/// The start of a file named relative to a script's folder, if it exists
/// (as an open document or on disk).
fn file_location(ws: &Workspace, dir: &str, name: &str) -> Option<lsp::Location> {
    let file = path::join(dir, name);
    let start = lsp::Range::default();
    if let Some(doc) = ws.docs.values().find(|d| d.path.as_deref() == Some(&file)) {
        return Some(lsp::Location::new(doc.uri.clone(), start));
    }
    ws.host.read(&file).ok()?;
    Some(lsp::Location::new(
        crate::convert::path_to_url(&file)?,
        start,
    ))
}

fn variable_items(
    library: &BuiltinLibrary,
    load: Option<&LoadInfo>,
    writable_only: bool,
    include_time: bool,
) -> Vec<lsp::CompletionItem> {
    let mut items = Vec::new();
    let push = |items: &mut Vec<lsp::CompletionItem>, label: String, detail: String, kind| {
        items.push(lsp::CompletionItem {
            label,
            kind: Some(kind),
            detail: Some(detail),
            ..Default::default()
        });
    };
    if include_time {
        push(
            &mut items,
            "time".into(),
            "simulated time (text)".into(),
            lsp::CompletionItemKind::CONSTANT,
        );
    }
    let Some(load) = load else { return items };
    if let Some(interface) = &load.interface {
        for pin in &interface.inputs {
            push(
                &mut items,
                pin.name.clone(),
                pin_text(PinDirection::Input, pin),
                lsp::CompletionItemKind::FIELD,
            );
        }
        if !writable_only {
            for pin in &interface.outputs {
                push(
                    &mut items,
                    pin.name.clone(),
                    pin_text(PinDirection::Output, pin),
                    lsp::CompletionItemKind::FIELD,
                );
            }
        }
    }
    if let Some(definition) = &load.definition {
        if !writable_only && let Some(composite) = definition.as_composite() {
            for net in composite
                .nets
                .iter()
                .filter(|n| n.kind == NetKind::Internal)
            {
                push(
                    &mut items,
                    net.name.clone(),
                    format!("internal pin ({} bits)", net.width),
                    lsp::CompletionItemKind::VARIABLE,
                );
            }
        }
        let mut seen = Vec::new();
        definition.visit_instances(&mut |def, _| {
            if let Some(b) = def.as_builtin()
                && let Some(chip) = library.by_symbol(&b.symbol)
                && let Some(state) = &chip.script_state
                && (!writable_only || state.write != WritePolicy::ReadOnly)
                && !seen.contains(&chip.name)
            {
                seen.push(chip.name.clone());
                let label = match state.shape {
                    StateShape::Scalar => format!("{}[]", chip.name),
                    StateShape::Indexed { .. } => format!("{}[0]", chip.name),
                };
                push(
                    &mut items,
                    label,
                    format!("state of built-in {}", chip.name),
                    lsp::CompletionItemKind::PROPERTY,
                );
            }
        });
    }
    items
}

pub fn completion(
    ws: &mut Workspace,
    uri: &Url,
    position: lsp::Position,
    snippets: bool,
) -> Vec<lsp::CompletionItem> {
    let Some(doc) = ws.docs.get(uri) else {
        return Vec::new();
    };
    let (dir, stem) = (doc.dir.clone(), doc.stem.clone());
    let text = doc.text.clone();
    let offset = doc.text().offset(position);
    let library = ws.builtins.clone();
    let Some((parsed, analysis)) = ws.tst_state(uri) else {
        return Vec::new();
    };

    for t in parsed.tokens.iter().filter(|t| t.is_trivia()) {
        let is_line = text[t.span.start..].starts_with("//");
        if t.span.start < offset && (offset < t.span.end || (is_line && offset == t.span.end)) {
            return Vec::new();
        }
    }

    let mut before: Vec<&Token> = parsed
        .tokens
        .iter()
        .filter(|t| !t.is_trivia() && t.kind != TokenKind::Eof && t.span.end <= offset)
        .collect();
    if before
        .last()
        .is_some_and(|t| t.span.end == offset && matches!(t.kind, TokenKind::Word(_)))
    {
        before.pop();
    }
    let start = before
        .iter()
        .rposition(|t| {
            matches!(
                t.kind,
                TokenKind::Comma
                    | TokenKind::Semicolon
                    | TokenKind::Bang
                    | TokenKind::LeftBrace
                    | TokenKind::RightBrace
            )
        })
        .map_or(0, |i| i + 1);
    let statement = &before[start..];
    let load = analysis.loaded_chip_at(offset).cloned();
    let command = match statement.first().map(|t| &t.kind) {
        None => None,
        Some(TokenKind::Word(w)) => Some(w.to_ascii_lowercase()),
        Some(_) => return Vec::new(),
    };
    let statement_len = statement.len();
    let dialect = known_dialect(&parsed.script);

    let Some(command) = command else {
        return COMMAND_DOCS
            .iter()
            .filter(|(name, ..)| {
                dialect.is_none() || command_dialect(name).is_none_or(|d| Some(d) == dialect)
            })
            .map(|(name, snippet, doc)| lsp::CompletionItem {
                label: name.to_string(),
                kind: Some(lsp::CompletionItemKind::KEYWORD),
                documentation: Some(lsp::Documentation::String(doc.to_string())),
                insert_text: Some(if snippets {
                    snippet.to_string()
                } else {
                    name.to_string()
                }),
                insert_text_format: Some(if snippets {
                    lsp::InsertTextFormat::SNIPPET
                } else {
                    lsp::InsertTextFormat::PLAIN_TEXT
                }),
                ..Default::default()
            })
            .collect();
    };
    let file_item = |name: String| lsp::CompletionItem {
        label: name,
        kind: Some(lsp::CompletionItemKind::FILE),
        ..Default::default()
    };
    if let Some(d @ (Dialect::Cpu | Dialect::Vm)) = dialect {
        match (command.as_str(), statement_len) {
            ("set", 1) => return emulator_variable_items(d, true),
            ("breakpoint" | "while", 1) | ("output-list", _) => {
                return emulator_variable_items(d, false);
            }
            _ => {}
        }
    }
    match (command.as_str(), statement_len) {
        ("load", 1) => {
            let mut items = Vec::new();
            if matches!(dialect, None | Some(Dialect::Hardware)) {
                items.extend(
                    ws.project(&dir)
                        .all_chips()
                        .into_iter()
                        .map(|c| file_item(format!("{c}.hdl"))),
                );
            }
            let wanted: &[&str] = match dialect {
                None => &["asm", "hack", "vm"],
                Some(Dialect::Cpu) => &["asm", "hack"],
                Some(Dialect::Vm) => &["vm"],
                Some(Dialect::Hardware) => &[],
            };
            let mut files: Vec<String> = ws
                .project(&dir)
                .files()
                .into_iter()
                .filter(|f| wanted.iter().any(|e| path::has_extension(f, e)))
                .collect();
            files.sort();
            items.extend(files.into_iter().map(file_item));
            items
        }
        ("output-file", 1) => vec![file_item(format!("{stem}.out"))],
        ("compare-to", 1) => {
            let mut names = vec![format!("{stem}.cmp")];
            for name in ws.project(&dir).files() {
                if name.ends_with(".cmp") && !names.contains(&name) {
                    names.push(name);
                }
            }
            names.into_iter().map(file_item).collect()
        }
        ("set", 1) => variable_items(&library, load.as_ref(), true, false),
        ("breakpoint", 1) => variable_items(&library, load.as_ref(), false, true),
        ("output-list", _) => variable_items(&library, load.as_ref(), false, true),
        ("while", 1) => variable_items(&library, load.as_ref(), false, true),
        _ => Vec::new(),
    }
}
