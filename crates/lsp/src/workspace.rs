//! Open documents and per-folder analysis projects.

use std::collections::HashMap;
use std::sync::Arc;

use lsp_types as lsp;
use lsp_types::Url;
use n2t_core::LineIndex;
use n2t_core::builtins::{BuiltinChip, BuiltinLibrary};
use n2t_core::hdl::resolve::{
    HostSourceProvider, OverlaySourceProvider, SourceFile, is_valid_chip_name,
};
use n2t_core::hdl::{ChipUnit, Elaborator, ElaboratorOptions, UnitOrigin};
use n2t_core::host::{SharedHost, path, read_text};
use n2t_core::source::Span;
use n2t_core::tst::{ParsedScript, ScriptAnalysis};

use crate::convert::{self, BUILTIN_SCHEME, Text, path_to_url, url_to_path};

pub type ProjectElaborator = Elaborator<OverlaySourceProvider<HostSourceProvider>>;

/// The folder that documents without a file (untitled buffers) live in.
/// Nothing exists there, so their parts resolve to the built-in chips.
const UNTITLED_DIR: &str = "/n2t-untitled";

/// All chips in one folder share a resolution context (the folder is the
/// "project directory" of the specification).
pub struct Project {
    pub dir: String,
    pub elaborator: ProjectElaborator,
    host: SharedHost,
}

impl Project {
    fn new(host: SharedHost, dir: String, builtins: Arc<BuiltinLibrary>) -> Self {
        let provider = OverlaySourceProvider::new(HostSourceProvider::new(host.clone(), &dir));
        Self {
            dir,
            elaborator: Elaborator::with_options(provider, builtins, ElaboratorOptions::editor()),
            host,
        }
    }

    /// The source identity the file-system provider uses for a chip.
    pub fn source_id(&self, chip_name: &str) -> String {
        self.elaborator.provider().inner().chip_path(chip_name)
    }

    /// Names of chips with a `.hdl` file in the folder.
    pub fn local_chips(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .files()
            .iter()
            .filter_map(|name| {
                let stem = name.strip_suffix(".hdl")?;
                is_valid_chip_name(stem).then(|| stem.to_string())
            })
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Names of the files in the folder.
    pub fn files(&self) -> Vec<String> {
        self.host
            .list(&self.dir)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| !e.is_dir)
            .map(|e| e.name)
            .collect()
    }

    /// Local chips plus built-ins not shadowed by a local file.
    pub fn all_chips(&self) -> Vec<String> {
        let mut names = self.local_chips();
        for chip in self.elaborator.builtins().iter() {
            if !names.contains(&chip.name) {
                names.push(chip.name.clone());
            }
        }
        names.sort();
        names
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocKind {
    Hdl,
    Tst,
    Other,
}

pub struct Document {
    pub uri: Url,
    pub version: i32,
    pub text: Arc<str>,
    pub lines: LineIndex,
    pub kind: DocKind,
    /// Engine-form file path, for `file:` documents.
    pub path: Option<String>,
    /// The project folder, in engine form.
    pub dir: String,
    /// The file stem (the chip name for `.hdl` files).
    pub stem: String,
    pub tst: Option<(ParsedScript, ScriptAnalysis)>,
}

impl Document {
    pub fn text(&self) -> Text<'_> {
        Text::new(&self.text, &self.lines)
    }

    /// Whether this HDL document is analysed through the project overlay
    /// (its stem is a valid chip name).
    pub fn is_overlay_chip(&self) -> bool {
        self.kind == DocKind::Hdl && is_valid_chip_name(&self.stem)
    }
}

pub struct Workspace {
    pub docs: HashMap<Url, Document>,
    pub projects: HashMap<String, Project>,
    pub builtins: Arc<BuiltinLibrary>,
    pub host: SharedHost,
}

impl Workspace {
    pub fn new(host: SharedHost) -> Self {
        Self {
            docs: HashMap::new(),
            projects: HashMap::new(),
            builtins: Arc::new(BuiltinLibrary::standard()),
            host,
        }
    }

    pub fn project(&mut self, dir: &str) -> &mut Project {
        let builtins = self.builtins.clone();
        let host = self.host.clone();
        self.projects
            .entry(dir.to_string())
            .or_insert_with(|| Project::new(host, dir.to_string(), builtins))
    }

    // ---------------------------------------------------------------------
    // Document lifecycle
    // ---------------------------------------------------------------------

    pub fn open(&mut self, uri: Url, text: String, version: i32) {
        let path = url_to_path(&uri);
        let file_name = path
            .as_deref()
            .map(|p| path::file_name(p).to_string())
            .unwrap_or_else(|| {
                uri.path_segments()
                    .and_then(|mut s| s.next_back())
                    .unwrap_or("")
                    .to_string()
            });
        let (stem, extension) = match file_name.rsplit_once('.') {
            Some((s, e)) => (s.to_string(), e.to_ascii_lowercase()),
            None => (file_name.clone(), String::new()),
        };
        let kind = match extension.as_str() {
            "hdl" => DocKind::Hdl,
            "tst" => DocKind::Tst,
            _ => DocKind::Other,
        };
        let dir = match path.as_deref() {
            Some(p) => path::dir(p).to_string(),
            None => UNTITLED_DIR.to_string(),
        };
        let text: Arc<str> = Arc::from(text);
        let doc = Document {
            uri: uri.clone(),
            version,
            lines: LineIndex::new(&text),
            text,
            kind,
            path,
            dir,
            stem,
            tst: None,
        };
        self.docs.insert(uri.clone(), doc);
        self.sync_overlay(&uri);
    }

