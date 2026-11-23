//! Language features for Jack (`.jack`) documents.
//!
//! The `.jack` files of a folder form one program: they are compiled
//! together (open editor buffers win over the files on disk), so calls
//! between classes are checked and classes and subroutines can be found
//! across files.

use std::collections::HashMap;
use std::sync::Arc;

use lsp_types as lsp;
use lsp_types::Url;
use n2t_core::host::{path, read_text};
use n2t_core::{LineIndex, Span};
use n2t_jack::ast::{Class, ClassVarKind, Subroutine, SubroutineKind};
use n2t_jack::os;
use n2t_jack::{
    Keyword, ParsedJack, ProgramOutput, Role, Symbol, Symbols, Token, TokenKind, VarKind,
};

use crate::convert::{Text, clean_doc, path_to_url};
use crate::workspace::{DocKind, UNTITLED_DIR, Workspace};

/// The most `.jack` files of one folder that are compiled together.
const MAX_FILES: usize = 256;

/// One `.jack` file of a folder.
pub struct JackFile {
    pub stem: String,
    /// Where the file is: the open document's URI, or the file's URL.
    pub uri: Option<Url>,
    /// Engine-form path, when the file has one.
    pub path: Option<String>,
    pub text: Arc<str>,
    pub lines: LineIndex,
    pub parsed: ParsedJack,
    pub symbols: Symbols,
}

impl JackFile {
    pub fn text(&self) -> Text<'_> {
        Text::new(&self.text, &self.lines)
    }

    fn class(&self) -> Option<&Class> {
        self.parsed.class.as_ref()
    }

    fn class_name(&self) -> Option<&str> {
        self.class()?.name.as_ref().map(|n| n.name.as_str())
    }

    fn location(&self, span: Span) -> Option<lsp::Location> {
        Some(lsp::Location::new(
            self.uri.clone()?,
            self.text().range(span),
        ))
    }
}

/// The Jack program of one folder.
pub struct JackFolder {
    pub files: Vec<JackFile>,
    pub program: ProgramOutput,
}

impl JackFolder {
    fn class_file(&self, name: &str) -> Option<&JackFile> {
        self.files.iter().find(|f| f.class_name() == Some(name))
    }

    fn class_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .files
            .iter()
            .filter_map(|f| f.class_name().map(str::to_string))
            .filter(|n| !n.is_empty())
            .collect();
        for c in os::OS_CLASSES {
            if !names.iter().any(|n| n == c) {
                names.push(c.to_string());
            }
        }
        names
    }
}

/// A source of the folder: stem, location, path and text.
type Source = (String, Option<Url>, Option<String>, Arc<str>);

/// The compiled program of the folder holding `dir`.
pub fn folder(ws: &mut Workspace, dir: &str) -> Arc<JackFolder> {
    if let Some(f) = ws.jack_folders.get(dir) {
        return f.clone();
    }
    let mut sources: Vec<Source> = ws
        .docs
        .values()
        .filter(|d| d.dir == dir && d.kind == DocKind::Jack)
        .map(|d| {
            (
                d.stem.clone(),
                Some(d.uri.clone()),
                d.path.clone(),
                d.text.clone(),
            )
        })
        .collect();
    if dir != UNTITLED_DIR {
        let mut names: Vec<String> = ws
            .host
            .list(dir)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| !e.is_dir && path::has_extension(&e.name, "jack"))
            .map(|e| e.name)
            .collect();
        names.sort();
        for name in names {
            if sources.len() >= MAX_FILES {
                break;
            }
            let file_path = path::join(dir, &name);
            if sources.iter().any(|s| s.2.as_deref() == Some(&file_path)) {
                continue;
            }
            let Ok(file) = read_text(ws.host.as_ref(), &file_path) else {
                continue;
            };
            sources.push((
                path::stem(&name).to_string(),
                path_to_url(&file_path),
                Some(file_path),
                Arc::from(file.text),
            ));
        }
    }
    sources.sort_by(|a, b| a.0.cmp(&b.0));
    let inputs: Vec<(&str, &str)> = sources
        .iter()
        .map(|(stem, _, _, text)| (stem.as_str(), &**text))
        .collect();
    let program = n2t_jack::compile_program(&inputs);
    let files = sources
        .iter()
        .map(|(stem, uri, file_path, text)| {
            let parsed = n2t_jack::parse(text);
            let symbols = n2t_jack::resolve(&parsed);
            JackFile {
                stem: stem.clone(),
                uri: uri.clone(),
                path: file_path.clone(),
                lines: LineIndex::new(text),
                text: text.clone(),
                parsed,
                symbols,
            }
        })
        .collect();
    let folder = Arc::new(JackFolder { files, program });
    ws.jack_folders.insert(dir.to_string(), folder.clone());
    folder
}

/// The folder of a Jack document and the index of the document in it.
fn file_of(ws: &mut Workspace, uri: &Url) -> Option<(Arc<JackFolder>, usize)> {
    let doc = ws.docs.get(uri)?;
    if doc.kind != DocKind::Jack {
        return None;
    }
    let dir = doc.dir.clone();
    let folder = folder(ws, &dir);
    let index = folder
        .files
        .iter()
        .position(|f| f.uri.as_ref() == Some(uri))?;
    Some((folder, index))
}

