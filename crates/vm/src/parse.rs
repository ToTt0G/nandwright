//! Front-end for the stack-machine VM language (projects 7 and 8).
//!
//! The parser accepts exactly what the official Java VM emulator loads and
//! reports everything it would refuse as an error. Things the Java emulator
//! loads but that fail (or silently do nothing) when executed are reported as
//! warnings, so that "has errors" means "the Java emulator would refuse it".

use n2t_core::{Diagnostic, DiagnosticCode, Span};

/// A memory segment named in a `push` or `pop` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Segment {
    Argument,
    Local,
    Static,
    Constant,
    This,
    That,
    Pointer,
    Temp,
}

impl Segment {
    pub const ALL: [Segment; 8] = [
        Segment::Argument,
        Segment::Local,
        Segment::Static,
        Segment::Constant,
        Segment::This,
        Segment::That,
        Segment::Pointer,
        Segment::Temp,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Segment::Argument => "argument",
            Segment::Local => "local",
            Segment::Static => "static",
            Segment::Constant => "constant",
            Segment::This => "this",
            Segment::That => "that",
            Segment::Pointer => "pointer",
            Segment::Temp => "temp",
        }
    }

    pub fn from_name(name: &str) -> Option<Segment> {
        Segment::ALL.into_iter().find(|s| s.name() == name)
    }
}

/// An arithmetic or logical command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArithOp {
    Add,
    Sub,
    Neg,
    Eq,
    Gt,
    Lt,
    And,
    Or,
    Not,
}

impl ArithOp {
    pub const ALL: [ArithOp; 9] = [
        ArithOp::Add,
        ArithOp::Sub,
        ArithOp::Neg,
        ArithOp::Eq,
        ArithOp::Gt,
        ArithOp::Lt,
        ArithOp::And,
        ArithOp::Or,
        ArithOp::Not,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ArithOp::Add => "add",
            ArithOp::Sub => "sub",
            ArithOp::Neg => "neg",
            ArithOp::Eq => "eq",
            ArithOp::Gt => "gt",
            ArithOp::Lt => "lt",
            ArithOp::And => "and",
            ArithOp::Or => "or",
            ArithOp::Not => "not",
        }
    }

    pub fn from_name(name: &str) -> Option<ArithOp> {
        ArithOp::ALL.into_iter().find(|s| s.name() == name)
    }

    /// Whether the command pops one operand (otherwise two).
    pub fn is_unary(self) -> bool {
        matches!(self, ArithOp::Neg | ArithOp::Not)
    }
}

/// A name (function or label) together with where it was written.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Name {
    pub text: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandKind {
    Push {
        segment: Segment,
        segment_span: Span,
        index: u16,
        index_span: Span,
    },
    Pop {
        segment: Segment,
        segment_span: Span,
        index: u16,
        index_span: Span,
    },
    Arithmetic(ArithOp),
    Label(Name),
    Goto(Name),
    IfGoto(Name),
    Function {
        name: Name,
        n_locals: u16,
        n_locals_span: Span,
    },
    Call {
        name: Name,
        n_args: u16,
        n_args_span: Span,
    },
    Return,
}

/// One VM command. Every command, labels included, occupies one slot of the
/// program, exactly as in the Java emulator (return addresses count labels).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub kind: CommandKind,
    /// From the first to the last token of the command.
    pub span: Span,
    /// The span of the command keyword (`push`, `call`, ...).
    pub keyword_span: Span,
    /// Zero-based source line of the command.
    pub line: u32,
    /// Index of the enclosing `function` command in [`ParsedVm::commands`],
    /// or `None` for commands before the first `function` of the file.
    pub function: Option<u32>,
}

/// The result of [`parse`]: commands of the lines that parsed, plus diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedVm {
    /// The file name without directory and `.vm` extension (the class name).
    pub file_stem: String,
    pub commands: Vec<Command>,
    pub diagnostics: Vec<Diagnostic>,
}

