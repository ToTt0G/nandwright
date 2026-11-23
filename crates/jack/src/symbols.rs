//! Symbol information for editors: what every identifier in a class refers to.

use n2t_core::Span;

use crate::ast::SubroutineKind;

/// The four kinds of Jack variables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VarKind {
    Static,
    Field,
    Argument,
    Local,
}

impl VarKind {
    /// The VM memory segment holding variables of this kind.
    pub fn segment(self) -> &'static str {
        match self {
            VarKind::Static => "static",
            VarKind::Field => "this",
            VarKind::Argument => "argument",
            VarKind::Local => "local",
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            VarKind::Static => "static",
            VarKind::Field => "field",
            VarKind::Argument => "argument",
            VarKind::Local => "local",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    /// A class name: the class declaration, a type, or the receiver of a
    /// function/constructor call such as `Math` in `Math.abs(x)`.
    Class,
    /// A subroutine name. `kind` is known for subroutines of the class being
    /// analysed and for the OS; otherwise it is `None`.
    Subroutine {
        class: String,
        kind: Option<SubroutineKind>,
    },
    /// A variable with its declared type, VM segment and index.
    Variable {
        kind: VarKind,
        ty: String,
        index: u16,
    },
    /// An identifier used as a variable that is not declared anywhere visible.
    Undefined,
}

/// One identifier occurrence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub span: Span,
    pub name: String,
    pub role: Role,
    /// Whether this occurrence is the declaration itself.
    pub is_declaration: bool,
    /// The declaration's name span, when it is in this file.
    pub declaration: Option<Span>,
}

/// All identifier occurrences of one file, sorted by position.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Symbols {
    pub symbols: Vec<Symbol>,
}

impl Symbols {
    /// The occurrence covering `offset` (a cursor directly after an identifier
    /// still counts).
    pub fn at(&self, offset: usize) -> Option<&Symbol> {
        let i = self.symbols.partition_point(|s| s.span.end < offset);
        self.symbols.get(i).filter(|s| s.span.contains(offset))
    }

    /// Every occurrence that refers to the same declaration as `symbol`
    /// (declaration included), for references and rename.
    pub fn references<'a>(&'a self, symbol: &'a Symbol) -> impl Iterator<Item = &'a Symbol> + 'a {
        self.symbols.iter().filter(move |s| match (&symbol.role, &s.role) {
            (Role::Class, Role::Class) => s.name == symbol.name,
            (Role::Subroutine { class: a, .. }, Role::Subroutine { class: b, .. }) => {
                a == b && s.name == symbol.name
            }
            (Role::Variable { .. }, Role::Variable { .. }) => {
                symbol.declaration.is_some() && s.declaration == symbol.declaration
            }
            (Role::Undefined, Role::Undefined) => s.name == symbol.name,
            _ => false,
        })
    }
}