/// Errors, warnings and hints of a Jack document, including the checks of
/// calls between the classes of its folder.
pub fn diagnostics(ws: &mut Workspace, uri: &Url) -> Vec<n2t_core::Diagnostic> {
    let Some((folder, index)) = file_of(ws, uri) else {
        return Vec::new();
    };
    let folder_name = ws
        .docs
        .get(uri)
        .map(|doc| n2t_core::host::path::file_name(&doc.dir).to_string())
        .unwrap_or_default();
    match (folder.program.files.get(index), folder.files.get(index)) {
        (Some(output), Some(file)) => {
            n2t_jack::editor_diagnostics(&file.parsed, &folder_name, &output.compiled.diagnostics)
        }
        _ => Vec::new(),
    }
}

// -------------------------------------------------------------------------
// Shared helpers
// -------------------------------------------------------------------------

fn markdown(value: String, range: Option<lsp::Range>) -> lsp::Hover {
    lsp::Hover {
        contents: lsp::HoverContents::Markup(lsp::MarkupContent {
            kind: lsp::MarkupKind::Markdown,
            value,
        }),
        range,
    }
}

/// The text of a `/** … */` comment without its delimiters.
fn doc_text(text: &str, span: Option<Span>) -> String {
    let Some(raw) = span.and_then(|s| text.get(s.start..s.end)) else {
        return String::new();
    };
    let inner = raw.strip_prefix("/**").unwrap_or(raw);
    let inner = inner.strip_suffix("*/").unwrap_or(inner);
    clean_doc(inner)
}

fn param_texts(sub: &Subroutine) -> Vec<String> {
    sub.params
        .iter()
        .map(|p| {
            format!(
                "{} {}",
                p.ty.as_ref().map_or("?", |t| t.kind.name()),
                p.name.as_ref().map_or("?", |n| n.name.as_str())
            )
        })
        .collect()
}

/// A subroutine's signature and parameters: `method int Point.dist(Point q)`.
fn signature(class: &str, sub: &Subroutine) -> (String, Vec<String>) {
    let params = param_texts(sub);
    let label = format!(
        "{} {} {}.{}({})",
        sub.kind.as_str(),
        sub.return_type.as_ref().map_or("?", |r| r.name()),
        class,
        sub.name.as_ref().map_or("?", |n| n.name.as_str()),
        params.join(", ")
    );
    (label, params)
}

fn os_params(sub: &os::OsSubroutine) -> Vec<String> {
    sub.params.iter().map(|(t, n)| format!("{t} {n}")).collect()
}

fn find_sub<'a>(class: &'a Class, name: &str) -> Option<&'a Subroutine> {
    class
        .subroutines
        .iter()
        .find(|s| s.name.as_ref().is_some_and(|n| n.name == name))
}

/// The subroutine whose body contains `offset`.
fn enclosing_sub(class: &Class, offset: usize) -> Option<&Subroutine> {
    class
        .subroutines
        .iter()
        .rev()
        .find(|s| s.span.start <= offset)
        .filter(|s| offset <= s.span.end || s.body_close.is_none())
}

/// A variable visible at a point of a class.
struct Var {
    name: String,
    ty: String,
    kind: VarKind,
}