impl ParsedVm {
    /// Whether the Java emulator would refuse to load this file.
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(Diagnostic::is_error)
    }

    /// The `function` commands of the file: (command index, name, local count).
    pub fn functions(&self) -> impl Iterator<Item = (usize, &Name, u16)> + '_ {
        self.commands
            .iter()
            .enumerate()
            .filter_map(|(i, c)| match &c.kind {
                CommandKind::Function { name, n_locals, .. } => Some((i, name, *n_locals)),
                _ => None,
            })
    }

    /// The `label` commands of the file: (command index, enclosing function name, label).
    pub fn labels(&self) -> impl Iterator<Item = (usize, Option<&Name>, &Name)> + '_ {
        self.commands
            .iter()
            .enumerate()
            .filter_map(|(i, c)| match &c.kind {
                CommandKind::Label(label) => Some((i, self.function_name(c), label)),
                _ => None,
            })
    }

    /// The name of the function a command belongs to.
    pub fn function_name(&self, command: &Command) -> Option<&Name> {
        let f = command.function? as usize;
        match &self.commands.get(f)?.kind {
            CommandKind::Function { name, .. } => Some(name),
            _ => None,
        }
    }

    /// Finds the label a `goto`/`if-goto` at command index `from` jumps to:
    /// the last label of that name in the same function (the Java emulator
    /// lets a later duplicate win).
    pub fn resolve_label(&self, from: usize, label: &str) -> Option<usize> {
        let scope = self.commands.get(from)?.function;
        self.commands
            .iter()
            .enumerate()
            .filter(|(_, c)| c.function == scope)
            .filter(|(_, c)| matches!(&c.kind, CommandKind::Label(n) if n.text == label))
            .map(|(i, _)| i)
            .next_back()
    }

    /// The command whose span contains `offset`, if any.
    pub fn command_at(&self, offset: usize) -> Option<usize> {
        self.commands
            .iter()
            .position(|c| c.span.start <= offset && offset <= c.span.end)
    }
}

fn code(s: &'static str) -> DiagnosticCode {
    DiagnosticCode::Other(s)
}

/// A whitespace-separated token of one line.
#[derive(Debug, Clone, Copy)]
struct Token<'a> {
    text: &'a str,
    span: Span,
}

/// Splits the source into lines of tokens, removing `//` and `/* */`
/// comments (block comments may span lines, as in the Java emulator).
fn tokenize<'a>(source: &'a str, diagnostics: &mut Vec<Diagnostic>) -> Vec<(u32, Vec<Token<'a>>)> {
    let bytes = source.as_bytes();
    let mut lines = Vec::new();
    let mut tokens: Vec<Token<'a>> = Vec::new();
    let mut line: u32 = 0;
    let mut i = if source.starts_with('\u{FEFF}') { 3 } else { 0 };
    let mut token_start: Option<usize> = None;
    let mut block_start: Option<usize> = None;
    let flush = |tokens: &mut Vec<Token<'a>>, start: &mut Option<usize>, end: usize| {
        if let Some(s) = start.take() {
            tokens.push(Token {
                text: &source[s..end],
                span: Span::new(s, end),
            });
        }
    };
    while i < bytes.len() {
        let b = bytes[i];
        if block_start.is_some() {
            if b == b'*' && bytes.get(i + 1) == Some(&b'/') {
                block_start = None;
                i += 2;
                continue;
            }
            if b == b'\n' {
                lines.push((line, std::mem::take(&mut tokens)));
                line += 1;
            }
            i += 1;
            continue;
        }
        match b {
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                flush(&mut tokens, &mut token_start, i);
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                flush(&mut tokens, &mut token_start, i);
                block_start = Some(i);
                i += 2;
            }
            b'\n' => {
                flush(&mut tokens, &mut token_start, i);
                lines.push((line, std::mem::take(&mut tokens)));
                line += 1;
                i += 1;
            }
            b' ' | b'\t' | b'\r' | 0x0b | 0x0c => {
                flush(&mut tokens, &mut token_start, i);
                i += 1;
            }
            _ => {
                if token_start.is_none() {
                    token_start = Some(i);
                }
                // Advance by a whole UTF-8 character so spans stay on boundaries.
                i += utf8_len(b);
            }
        }
    }
    let end = bytes.len();
    flush(&mut tokens, &mut token_start, end.min(source.len()));
    lines.push((line, tokens));
    if let Some(start) = block_start {
        diagnostics.push(Diagnostic::warning(
            code("vm-unterminated-comment"),
            "this `/*` comment is never closed; the rest of the file is ignored",
            Span::new(start, (start + 2).min(source.len())),
        ));
    }
    lines
}