    pub fn change(&mut self, uri: &Url, text: String, version: i32) {
        if let Some(doc) = self.docs.get_mut(uri) {
            doc.text = Arc::from(text);
            doc.lines = LineIndex::new(&doc.text);
            doc.version = version;
            doc.tst = None;
        }
        self.sync_overlay(uri);
    }

    pub fn close(&mut self, uri: &Url) {
        if let Some(doc) = self.docs.remove(uri)
            && doc.is_overlay_chip()
        {
            let stem = doc.stem.clone();
            self.project(&doc.dir)
                .elaborator
                .provider_mut()
                .remove_overlay(&stem);
        }
    }

    fn sync_overlay(&mut self, uri: &Url) {
        let Some(doc) = self.docs.get(uri) else {
            return;
        };
        if !doc.is_overlay_chip() {
            return;
        }
        let (dir, stem, text) = (doc.dir.clone(), doc.stem.clone(), doc.text.clone());
        let project = self.project(&dir);
        let id = project.source_id(&stem);
        project
            .elaborator
            .provider_mut()
            .set_overlay(stem, SourceFile::new(id, text));
    }

    /// Open documents sharing a project folder.
    pub fn docs_in(&self, dir: &str) -> Vec<Url> {
        self.docs
            .values()
            .filter(|d| d.dir == dir)
            .map(|d| d.uri.clone())
            .collect()
    }

    // ---------------------------------------------------------------------
    // Analysis
    // ---------------------------------------------------------------------

    /// The compiled unit for an HDL document.
    pub fn hdl_unit(&mut self, uri: &Url) -> Option<Arc<ChipUnit>> {
        let doc = self.docs.get(uri)?;
        if doc.kind != DocKind::Hdl {
            return None;
        }
        let (dir, stem, text) = (doc.dir.clone(), doc.stem.clone(), doc.text.clone());
        let overlay = doc.is_overlay_chip();
        let project = self.project(&dir);
        if overlay {
            Some(project.elaborator.compile(&stem))
        } else {
            let file = SourceFile::new(uri.to_string(), text);
            Some(Arc::new(project.elaborator.compile_file(None, file)))
        }
    }

    /// Ensures the script analysis of a TST document is up to date.
    pub fn tst_state(&mut self, uri: &Url) -> Option<&(ParsedScript, ScriptAnalysis)> {
        let doc = self.docs.get(uri)?;
        if doc.kind != DocKind::Tst {
            return None;
        }
        if doc.tst.is_none() {
            let (dir, text) = (doc.dir.clone(), doc.text.clone());
            let parsed = n2t_core::tst::parse(&text);
            let analysis =
                n2t_core::tst::analyze(&parsed.script, &mut self.project(&dir).elaborator);
            self.docs.get_mut(uri)?.tst = Some((parsed, analysis));
        }
        self.docs.get(uri)?.tst.as_ref()
    }

    /// Discards cached script analyses in a folder (after an HDL edit).
    pub fn invalidate_scripts(&mut self, dir: &str) {
        for doc in self.docs.values_mut() {
            if doc.dir == dir {
                doc.tst = None;
            }
        }
    }

    pub fn diagnostics(&mut self, uri: &Url) -> Vec<lsp::Diagnostic> {
        let Some(kind) = self.docs.get(uri).map(|d| d.kind) else {
            return Vec::new();
        };
        let core: Vec<n2t_core::Diagnostic> = match kind {
            DocKind::Hdl => self
                .hdl_unit(uri)
                .map(|u| u.diagnostics.clone())
                .unwrap_or_default(),
            DocKind::Tst => self
                .tst_state(uri)
                .map(|(p, a)| {
                    p.diagnostics
                        .iter()
                        .chain(&a.diagnostics)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default(),
            DocKind::Other => Vec::new(),
        };
        let doc = &self.docs[uri];
        let text = doc.text();
        let docs = &self.docs;
        let host = &self.host;
        let mut resolve = |id: &str| source_by_id(docs, host, id);
        core.iter()
            .map(|d| convert::diagnostic(uri, &text, d, &mut resolve))
            .collect()
    }

    // ---------------------------------------------------------------------
    // Locations
    // ---------------------------------------------------------------------

    /// The URL and text of a source identity (an open document or a file).
    pub fn source(&self, id: &str) -> Option<(Url, String)> {
        source_by_id(&self.docs, &self.host, id)
    }

    /// A location for a span in a chip unit's source (local or built-in).
    pub fn unit_location(&self, unit: &ChipUnit, span: Span) -> Option<lsp::Location> {
        match &unit.origin {
            UnitOrigin::Source(file) => {
                let (url, _) = self.source(&file.id)?;
                Some(lsp::Location::new(url, convert::range_in(&file.text, span)))
            }
            UnitOrigin::Builtin(chip) => self.builtin_location(chip, span),
            _ => None,
        }
    }

    /// A location in the read-only document that shows a built-in chip's
    /// interface. The editor fetches its text with `n2t/builtinSource`.
    pub fn builtin_location(&self, chip: &BuiltinChip, span: Span) -> Option<lsp::Location> {
        let url = Url::parse(&format!("{BUILTIN_SCHEME}:/{}.hdl", chip.name)).ok()?;
        Some(lsp::Location::new(
            url,
            convert::range_in(&chip.source, span),
        ))
    }
}

fn source_by_id(
    docs: &HashMap<Url, Document>,
    host: &SharedHost,
    id: &str,
) -> Option<(Url, String)> {
    if let Some(doc) = docs
        .values()
        .find(|d| d.path.as_deref() == Some(id) || d.uri.as_str() == id)
    {
        return Some((doc.uri.clone(), doc.text.to_string()));
    }
    let text = read_text(host.as_ref(), id).ok()?.text;
    Some((path_to_url(id)?, text))
}