/// The variables visible inside `sub` (or at class level), in the order the
/// compiler looks names up: arguments, locals, fields (not in functions),
/// statics.
fn visible_vars(class: &Class, sub: Option<&Subroutine>) -> Vec<Var> {
    let mut vars = Vec::new();
    if let Some(sub) = sub {
        for p in &sub.params {
            if let (Some(ty), Some(name)) = (&p.ty, &p.name) {
                vars.push(Var {
                    name: name.name.clone(),
                    ty: ty.kind.name().to_string(),
                    kind: VarKind::Argument,
                });
            }
        }
        for dec in &sub.locals {
            for name in &dec.names {
                vars.push(Var {
                    name: name.name.clone(),
                    ty: dec.ty.as_ref().map_or("?", |t| t.kind.name()).to_string(),
                    kind: VarKind::Local,
                });
            }
        }
    }
    let in_function = sub.is_some_and(|s| s.kind == SubroutineKind::Function);
    for wanted in [ClassVarKind::Field, ClassVarKind::Static] {
        if wanted == ClassVarKind::Field && in_function {
            continue;
        }
        for dec in class.vars.iter().filter(|d| d.kind == wanted) {
            for name in &dec.names {
                vars.push(Var {
                    name: name.name.clone(),
                    ty: dec.ty.as_ref().map_or("?", |t| t.kind.name()).to_string(),
                    kind: if wanted == ClassVarKind::Field {
                        VarKind::Field
                    } else {
                        VarKind::Static
                    },
                });
            }
        }
    }
    vars.retain(|v| !v.name.is_empty());
    vars
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

// -------------------------------------------------------------------------
// Hover
// -------------------------------------------------------------------------

fn class_markdown(folder: &JackFolder, name: &str) -> String {
    if let Some(file) = folder.class_file(name)
        && let Some(class) = file.class()
    {
        let fields = class
            .vars
            .iter()
            .filter(|d| d.kind == ClassVarKind::Field)
            .map(|d| d.names.len())
            .sum::<usize>();
        let mut text = format!(
            "```jack\nclass {name}\n```\n{}, {}",
            plural(class.subroutines.len(), "subroutine"),
            plural(fields, "field")
        );
        let doc = doc_text(&file.text, class.doc);
        if !doc.is_empty() {
            text.push_str("\n\n");
            text.push_str(&doc);
        }
        return text;
    }
    if let Some(description) = os::class_description(name) {
        return format!("```jack\nclass {name}\n```\nJack OS class. {description}");
    }
    match name {
        "int" | "char" | "boolean" => format!("`{name}` is a primitive type"),
        _ => format!("class `{name}`: not in this folder and not part of the Jack OS"),
    }
}

fn subroutine_markdown(folder: &JackFolder, class: &str, name: &str) -> String {
    if let Some(file) = folder.class_file(class)
        && let Some(sub) = file.class().and_then(|c| find_sub(c, name))
    {
        let (label, _) = signature(class, sub);
        let mut text = format!("```jack\n{label}\n```");
        let doc = doc_text(&file.text, sub.doc);
        if !doc.is_empty() {
            text.push_str("\n\n");
            text.push_str(&doc);
        }
        return text;
    }
    if let Some(sub) = os::find(class, name) {
        let mut text = format!("```jack\n{}\n```\n{}", sub.signature(), sub.description);
        if sub.internal {
            text.push_str("\n\nCalled by `Sys.init`; programs should not call it.");
        }
        return text;
    }
    if folder.class_file(class).is_some() || os::is_os_class(class) {
        format!("`{class}` has no subroutine `{name}`")
    } else {
        format!("`{class}.{name}`: class `{class}` is not in this folder")
    }
}

pub fn hover(ws: &mut Workspace, uri: &Url, position: lsp::Position) -> Option<lsp::Hover> {
    let (folder, index) = file_of(ws, uri)?;
    let file = folder.files.get(index)?;
    let text = file.text();
    let offset = text.offset(position);
    let symbol = file.symbols.at(offset)?;
    let value = match &symbol.role {
        Role::Variable { kind, ty, index } => {
            let mut value = format!(
                "```jack\n{} {ty} {}\n```\n{} variable, VM `{} {index}`",
                match kind {
                    VarKind::Argument => "argument",
                    VarKind::Local => "var",
                    VarKind::Field => "field",
                    VarKind::Static => "static",
                },
                symbol.name,
                kind.as_str(),
                kind.segment()
            );
            if *kind == VarKind::Argument
                && let Some(sub) = file.class().and_then(|c| enclosing_sub(c, offset))
                && sub.kind == SubroutineKind::Method
            {
                value.push_str(" (`argument 0` is `this` in a method)");
            }
            value
        }
        Role::Subroutine { class, .. } => subroutine_markdown(&folder, class, &symbol.name),
        Role::Class => class_markdown(&folder, &symbol.name),
        Role::Undefined => format!("`{}` is not declared here", symbol.name),
    };
    Some(markdown(value, Some(text.range(symbol.span))))
}

// -------------------------------------------------------------------------
// Definition, references, highlights
// -------------------------------------------------------------------------

/// Where a symbol is declared, as (file index, span).
fn declaration(folder: &JackFolder, index: usize, symbol: &Symbol) -> Option<(usize, Span)> {
    match &symbol.role {
        Role::Variable { .. } => Some((index, symbol.declaration?)),
        Role::Subroutine { class, .. } => {
            let (i, file) = folder
                .files
                .iter()
                .enumerate()
                .find(|(_, f)| f.class_name() == Some(class.as_str()))?;
            let sub = find_sub(file.class()?, &symbol.name)?;
            Some((i, sub.name.as_ref()?.span))
        }
        Role::Class => {
            let (i, file) = folder
                .files
                .iter()
                .enumerate()
                .find(|(_, f)| f.class_name() == Some(symbol.name.as_str()))?;
            Some((i, file.class()?.name.as_ref()?.span))
        }
        Role::Undefined => None,
    }
}

pub fn definition(ws: &mut Workspace, uri: &Url, position: lsp::Position) -> Option<lsp::Location> {
    let (folder, index) = file_of(ws, uri)?;
    let file = folder.files.get(index)?;
    let symbol = file.symbols.at(file.text().offset(position))?;
    let (i, span) = declaration(&folder, index, symbol)?;
    folder.files.get(i)?.location(span)
}

/// Whether two occurrences refer to the same class or subroutine.
fn same_global(a: &Symbol, b: &Symbol) -> bool {
    match (&a.role, &b.role) {
        (Role::Class, Role::Class) => a.name == b.name,
        (Role::Subroutine { class: x, .. }, Role::Subroutine { class: y, .. }) => {
            x == y && a.name == b.name
        }
        _ => false,
    }
}

/// Every occurrence of the symbol: (file index, occurrence).
fn occurrences<'a>(
    folder: &'a JackFolder,
    index: usize,
    symbol: &'a Symbol,
) -> Vec<(usize, &'a Symbol)> {
    match symbol.role {
        Role::Variable { .. } | Role::Undefined => folder
            .files
            .get(index)
            .map(|f| f.symbols.references(symbol).map(|s| (index, s)).collect())
            .unwrap_or_default(),
        Role::Class | Role::Subroutine { .. } => folder
            .files
            .iter()
            .enumerate()
            .flat_map(|(i, f)| {
                f.symbols
                    .symbols
                    .iter()
                    .filter(|s| same_global(s, symbol))
                    .map(move |s| (i, s))
            })
            .collect(),
    }
}

