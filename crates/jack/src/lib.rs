//! The Jack compiler: Jack source to VM code, plus the syntax tree, tokens and
//! symbol information an editor needs.
//!
//! The compiler reproduces the official Nand to Tetris `JackCompiler`: for
//! every program it accepts, the VM code is byte-for-byte the same, and it
//! rejects the same programs, reporting the first problem on the same line
//! (see [`Compiled::official_lines`]). Its own messages are more specific and
//! carry exact spans. The behaviours of the official compiler that a reader of
//! the book would not expect are documented in `crates/jack/README.md`.
//!
//! * [`compile`] compiles one class (one `.jack` file).
//! * [`compile_program`] compiles a folder's worth of classes and checks calls
//!   between them, like the official compiler does when given a directory.
//! * [`parse`] gives tokens and an error-tolerant syntax tree; [`resolve`]
//!   tells what every identifier refers to; [`os`] describes the Jack OS API.

pub mod ast;
mod check;
mod diag;
pub mod editor;
pub mod lexer;
pub mod os;
mod parser;
mod program;
pub mod symbols;
pub mod xml;

use n2t_core::{Diagnostic, LineIndex, Severity, Span};

pub use diag::codes;
pub use editor::editor_diagnostics;
pub use lexer::{Keyword, Token, TokenKind};
pub use parser::MAX_DEPTH;
pub use program::{FileOutput, ProgramOutput, compile_program, compile_program_with};
pub use symbols::{Role, Symbol, Symbols, VarKind};
pub use xml::{parse_xml, tokens_xml};

use check::{Analysis, JavaLines};
use diag::JDiag;

/// Tokens and syntax tree of one Jack file.
#[derive(Debug, Clone)]
pub struct ParsedJack {
    /// Every token in source order, comments included (see [`TokenKind::is_trivia`]).
    pub tokens: Vec<Token>,
    /// The class, or `None` when the file contains no `class` keyword at all.
    pub class: Option<ast::Class>,
    /// Lexical and syntax diagnostics.
    pub diagnostics: Vec<Diagnostic>,
}

/// Tokenizes and parses `source`. Never fails; the tree is as complete as the
/// input allows.
pub fn parse(source: &str) -> ParsedJack {
    let lexed = lexer::lex(source);
    let out = parser::parse_tokens(source, &lexed);
    let mut diags = out.diags;
    diags.sort_by_key(|d| d.order);
    ParsedJack {
        tokens: lexed.tokens,
        class: out.class,
        diagnostics: diags.into_iter().map(|d| d.diag).collect(),
    }
}

/// What every identifier in the parsed file refers to.
pub fn resolve(parsed: &ParsedJack) -> Symbols {
    let Some(class) = &parsed.class else {
        return Symbols::default();
    };
    let tokens: Vec<Token> = parsed
        .tokens
        .iter()
        .filter(|t| !t.kind.is_trivia())
        .copied()
        .collect();
    let lines = JavaLines::new(&tokens, 0);
    let mut symbols = check::analyze(class, &lines).symbols;
    symbols.sort_by_key(|s| (s.span.start, s.span.end));
    Symbols { symbols }
}

/// The result of compiling one class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compiled {
    /// The VM code, or `None` if there are errors.
    pub vm: Option<String>,
    /// Errors, warnings and hints, in the order the official compiler would
    /// report them.
    pub diagnostics: Vec<Diagnostic>,
    /// For each VM command in `vm`, the 0-based Jack source line it came from.
    pub line_map: Vec<u32>,
    /// For each entry of `diagnostics`, the 1-based line number the official
    /// compiler prints for the same problem, or 0 if it does not report it.
    /// This sometimes differs from the diagnostic's span (the official
    /// compiler reports a bad assignment one line above the `;`, for
    /// example); it exists for conformance testing.
    pub official_lines: Vec<u32>,
}

impl Compiled {
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(Diagnostic::is_error)
    }
}

/// Compilation settings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// The official compiler still writes the `.vm` file when the only errors
    /// are in calls it checks after compiling (an unknown subroutine of a known
    /// class, a wrong argument count, a method called as a function or vice
    /// versa). With this set, [`Compiled::vm`] is produced in that case too.
    /// By default any error withholds the VM code.
    pub keep_vm_on_call_errors: bool,
}

/// Compiles one class. `file_stem` is the file name without `.jack`; the class
/// must have the same name.
pub fn compile(file_stem: &str, source: &str) -> Compiled {
    compile_with(file_stem, source, &Options::default())
}

/// [`compile`] with explicit [`Options`].
pub fn compile_with(file_stem: &str, source: &str, options: &Options) -> Compiled {
    let unit = Unit::new(file_stem, source);
    let table: Vec<&check::ClassSig> = unit.sig_if_clean().into_iter().collect();
    let calls = program::check_calls(&unit.deferred, &table, None);
    unit.finish(calls, options)
}

/// Everything an editor needs for one file at once.
#[derive(Debug, Clone)]
pub struct FileAnalysis {
    pub parsed: ParsedJack,
    pub symbols: Symbols,
    pub compiled: Compiled,
}

/// Parses, resolves and compiles one file in a single call.
pub fn analyze(file_stem: &str, source: &str) -> FileAnalysis {
    let parsed = parse(source);
    let symbols = resolve(&parsed);
    let compiled = compile(file_stem, source);
    FileAnalysis {
        parsed,
        symbols,
        compiled,
    }
}

