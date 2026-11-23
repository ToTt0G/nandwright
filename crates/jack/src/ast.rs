//! The Jack syntax tree. Every node carries a byte-offset [`Span`]; parts the
//! parser could not find in a half-typed file are `None` (or
//! [`Term::Missing`]) so the rest of the tree stays usable.

use n2t_core::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Class {
    pub name: Option<Ident>,
    pub vars: Vec<ClassVarDec>,
    pub subroutines: Vec<Subroutine>,
    /// From `class` to the closing brace (or the last token parsed).
    pub span: Span,
    /// The `/** … */` comment directly before `class`, if any.
    pub doc: Option<Span>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClassVarKind {
    Static,
    Field,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassVarDec {
    pub kind: ClassVarKind,
    pub ty: Option<Type>,
    pub names: Vec<Ident>,
    pub span: Span,
    pub doc: Option<Span>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeKind {
    Int,
    Char,
    Boolean,
    /// Any class name, including `Array` and `String`.
    Class(String),
}

impl TypeKind {
    pub fn name(&self) -> &str {
        match self {
            TypeKind::Int => "int",
            TypeKind::Char => "char",
            TypeKind::Boolean => "boolean",
            TypeKind::Class(c) => c,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Type {
    pub kind: TypeKind,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubroutineKind {
    Constructor,
    Function,
    Method,
}

impl SubroutineKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SubroutineKind::Constructor => "constructor",
            SubroutineKind::Function => "function",
            SubroutineKind::Method => "method",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReturnType {
    Void(Span),
    Type(Type),
}

impl ReturnType {
    pub fn span(&self) -> Span {
        match self {
            ReturnType::Void(s) => *s,
            ReturnType::Type(t) => t.span,
        }
    }
    pub fn name(&self) -> &str {
        match self {
            ReturnType::Void(_) => "void",
            ReturnType::Type(t) => t.kind.name(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subroutine {
    pub kind: SubroutineKind,
    pub kind_span: Span,
    pub return_type: Option<ReturnType>,
    pub name: Option<Ident>,
    pub params: Vec<Param>,
    pub locals: Vec<VarDec>,
    pub statements: Vec<Statement>,
    /// The opening brace of the body, if present.
    pub body_open: Option<Span>,
    /// The closing brace of the body, if present.
    pub body_close: Option<Span>,
    pub span: Span,
    pub doc: Option<Span>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub ty: Option<Type>,
    pub name: Option<Ident>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VarDec {
    pub ty: Option<Type>,
    pub names: Vec<Ident>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub statements: Vec<Statement>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Statement {
    Let {
        name: Ident,
        index: Option<Expr>,
        value: Option<Expr>,
        semicolon: Option<Span>,
        span: Span,
    },
    If {
        cond: Option<Expr>,
        then_block: Block,
        else_block: Option<Block>,
        span: Span,
    },
    While {
        cond: Option<Expr>,
        body: Block,
        span: Span,
    },
    Do {
        call: Option<Call>,
        span: Span,
    },
    Return {
        value: Option<Expr>,
        semicolon: Option<Span>,
        span: Span,
    },
}

impl Statement {
    pub fn span(&self) -> Span {
        match self {
            Statement::Let { span, .. }
            | Statement::If { span, .. }
            | Statement::While { span, .. }
            | Statement::Do { span, .. }
            | Statement::Return { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    And,
    Or,
    Lt,
    Gt,
    Eq,
}

impl BinOp {
    pub fn from_symbol(c: u8) -> Option<BinOp> {
        Some(match c {
            b'+' => BinOp::Add,
            b'-' => BinOp::Sub,
            b'*' => BinOp::Mul,
            b'/' => BinOp::Div,
            b'&' => BinOp::And,
            b'|' => BinOp::Or,
            b'<' => BinOp::Lt,
            b'>' => BinOp::Gt,
            b'=' => BinOp::Eq,
            _ => return None,
        })
    }
    pub fn symbol(self) -> char {
        match self {
            BinOp::Add => '+',
            BinOp::Sub => '-',
            BinOp::Mul => '*',
            BinOp::Div => '/',
            BinOp::And => '&',
            BinOp::Or => '|',
            BinOp::Lt => '<',
            BinOp::Gt => '>',
            BinOp::Eq => '=',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    Neg,
    Not,
}

/// `term (op term)*`, evaluated strictly left to right (Jack has no precedence).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expr {
    pub first: Term,
    pub rest: Vec<(BinOp, Span, Term)>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeywordConst {
    True,
    False,
    Null,
    This,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Term {
    /// The value is the official compiler's (truncated) value; it may be out of range.
    Int { value: i32, span: Span },
    /// UTF-16 code units after escape processing.
    Str { units: Vec<u16>, span: Span },
    Keyword { value: KeywordConst, span: Span },
    Var(Ident),
    Index {
        name: Ident,
        index: Box<Expr>,
        span: Span,
    },
    Call(Call),
    Paren { expr: Box<Expr>, span: Span },
    Unary {
        op: UnaryOp,
        op_span: Span,
        term: Box<Term>,
        span: Span,
    },
    /// Nothing usable was found where a term was expected.
    Missing(Span),
}

impl Term {
    pub fn span(&self) -> Span {
        match self {
            Term::Int { span, .. }
            | Term::Str { span, .. }
            | Term::Keyword { span, .. }
            | Term::Index { span, .. }
            | Term::Paren { span, .. }
            | Term::Unary { span, .. }
            | Term::Missing(span) => *span,
            Term::Var(id) => id.span,
            Term::Call(c) => c.span,
        }
    }
}

/// `name(args)` or `receiver.name(args)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub receiver: Option<Ident>,
    pub name: Ident,
    pub args: Vec<Expr>,
    /// Spans of the parentheses, if present.
    pub open: Option<Span>,
    pub close: Option<Span>,
    pub span: Span,
}