pub fn references(
    ws: &mut Workspace,
    uri: &Url,
    position: lsp::Position,
    include_declaration: bool,
) -> Vec<lsp::Location> {
    let Some((folder, index)) = file_of(ws, uri) else {
        return Vec::new();
    };
    let Some(file) = folder.files.get(index) else {
        return Vec::new();
    };
    let Some(symbol) = file.symbols.at(file.text().offset(position)) else {
        return Vec::new();
    };
    occurrences(&folder, index, symbol)
        .into_iter()
        .filter(|(_, s)| include_declaration || !s.is_declaration)
        .filter_map(|(i, s)| folder.files.get(i)?.location(s.span))
        .collect()
}

pub fn highlights(
    ws: &mut Workspace,
    uri: &Url,
    position: lsp::Position,
) -> Vec<lsp::DocumentHighlight> {
    let Some((folder, index)) = file_of(ws, uri) else {
        return Vec::new();
    };
    let Some(file) = folder.files.get(index) else {
        return Vec::new();
    };
    let text = file.text();
    let Some(symbol) = file.symbols.at(text.offset(position)) else {
        return Vec::new();
    };
    occurrences(&folder, index, symbol)
        .into_iter()
        .filter(|(i, _)| *i == index)
        .map(|(_, s)| lsp::DocumentHighlight {
            range: text.range(s.span),
            kind: Some(if s.is_declaration {
                lsp::DocumentHighlightKind::WRITE
            } else {
                lsp::DocumentHighlightKind::READ
            }),
        })
        .collect()
}

// -------------------------------------------------------------------------
// Rename
// -------------------------------------------------------------------------

/// A Jack identifier as the book defines it (letters, digits, `_`, not
/// starting with a digit), and not a keyword.
pub fn is_valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && Keyword::from_word(name).is_none()
}

pub fn prepare_rename(
    ws: &mut Workspace,
    uri: &Url,
    position: lsp::Position,
) -> Option<lsp::PrepareRenameResponse> {
    let (folder, index) = file_of(ws, uri)?;
    let file = folder.files.get(index)?;
    let text = file.text();
    let symbol = file.symbols.at(text.offset(position))?;
    renamable(&folder, index, symbol)
        .ok()
        .map(|_| lsp::PrepareRenameResponse::Range(text.range(symbol.span)))
}

fn renamable(folder: &JackFolder, index: usize, symbol: &Symbol) -> Result<(), String> {
    match &symbol.role {
        Role::Undefined => Err(format!("`{}` is not declared", symbol.name)),
        Role::Variable { .. } if symbol.declaration.is_none() => {
            Err(format!("`{}` is not declared in this file", symbol.name))
        }
        Role::Variable { .. } => Ok(()),
        Role::Class | Role::Subroutine { .. } => {
            let class = match &symbol.role {
                Role::Subroutine { class, .. } => class.as_str(),
                _ => symbol.name.as_str(),
            };
            if folder.class_file(class).is_none() {
                return Err(if os::is_os_class(class) {
                    format!("`{class}` is part of the Jack OS and cannot be renamed")
                } else {
                    format!("class `{class}` is not in this folder")
                });
            }
            if declaration(folder, index, symbol).is_none() {
                return Err(format!("`{}` is not declared in this folder", symbol.name));
            }
            Ok(())
        }
    }
}

pub fn rename(
    ws: &mut Workspace,
    uri: &Url,
    position: lsp::Position,
    new_name: &str,
) -> Result<Option<lsp::WorkspaceEdit>, String> {
    let Some((folder, index)) = file_of(ws, uri) else {
        return Ok(None);
    };
    let Some(file) = folder.files.get(index) else {
        return Ok(None);
    };
    let offset = file.text().offset(position);
    let Some(symbol) = file.symbols.at(offset) else {
        return Ok(None);
    };
    renamable(&folder, index, symbol)?;
    if !is_valid_identifier(new_name) {
        return Err(format!(
            "`{new_name}` is not a valid Jack identifier (letters, digits and `_`, not starting \
             with a digit, not a keyword)"
        ));
    }
    if new_name != symbol.name {
        check_collision(&folder, index, symbol, new_name)?;
    }

    let mut by_file: HashMap<usize, Vec<lsp::TextEdit>> = HashMap::new();
    for (i, s) in occurrences(&folder, index, symbol) {
        if let Some(f) = folder.files.get(i) {
            by_file.entry(i).or_default().push(lsp::TextEdit::new(
                f.text().range(s.span),
                new_name.to_string(),
            ));
        }
    }
    let mut files: Vec<(usize, Vec<lsp::TextEdit>)> = by_file.into_iter().collect();
    files.sort_by_key(|(i, _)| *i);

    // Renaming a class renames its file too, so they still match.
    let file_rename = match symbol.role {
        Role::Class if new_name != symbol.name => folder
            .class_file(&symbol.name)
            .filter(|f| f.stem == symbol.name)
            .and_then(|f| {
                let old_path = f.path.as_ref()?;
                let new_path = path::join(path::dir(old_path), &format!("{new_name}.jack"));
                Some((f.uri.clone()?, path_to_url(&new_path)?))
            }),
        _ => None,
    };
    let Some((old_uri, new_uri)) = file_rename else {
        let changes = files
            .into_iter()
            .filter_map(|(i, edits)| Some((folder.files.get(i)?.uri.clone()?, edits)))
            .collect();
        return Ok(Some(lsp::WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }));
    };
    let mut operations: Vec<lsp::DocumentChangeOperation> = files
        .into_iter()
        .filter_map(|(i, edits)| {
            Some(lsp::DocumentChangeOperation::Edit(lsp::TextDocumentEdit {
                text_document: lsp::OptionalVersionedTextDocumentIdentifier {
                    uri: folder.files.get(i)?.uri.clone()?,
                    version: None,
                },
                edits: edits.into_iter().map(lsp::OneOf::Left).collect(),
            }))
        })
        .collect();
    operations.push(lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Rename(
        lsp::RenameFile {
            old_uri,
            new_uri,
            options: None,
            annotation_id: None,
        },
    )));
    Ok(Some(lsp::WorkspaceEdit {
        document_changes: Some(lsp::DocumentChanges::Operations(operations)),
        ..Default::default()
    }))
}