fn utf8_len(first: u8) -> usize {
    match first {
        0xF0..=0xFF => 4,
        0xE0..=0xEF => 3,
        0xC0..=0xDF => 2,
        _ => 1,
    }
}

/// Parses a non-negative 16-bit number the way the Java emulator does:
/// decimal digits with an optional leading `+`, at most 32767.
fn parse_number(token: &Token, what: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<u16> {
    let text = token.text;
    let digits = text.strip_prefix('+').unwrap_or(text);
    if let Some(neg) = text.strip_prefix('-')
        && !neg.is_empty()
        && neg.bytes().all(|b| b.is_ascii_digit())
    {
        diagnostics.push(Diagnostic::error(
            code("vm-negative-number"),
            format!("{what} cannot be negative (`{text}`)"),
            token.span,
        ));
        return None;
    }
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        diagnostics.push(Diagnostic::error(
            code("vm-invalid-number"),
            format!("{what} must be a decimal number from 0 to 32767, not `{text}`"),
            token.span,
        ));
        return None;
    }
    let trimmed = digits.trim_start_matches('0');
    if trimmed.len() > 5 {
        diagnostics.push(Diagnostic::error(
            code("vm-number-too-large"),
            format!("{what} `{text}` does not fit in 15 bits (the maximum is 32767)"),
            token.span,
        ));
        return None;
    }
    let value: u32 = trimmed.parse().unwrap_or(0);
    if value > 32767 {
        diagnostics.push(Diagnostic::error(
            code("vm-number-too-large"),
            format!("{what} `{text}` does not fit in 15 bits (the maximum is 32767)"),
            token.span,
        ));
        return None;
    }
    Some(value as u16)
}

fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
}