/// One compiled-but-not-finished file.
pub(crate) struct Unit<'a> {
    source: &'a str,
    /// Lexical, syntax and immediate semantic diagnostics, in reporting order.
    diags: Vec<JDiag>,
    analysis: Option<Analysis>,
    pub(crate) deferred: Vec<check::DeferredCall>,
    pub(crate) class_name: Option<String>,
    pub(crate) class_name_span: Option<Span>,
}

impl<'a> Unit<'a> {
    pub(crate) fn new(file_stem: &str, source: &'a str) -> Self {
        let lexed = lexer::lex(source);
        let parsed = parser::parse_tokens(source, &lexed);
        let tokens: Vec<Token> = lexed
            .tokens
            .iter()
            .filter(|t| !t.kind.is_trivia())
            .copied()
            .collect();
        let lines = JavaLines::new(&tokens, lexed.eof_java_line);
        let mut diags = parsed.diags;
        let syntax_errors: Vec<usize> = diags
            .iter()
            .filter(|d| d.diag.is_error())
            .map(|d| d.order)
            .collect();
        let mut analysis = None;
        let mut deferred = Vec::new();
        let mut class_name = None;
        let mut class_name_span = None;
        if let Some(class) = &parsed.class {
            if let Some(name) = &class.name {
                class_name = Some(name.name.clone());
                class_name_span = Some(name.span);
                if name.name != file_stem {
                    diags.push(diag::error(
                        codes::CLASS_NAME_MISMATCH,
                        format!(
                            "The class is named `{}` but the file is `{file_stem}.jack`. Rename \
                             the class or the file so that they match.",
                            name.name
                        ),
                        name.span,
                        lines.at(name.span.start),
                    ));
                }
            }
            let mut a = check::analyze(class, &lines);
            // In a subroutine with a syntax error, semantic findings after the
            // error are usually artefacts of the recovery: drop them.
            let sub_spans: Vec<Span> = class
                .subroutines
                .iter()
                .map(|s| Span::new(s.span.start, s.span.end.max(s.span.start)))
                .collect();
            let first_error_in = |offset: usize| -> Option<usize> {
                let sub = sub_spans
                    .iter()
                    .find(|s| s.start <= offset && offset <= s.end)?;
                syntax_errors
                    .iter()
                    .copied()
                    .filter(|e| sub.start <= *e && *e <= sub.end)
                    .min()
            };
            a.diags.retain(|d| match first_error_in(d.order) {
                Some(e) => d.order < e,
                None => true,
            });
            deferred = std::mem::take(&mut a.deferred);
            deferred.retain(|d| match first_error_in(d.name_span.start) {
                Some(e) => d.name_span.start < e,
                None => true,
            });
            diags.append(&mut a.diags);
            analysis = Some(a);
        }
        diags.sort_by_key(|d| d.order);
        Unit {
            source,
            diags,
            analysis,
            deferred,
            class_name,
            class_name_span,
        }
    }

    pub(crate) fn has_immediate_errors(&self) -> bool {
        self.analysis.is_none() || self.diags.iter().any(|d| d.diag.is_error())
    }

    /// The class signature table, if the official compiler would know it
    /// (the class compiled without immediate errors).
    pub(crate) fn sig_if_clean(&self) -> Option<&check::ClassSig> {
        if self.has_immediate_errors() {
            return None;
        }
        self.analysis.as_ref().map(|a| &a.sig)
    }

    pub(crate) fn sig(&self) -> Option<&check::ClassSig> {
        self.analysis.as_ref().map(|a| &a.sig)
    }

    pub(crate) fn finish(self, calls: Vec<JDiag>, options: &Options) -> Compiled {
        let immediate = self.diags.iter().any(|d| d.diag.is_error());
        let call_errors = calls.iter().any(|d| d.diag.is_error());
        let mut all = self.diags;
        all.extend(calls);
        let emit = self.analysis.is_some()
            && !immediate
            && (!call_errors || options.keep_vm_on_call_errors);
        let (vm, line_map) = match (&self.analysis, emit) {
            (Some(a), true) => {
                let index = LineIndex::new(self.source);
                let mut text = String::new();
                for cmd in &a.vm {
                    text.push_str(cmd);
                    text.push('\n');
                }
                let lines = a
                    .vm_offsets
                    .iter()
                    .map(|o| index.position(*o).line)
                    .collect();
                (Some(text), lines)
            }
            _ => (None, Vec::new()),
        };
        Compiled {
            vm,
            official_lines: all.iter().map(|d| d.java_line).collect(),
            diagnostics: all.into_iter().map(|d| d.diag).collect(),
            line_map,
        }
    }
}

/// Formats a diagnostic as `file:line:col: severity: message` (1-based line
/// and column), the format of the `jackc` example.
pub fn format_diagnostic(file: &str, source: &str, d: &Diagnostic) -> String {
    let (line, col) = match d.span {
        Some(span) => LineIndex::new(source).line_col_one_based(source, span.start),
        None => (1, 1),
    };
    let sev = match d.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Information => "info",
        Severity::Hint => "hint",
    };
    format!("{file}:{line}:{col}: {sev}: {}", d.message)
}