/// Refuses a rename that would make two declarations share a name, or make
/// a name start meaning something else.
fn check_collision(
    folder: &JackFolder,
    index: usize,
    symbol: &Symbol,
    new_name: &str,
) -> Result<(), String> {
    let is_class = |name: &str| folder.class_file(name).is_some() || os::is_os_class(name);
    match &symbol.role {
        Role::Variable { kind, .. } => {
            let Some(class) = folder.files.get(index).and_then(|f| f.class()) else {
                return Ok(());
            };
            if is_class(new_name) {
                return Err(format!(
                    "`{new_name}` is a class name: `{new_name}.f()` would then call a method on \
                     the variable"
                ));
            }
            let class_vars = class.vars.iter().flat_map(|d| &d.names);
            let sub_vars = |s: &'_ Subroutine| -> Vec<String> {
                s.params
                    .iter()
                    .filter_map(|p| p.name.as_ref().map(|n| n.name.clone()))
                    .chain(
                        s.locals
                            .iter()
                            .flat_map(|d| d.names.iter().map(|n| n.name.clone())),
                    )
                    .collect()
            };
            let clash = match kind {
                VarKind::Field | VarKind::Static => {
                    class_vars.clone().any(|n| n.name == new_name)
                        || class
                            .subroutines
                            .iter()
                            .any(|s| sub_vars(s).iter().any(|n| n == new_name))
                }
                VarKind::Argument | VarKind::Local => {
                    let decl = symbol.declaration.map_or(0, |d| d.start);
                    class_vars.clone().any(|n| n.name == new_name)
                        || enclosing_sub(class, decl)
                            .is_some_and(|s| sub_vars(s).iter().any(|n| n == new_name))
                }
            };
            if clash {
                return Err(format!(
                    "a variable named `{new_name}` is already visible here: the rename would \
                     change which variable some uses refer to"
                ));
            }
        }
        Role::Subroutine { class, .. } => {
            if let Some(c) = folder.class_file(class).and_then(|f| f.class())
                && find_sub(c, new_name).is_some()
            {
                return Err(format!("`{class}` already has a subroutine `{new_name}`"));
            }
        }
        Role::Class => {
            if is_class(new_name) {
                return Err(format!("a class named `{new_name}` already exists"));
            }
            let shadowed = folder.files.iter().any(|f| {
                f.symbols
                    .symbols
                    .iter()
                    .any(|s| matches!(s.role, Role::Variable { .. }) && s.name == new_name)
            });
            if shadowed {
                return Err(format!(
                    "a variable named `{new_name}` exists: calls such as `{new_name}.f()` would \
                     then be method calls on it"
                ));
            }
        }
        Role::Undefined => {}
    }
    Ok(())
}

// -------------------------------------------------------------------------
// Completion
// -------------------------------------------------------------------------

fn keyword_items(words: &[&str]) -> Vec<lsp::CompletionItem> {
    words
        .iter()
        .map(|w| lsp::CompletionItem {
            label: w.to_string(),
            kind: Some(lsp::CompletionItemKind::KEYWORD),
            ..Default::default()
        })
        .collect()
}

fn class_items(folder: &JackFolder) -> Vec<lsp::CompletionItem> {
    folder
        .class_names()
        .into_iter()
        .map(|name| lsp::CompletionItem {
            detail: Some(if os::is_os_class(&name) {
                "Jack OS class".to_string()
            } else {
                "class".to_string()
            }),
            documentation: os::class_description(&name)
                .map(|d| lsp::Documentation::String(d.to_string())),
            label: name,
            kind: Some(lsp::CompletionItemKind::CLASS),
            ..Default::default()
        })
        .collect()
}

fn type_items(folder: &JackFolder) -> Vec<lsp::CompletionItem> {
    let mut items = keyword_items(&["int", "char", "boolean"]);
    items.extend(class_items(folder));
    items
}