/// Parses one `.vm` file. `file_stem` is the file name without directory and
/// extension; it is the class name the file's functions must start with.
///
/// Never panics. Each line is parsed independently; a line with an error
/// produces a diagnostic and no command.
pub fn parse(file_stem: &str, source: &str) -> ParsedVm {
    let mut diagnostics = Vec::new();
    let lines = tokenize(source, &mut diagnostics);
    let mut commands: Vec<Command> = Vec::new();
    let mut current_function: Option<u32> = None;

    for (line, tokens) in lines {
        let Some(first) = tokens.first() else {
            continue;
        };
        let last = tokens.last().unwrap_or(first);
        let span = Span::new(first.span.start, last.span.end);
        let args = &tokens[1..];
        let missing = |diags: &mut Vec<Diagnostic>, what: &str| {
            diags.push(Diagnostic::error(
                code("vm-missing-argument"),
                format!("`{}` needs {what}", first.text),
                Span::empty(last.span.end),
            ));
        };
        let too_many = |diags: &mut Vec<Diagnostic>, expected: usize| {
            let extra = Span::new(tokens[expected + 1].span.start, last.span.end);
            diags.push(Diagnostic::error(
                code("vm-too-many-arguments"),
                format!(
                    "`{}` takes {expected} argument{}; remove the extra text",
                    first.text,
                    if expected == 1 { "" } else { "s" }
                ),
                extra,
            ));
        };

        let kind = match first.text {
            "push" | "pop" => {
                if args.len() < 2 {
                    missing(&mut diagnostics, "a segment and an index");
                    continue;
                }
                if args.len() > 2 {
                    too_many(&mut diagnostics, 2);
                    continue;
                }
                let Some(segment) = Segment::from_name(args[0].text) else {
                    diagnostics.push(Diagnostic::error(
                        code("vm-unknown-segment"),
                        format!(
                            "unknown memory segment `{}`; expected one of argument, local, static, constant, this, that, pointer, temp",
                            args[0].text
                        ),
                        args[0].span,
                    ));
                    continue;
                };
                let Some(index) = parse_number(&args[1], "the index", &mut diagnostics) else {
                    continue;
                };
                let is_push = first.text == "push";
                match segment {
                    Segment::Constant if !is_push => {
                        diagnostics.push(Diagnostic::error(
                            code("vm-pop-constant"),
                            "`pop constant` is not a valid command: the constant segment cannot be written",
                            span,
                        ));
                        continue;
                    }
                    Segment::Temp if index > 7 => diagnostics.push(Diagnostic::warning(
                        code("vm-index-out-of-range"),
                        format!("the temp segment has 8 entries (0 to 7); `temp {index}` fails when executed"),
                        args[1].span,
                    )),
                    Segment::Pointer if index > 1 => diagnostics.push(Diagnostic::warning(
                        code("vm-index-out-of-range"),
                        format!("the pointer segment has 2 entries (0 and 1); the Java emulator ignores `pointer {index}` entirely"),
                        args[1].span,
                    )),
                    Segment::Static if index > 238 => diagnostics.push(Diagnostic::warning(
                        code("vm-index-out-of-range"),
                        format!("`static {index}` is beyond the 239 static variables (RAM[16..=254]) that fit below the stack"),
                        args[1].span,
                    )),
                    _ => {}
                }
                if is_push {
                    CommandKind::Push {
                        segment,
                        segment_span: args[0].span,
                        index,
                        index_span: args[1].span,
                    }
                } else {
                    CommandKind::Pop {
                        segment,
                        segment_span: args[0].span,
                        index,
                        index_span: args[1].span,
                    }
                }
            }
            "label" | "goto" | "if-goto" => {
                if args.is_empty() {
                    missing(&mut diagnostics, "a label name");
                    continue;
                }
                if args.len() > 1 {
                    too_many(&mut diagnostics, 1);
                    continue;
                }
                let name = Name {
                    text: args[0].text.to_string(),
                    span: args[0].span,
                };
                match first.text {
                    "label" => CommandKind::Label(name),
                    "goto" => CommandKind::Goto(name),
                    _ => CommandKind::IfGoto(name),
                }
            }
            "function" | "call" => {
                let is_function = first.text == "function";
                if args.len() < 2 {
                    missing(
                        &mut diagnostics,
                        if is_function {
                            "a function name and a number of local variables"
                        } else {
                            "a function name and a number of arguments"
                        },
                    );
                    continue;
                }
                if args.len() > 2 {
                    too_many(&mut diagnostics, 2);
                    continue;
                }
                let name = Name {
                    text: args[0].text.to_string(),
                    span: args[0].span,
                };
                let what = if is_function {
                    "the number of local variables"
                } else {
                    "the number of arguments"
                };
                let Some(count) = parse_number(&args[1], what, &mut diagnostics) else {
                    continue;
                };
                if !is_valid_name(&name.text) {
                    continue;
                }
                if is_function {
                    match name.text.split_once('.') {
                        None => diagnostics.push(Diagnostic::warning(
                            code("vm-invalid-function-name"),
                            format!(
                                "function names have the form Class.name; the Java emulator stops with an error when it reaches `function {}`",
                                name.text
                            ),
                            name.span,
                        )),
                        Some((class, _)) if class != file_stem => {
                            diagnostics.push(Diagnostic::warning(
                                code("vm-function-class-mismatch"),
                                format!(
                                    "`{}` is in {file_stem}.vm, but its class part is `{class}`; the Java emulator stops with an error when it reaches this function",
                                    name.text
                                ),
                                name.span,
                            ))
                        }
                        _ => {}
                    }
                    CommandKind::Function {
                        name,
                        n_locals: count,
                        n_locals_span: args[1].span,
                    }
                } else {
                    if !name.text.contains('.') {
                        diagnostics.push(Diagnostic::error(
                            code("vm-invalid-function-name"),
                            format!(
                                "`call {}`: function names have the form Class.name",
                                name.text
                            ),
                            name.span,
                        ));
                        continue;
                    }
                    CommandKind::Call {
                        name,
                        n_args: count,
                        n_args_span: args[1].span,
                    }
                }
            }
            "return" => {
                if !args.is_empty() {
                    extra_ignored(&mut diagnostics, first.text, args);
                }
                CommandKind::Return
            }
            other => match ArithOp::from_name(other) {
                Some(op) => {
                    if !args.is_empty() {
                        extra_ignored(&mut diagnostics, first.text, args);
                    }
                    CommandKind::Arithmetic(op)
                }
                None => {
                    let hint = if ArithOp::from_name(&other.to_ascii_lowercase()).is_some()
                        || matches!(
                            other.to_ascii_lowercase().as_str(),
                            "push"
                                | "pop"
                                | "label"
                                | "goto"
                                | "if-goto"
                                | "function"
                                | "call"
                                | "return"
                        ) {
                        " (commands are lower case)"
                    } else {
                        ""
                    };
                    diagnostics.push(Diagnostic::error(
                        code("vm-unknown-command"),
                        format!("unknown command `{other}`{hint}"),
                        first.span,
                    ));
                    continue;
                }
            },
        };

        if matches!(kind, CommandKind::Function { .. }) {
            current_function = Some(commands.len() as u32);
        }
        commands.push(Command {
            kind,
            span,
            keyword_span: first.span,
            line,
            function: current_function,
        });
    }

    let mut parsed = ParsedVm {
        file_stem: file_stem.to_string(),
        commands,
        diagnostics,
    };
    check_labels(&mut parsed);
    parsed
}