fn var_items(vars: &[Var]) -> Vec<lsp::CompletionItem> {
    let mut seen: Vec<&str> = Vec::new();
    let mut items = Vec::new();
    for v in vars {
        if seen.contains(&v.name.as_str()) {
            continue;
        }
        seen.push(&v.name);
        items.push(lsp::CompletionItem {
            label: v.name.clone(),
            kind: Some(match v.kind {
                VarKind::Field => lsp::CompletionItemKind::FIELD,
                VarKind::Static => lsp::CompletionItemKind::VARIABLE,
                VarKind::Argument | VarKind::Local => lsp::CompletionItemKind::VARIABLE,
            }),
            detail: Some(format!("{} {}", v.kind.as_str(), v.ty)),
            sort_text: Some(format!("0{}", v.name)),
            ..Default::default()
        });
    }
    items
}

fn sub_kind_item(kind: SubroutineKind) -> lsp::CompletionItemKind {
    match kind {
        SubroutineKind::Method => lsp::CompletionItemKind::METHOD,
        SubroutineKind::Function => lsp::CompletionItemKind::FUNCTION,
        SubroutineKind::Constructor => lsp::CompletionItemKind::CONSTRUCTOR,
    }
}

/// The subroutines of `class` that can be called with `want` (methods on an
/// object, or functions and constructors on the class).
fn member_items(folder: &JackFolder, class: &str, methods: bool) -> Vec<lsp::CompletionItem> {
    let wanted = |k: SubroutineKind| (k == SubroutineKind::Method) == methods;
    if let Some(file) = folder.class_file(class)
        && let Some(c) = file.class()
    {
        return c
            .subroutines
            .iter()
            .filter(|s| wanted(s.kind))
            .filter_map(|s| {
                let name = s.name.as_ref()?;
                let doc = doc_text(&file.text, s.doc);
                Some(lsp::CompletionItem {
                    label: name.name.clone(),
                    kind: Some(sub_kind_item(s.kind)),
                    detail: Some(signature(class, s).0),
                    documentation: (!doc.is_empty()).then_some(lsp::Documentation::String(doc)),
                    ..Default::default()
                })
            })
            .collect();
    }
    os::class_api(class)
        .filter(|s| wanted(s.kind) && !s.internal)
        .map(|s| lsp::CompletionItem {
            label: s.name.to_string(),
            kind: Some(sub_kind_item(s.kind)),
            detail: Some(s.signature()),
            documentation: Some(lsp::Documentation::String(s.description.to_string())),
            ..Default::default()
        })
        .collect()
}

/// Whether the cursor is inside a comment or a string.
fn in_comment_or_string(text: &str, tokens: &[Token], offset: usize) -> bool {
    tokens.iter().any(|t| {
        let s = t.span;
        if s.start >= offset {
            return false;
        }
        let raw = text.get(s.start..s.end).unwrap_or("");
        match t.kind {
            TokenKind::LineComment => offset <= s.end,
            TokenKind::BlockComment | TokenKind::DocComment => {
                offset < s.end || (offset == s.end && (raw.len() < 4 || !raw.ends_with("*/")))
            }
            TokenKind::StringConst => {
                offset < s.end || (offset == s.end && (raw.len() < 2 || !raw.ends_with('"')))
            }
            _ => false,
        }
    })
}

/// The innermost `(` or `[` before the end of `tokens` that is not closed,
/// within the current statement.
fn unclosed_bracket(tokens: &[&Token]) -> Option<usize> {
    let mut depth = 0usize;
    for (i, t) in tokens.iter().enumerate().rev() {
        match t.kind {
            TokenKind::Symbol(b')' | b']') => depth += 1,
            TokenKind::Symbol(b'(' | b'[') => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
            TokenKind::Symbol(b';' | b'{' | b'}') => return None,
            _ => {}
        }
    }
    None
}

/// The first token of the statement or declaration the cursor is in.
fn statement_start<'a>(tokens: &[&'a Token]) -> Option<&'a Token> {
    let start = tokens
        .iter()
        .rposition(|t| matches!(t.kind, TokenKind::Symbol(b';' | b'{' | b'}')))
        .map_or(0, |i| i + 1);
    tokens.get(start).copied()
}

fn lookup<'a>(vars: &'a [Var], name: &str) -> Option<&'a Var> {
    vars.iter().find(|v| v.name == name)
}

pub fn completion(
    ws: &mut Workspace,
    uri: &Url,
    position: lsp::Position,
) -> Vec<lsp::CompletionItem> {
    let Some((folder, index)) = file_of(ws, uri) else {
        return Vec::new();
    };
    let Some(file) = folder.files.get(index) else {
        return Vec::new();
    };
    let text = &*file.text;
    let offset = file.text().offset(position);
    if in_comment_or_string(text, &file.parsed.tokens, offset) {
        return Vec::new();
    }
    let mut before: Vec<&Token> = file
        .parsed
        .tokens
        .iter()
        .filter(|t| !t.kind.is_trivia() && t.span.end <= offset)
        .collect();
    if before.last().is_some_and(|t| {
        t.span.end == offset && matches!(t.kind, TokenKind::Identifier | TokenKind::Keyword(_))
    }) {
        before.pop();
    }
    let n = before.len();
    let prev = n.checked_sub(1).and_then(|i| before.get(i)).copied();
    let prev2 = n.checked_sub(2).and_then(|i| before.get(i)).copied();
    let depth = before.iter().fold(0usize, |d, t| match t.kind {
        TokenKind::Symbol(b'{') => d + 1,
        TokenKind::Symbol(b'}') => d.saturating_sub(1),
        _ => d,
    });
    let class = file.class();
    let sub = class.and_then(|c| enclosing_sub(c, offset));
    let vars = class.map(|c| visible_vars(c, sub)).unwrap_or_default();
    let this_class = file.class_name().unwrap_or("").to_string();
    let in_function = sub.is_some_and(|s| s.kind == SubroutineKind::Function);

    // `Name.` : the members of a class, or the methods of a variable's type.
    if let (Some(dot), Some(name)) = (prev, prev2)
        && dot.is_symbol(b'.')
        && name.kind == TokenKind::Identifier
    {
        let name = name.text(text);
        return match lookup(&vars, name) {
            Some(v) => member_items(&folder, &v.ty, true),
            None => member_items(&folder, name, false),
        };
    }

    let Some(prev) = prev else {
        return keyword_items(&["class"]);
    };
    let start = statement_start(&before);
    let start_kw = |k: Keyword| start.is_some_and(|t| t.is_keyword(k));
    let bracket = unclosed_bracket(&before);

    if depth == 0 {
        let has_class = before.iter().any(|t| t.is_keyword(Keyword::Class));
        return if has_class {
            Vec::new()
        } else {
            keyword_items(&["class"])
        };
    }
    if depth == 1 {
        if bracket.is_some() {
            // A parameter list.
            return if prev.is_symbol(b'(') || prev.is_symbol(b',') {
                type_items(&folder)
            } else {
                Vec::new()
            };
        }
        return match prev.kind {
            TokenKind::Symbol(b'{' | b';' | b'}') => {
                keyword_items(&["static", "field", "constructor", "function", "method"])
            }
            TokenKind::Keyword(Keyword::Static | Keyword::Field) => type_items(&folder),
            TokenKind::Keyword(Keyword::Constructor | Keyword::Function | Keyword::Method) => {
                let mut items = keyword_items(&["void"]);
                items.extend(type_items(&folder));
                items
            }
            _ => Vec::new(),
        };
    }

    // Inside a subroutine body.
    let expression = || {
        let mut items = var_items(&vars);
        let mut constants = vec!["true", "false", "null"];
        if !in_function {
            constants.push("this");
        }
        items.extend(keyword_items(&constants));
        items.extend(class_items(&folder));
        if !in_function {
            items.extend(member_items(&folder, &this_class, true));
        }
        items
    };
    if start_kw(Keyword::Var) {
        return if prev.is_keyword(Keyword::Var) {
            type_items(&folder)
        } else {
            Vec::new()
        };
    }
    match prev.kind {
        TokenKind::Symbol(b'{' | b';' | b'}') if bracket.is_none() => {
            let mut words = vec!["let", "do", "if", "while", "return"];
            let body_started = sub.and_then(|s| s.body_open).is_some_and(|open| {
                before.iter().any(|t| {
                    t.span.start > open.start
                        && matches!(
                            t.kind,
                            TokenKind::Keyword(
                                Keyword::Let
                                    | Keyword::Do
                                    | Keyword::If
                                    | Keyword::While
                                    | Keyword::Return
                            )
                        )
                })
            });
            if !body_started {
                words.insert(0, "var");
            }
            if prev.is_symbol(b'}') {
                words.push("else");
            }
            keyword_items(&words)
        }
        TokenKind::Keyword(Keyword::Let) => var_items(&vars),
        TokenKind::Keyword(Keyword::Do) => {
            let mut items: Vec<lsp::CompletionItem> = var_items(
                &vars
                    .into_iter()
                    .filter(|v| !matches!(v.ty.as_str(), "int" | "char" | "boolean"))
                    .collect::<Vec<_>>(),
            );
            items.extend(class_items(&folder));
            if !in_function {
                items.extend(member_items(&folder, &this_class, true));
            }
            items
        }
        TokenKind::Keyword(Keyword::Return) => expression(),
        TokenKind::Symbol(
            b'=' | b'(' | b'[' | b',' | b'+' | b'-' | b'*' | b'/' | b'&' | b'|' | b'<' | b'>'
            | b'~',
        ) => expression(),
        _ => Vec::new(),
    }
}

// -------------------------------------------------------------------------
// Signature help
// -------------------------------------------------------------------------