fn extra_ignored(diagnostics: &mut Vec<Diagnostic>, command: &str, args: &[Token]) {
    let (Some(a), Some(b)) = (args.first(), args.last()) else {
        return;
    };
    diagnostics.push(Diagnostic::warning(
        code("vm-extra-arguments"),
        format!("`{command}` takes no arguments; the extra text is ignored"),
        Span::new(a.span.start, b.span.end),
    ));
}

/// Reports duplicate labels (warning: the Java emulator lets the last one win)
/// and jumps to labels that do not exist in the same function (error).
fn check_labels(parsed: &mut ParsedVm) {
    use std::collections::HashMap;
    let mut seen: HashMap<(Option<u32>, &str), Span> = HashMap::new();
    let mut diags = Vec::new();
    for c in &parsed.commands {
        if let CommandKind::Label(name) = &c.kind
            && let Some(prev) = seen.insert((c.function, name.text.as_str()), name.span)
        {
            diags.push(
                Diagnostic::warning(
                    code("vm-duplicate-label"),
                    format!(
                        "label `{}` is defined more than once in this function; jumps go to the last definition",
                        name.text
                    ),
                    name.span,
                )
                .with_related_span("first defined here", prev),
            );
        }
    }
    for c in &parsed.commands {
        if let CommandKind::Goto(name) | CommandKind::IfGoto(name) = &c.kind
            && !seen.contains_key(&(c.function, name.text.as_str()))
        {
            let scope = match c.function.and_then(|f| parsed.commands.get(f as usize)) {
                Some(Command {
                    kind: CommandKind::Function { name: f, .. },
                    ..
                }) => format!("function {}", f.text),
                _ => "the code before the first function".to_string(),
            };
            diags.push(Diagnostic::error(
                code("vm-unknown-label"),
                format!("label `{}` is not defined in {scope}", name.text),
                name.span,
            ));
        }
    }
    parsed.diagnostics.extend(diags);
}