pub fn signature_help(
    ws: &mut Workspace,
    uri: &Url,
    position: lsp::Position,
) -> Option<lsp::SignatureHelp> {
    let (folder, index) = file_of(ws, uri)?;
    let file = folder.files.get(index)?;
    let text = &*file.text;
    let offset = file.text().offset(position);
    if in_comment_or_string(text, &file.parsed.tokens, offset) {
        return None;
    }
    let before: Vec<&Token> = file
        .parsed
        .tokens
        .iter()
        .filter(|t| !t.kind.is_trivia() && t.span.end <= offset)
        .collect();
    // The innermost unclosed `(` of a call, counting commas at its level.
    let mut depth = 0usize;
    let mut commas = 0u32;
    let mut open = None;
    for (i, t) in before.iter().enumerate().rev() {
        match t.kind {
            TokenKind::Symbol(b')' | b']') => depth += 1,
            TokenKind::Symbol(b'[') => depth = depth.checked_sub(1)?,
            TokenKind::Symbol(b'(') => {
                if depth == 0 {
                    open = Some(i);
                    break;
                }
                depth -= 1;
            }
            TokenKind::Symbol(b',') if depth == 0 => commas += 1,
            TokenKind::Symbol(b';' | b'{' | b'}') => return None,
            _ => {}
        }
    }
    let open = open?;
    let name_token = before.get(open.checked_sub(1)?)?;
    if name_token.kind != TokenKind::Identifier {
        return None;
    }
    let name = name_token.text(text);
    let receiver = open
        .checked_sub(3)
        .and_then(|i| Some((before.get(i)?, before.get(i + 1)?)))
        .filter(|(r, dot)| r.kind == TokenKind::Identifier && dot.is_symbol(b'.'))
        .map(|(r, _)| r.text(text));
    if receiver.is_none() {
        // `function int name(`: a declaration, not a call.
        let before_name = open.checked_sub(2).and_then(|i| before.get(i));
        if before_name.is_some_and(|t| {
            matches!(
                t.kind,
                TokenKind::Identifier
                    | TokenKind::Keyword(
                        Keyword::Int | Keyword::Char | Keyword::Boolean | Keyword::Void
                    )
            )
        }) {
            return None;
        }
    }
    let class_ast = file.class();
    let sub = class_ast.and_then(|c| enclosing_sub(c, offset));
    let vars = class_ast.map(|c| visible_vars(c, sub)).unwrap_or_default();
    let class = match receiver {
        None => file.class_name()?.to_string(),
        Some(r) => lookup(&vars, r).map_or(r.to_string(), |v| v.ty.clone()),
    };
    let (label, params, doc) = if let Some(f) = folder.class_file(&class)
        && let Some(s) = f.class().and_then(|c| find_sub(c, name))
    {
        let (label, params) = signature(&class, s);
        (label, params, doc_text(&f.text, s.doc))
    } else {
        let s = os::find(&class, name)?;
        (s.signature(), os_params(s), s.description.to_string())
    };
    let active = (!params.is_empty()).then(|| commas.min(params.len() as u32 - 1));
    Some(lsp::SignatureHelp {
        signatures: vec![lsp::SignatureInformation {
            label,
            documentation: (!doc.is_empty()).then_some(lsp::Documentation::String(doc)),
            parameters: Some(
                params
                    .into_iter()
                    .map(|p| lsp::ParameterInformation {
                        label: lsp::ParameterLabel::Simple(p),
                        documentation: None,
                    })
                    .collect(),
            ),
            active_parameter: active,
        }],
        active_signature: Some(0),
        active_parameter: active,
    })
}

// -------------------------------------------------------------------------
// Document symbols and semantic tokens
// -------------------------------------------------------------------------

#[allow(deprecated)] // `DocumentSymbol::deprecated` must still be initialised.
pub fn document_symbols(ws: &mut Workspace, uri: &Url) -> Vec<lsp::DocumentSymbol> {
    let Some((folder, index)) = file_of(ws, uri) else {
        return Vec::new();
    };
    let Some(file) = folder.files.get(index) else {
        return Vec::new();
    };
    let Some(class) = file.class() else {
        return Vec::new();
    };
    let Some(class_name) = &class.name else {
        return Vec::new();
    };
    let text = file.text();
    let symbol = |name: String, detail: String, kind, span: Span, selection: Span, children| {
        lsp::DocumentSymbol {
            name,
            detail: Some(detail),
            kind,
            tags: None,
            deprecated: None,
            range: text.range(span.merge(selection)),
            selection_range: text.range(selection),
            children,
        }
    };
    let mut children = Vec::new();
    for dec in &class.vars {
        let ty = dec.ty.as_ref().map_or("?", |t| t.kind.name());
        let (kind, word) = match dec.kind {
            ClassVarKind::Field => (lsp::SymbolKind::FIELD, "field"),
            ClassVarKind::Static => (lsp::SymbolKind::VARIABLE, "static"),
        };
        for name in dec.names.iter().filter(|n| !n.name.is_empty()) {
            children.push(symbol(
                name.name.clone(),
                format!("{word} {ty}"),
                kind,
                dec.span,
                name.span,
                None,
            ));
        }
    }
    for sub in &class.subroutines {
        let Some(name) = &sub.name else { continue };
        let kind = match sub.kind {
            SubroutineKind::Constructor => lsp::SymbolKind::CONSTRUCTOR,
            SubroutineKind::Function => lsp::SymbolKind::FUNCTION,
            SubroutineKind::Method => lsp::SymbolKind::METHOD,
        };
        children.push(symbol(
            name.name.clone(),
            signature(&class_name.name, sub).0,
            kind,
            sub.span,
            name.span,
            None,
        ));
    }
    vec![symbol(
        class_name.name.clone(),
        "class".to_string(),
        lsp::SymbolKind::CLASS,
        class.span,
        class_name.span,
        Some(children),
    )]
}

/// Semantic token classes of a Jack document, as (span, role) pairs for the
/// shared encoder: `Some(role)` for each resolved identifier.
pub fn semantic_symbols(ws: &mut Workspace, uri: &Url) -> Option<Vec<Symbol>> {
    let (folder, index) = file_of(ws, uri)?;
    Some(folder.files.get(index)?.symbols.symbols.clone())
}
