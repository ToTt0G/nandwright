//! Semantic analysis and VM code generation for one class, in a single pass
//! that mirrors the official compiler: the same symbol rules, the same
//! (unusual) operand type checks, the same reachability analysis and
//! byte-for-byte the same VM code.

use n2t_core::Span;

use crate::ast::*;
use crate::diag::{JDiag, codes, error, warning};
use crate::lexer::Token;
use crate::symbols::{Role, Symbol, VarKind};

/// Maps token start offsets to the line the official compiler reports.
pub(crate) struct JavaLines {
    starts: Vec<(usize, u32)>,
    eof: u32,
}

impl JavaLines {
    pub fn new(tokens: &[Token], eof: u32) -> Self {
        JavaLines {
            starts: tokens.iter().map(|t| (t.span.start, t.java_line)).collect(),
            eof,
        }
    }
    /// The official line of the token starting at (or containing) `offset`.
    pub fn at(&self, offset: usize) -> u32 {
        let i = self.starts.partition_point(|(s, _)| *s <= offset);
        if i == 0 {
            return self.starts.first().map_or(self.eof, |s| s.1);
        }
        self.starts[i - 1].1
    }
}

/// A call that can only be checked once the callee's class is known.
#[derive(Debug, Clone)]
pub(crate) struct DeferredCall {
    pub class: String,
    pub name: String,
    pub name_span: Span,
    /// `Class.f()` (true) versus `f()` / `var.f()` (false).
    pub static_form: bool,
    pub args: usize,
    pub java_line: u32,
    /// The receiver identifier when it is a field used inside a function
    /// (the official compiler then treats it as a class name).
    pub field_in_function: bool,
}

/// The signature table of one class, used by the call checks.
#[derive(Debug, Clone, Default)]
pub(crate) struct ClassSig {
    pub name: String,
    pub subs: Vec<SubSig>,
}

#[derive(Debug, Clone)]
pub(crate) struct SubSig {
    pub name: String,
    pub kind: SubroutineKind,
    pub params: usize,
    pub span: Span,
}

pub(crate) struct Analysis {
    pub diags: Vec<JDiag>,
    /// VM commands and, for each, the byte offset of the Jack code that produced it.
    pub vm: Vec<String>,
    pub vm_offsets: Vec<usize>,
    pub deferred: Vec<DeferredCall>,
    pub symbols: Vec<Symbol>,
    pub sig: ClassSig,
}

/// The type the official compiler infers for operands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JType {
    /// Objects, calls, array elements, parenthesised expressions: never checked.
    Any,
    IntConst,
    StrConst,
    BoolConst,
    Null,
    This,
    Int,
    Char,
    Boolean,
}

impl JType {
    fn describe(self) -> &'static str {
        match self {
            JType::Any => "a value",
            JType::IntConst => "an integer constant",
            JType::StrConst => "a string constant",
            JType::BoolConst => "`true`/`false`",
            JType::Null => "`null`",
            JType::This => "`this`",
            JType::Int => "an int value",
            JType::Char => "a char value",
            JType::Boolean => "a boolean value",
        }
    }
}

#[derive(Clone, Copy)]
struct TermType {
    ty: JType,
    /// The term is a variable, array element or call (possibly under unary
    /// operators): the official compiler rejects those after a string constant.
    ident: bool,
    /// The token the official compiler reports operand errors at.
    primary: Span,
}

#[derive(Debug, Clone)]
struct Var {
    name: String,
    kind: VarKind,
    ty: TypeKind,
    index: u16,
    decl: Span,
}

fn var_jtype(t: &TypeKind) -> JType {
    match t {
        TypeKind::Int => JType::Int,
        TypeKind::Char => JType::Char,
        TypeKind::Boolean => JType::Boolean,
        TypeKind::Class(_) => JType::Any,
    }
}

struct Checker<'a> {
    lines: &'a JavaLines,
    class_name: String,
    fields: Vec<Var>,
    statics: Vec<Var>,
    sigs: Vec<SubSig>,
    diags: Vec<JDiag>,
    deferred: Vec<DeferredCall>,
    symbols: Vec<Symbol>,
    vm: Vec<String>,
    vm_offsets: Vec<usize>,
    // Per subroutine.
    sub_kind: SubroutineKind,
    sub_name: String,
    is_void: bool,
    args: Vec<Var>,
    locals: Vec<Var>,
    if_count: u32,
    while_count: u32,
    returned: bool,
}

pub(crate) fn analyze(class: &Class, lines: &JavaLines) -> Analysis {
    let class_name = class
        .name
        .as_ref()
        .map_or_else(String::new, |n| n.name.clone());
    let mut c = Checker {
        lines,
        class_name: class_name.clone(),
        fields: Vec::new(),
        statics: Vec::new(),
        sigs: Vec::new(),
        diags: Vec::new(),
        deferred: Vec::new(),
        symbols: Vec::new(),
        vm: Vec::new(),
        vm_offsets: Vec::new(),
        sub_kind: SubroutineKind::Function,
        sub_name: String::new(),
        is_void: false,
        args: Vec::new(),
        locals: Vec::new(),
        if_count: 0,
        while_count: 0,
        returned: false,
    };
    if let Some(n) = &class.name {
        c.symbols.push(Symbol {
            span: n.span,
            name: n.name.clone(),
            role: Role::Class,
            is_declaration: true,
            declaration: Some(n.span),
        });
    }
    for dec in &class.vars {
        let (kind, list_len) = match dec.kind {
            ClassVarKind::Field => (VarKind::Field, c.fields.len()),
            ClassVarKind::Static => (VarKind::Static, c.statics.len()),
        };
        let Some(ty) = &dec.ty else { continue };
        c.type_symbol(ty);
        for (i, n) in dec.names.iter().enumerate() {
            let v = Var {
                name: n.name.clone(),
                kind,
                ty: ty.kind.clone(),
                index: u16::try_from(list_len + i).unwrap_or(u16::MAX),
                decl: n.span,
            };
            c.declare(&v, n.span);
            match kind {
                VarKind::Field => c.fields.push(v),
                _ => c.statics.push(v),
            }
        }
    }
    // Signatures: the official compiler knows every subroutine of the class
    // when it checks calls, and reports a second declaration of a name.
    for sub in &class.subroutines {
        let Some(name) = &sub.name else { continue };
        if let Some(prev) = c.sigs.iter().find(|s| s.name == name.name) {
            let d = error(
                codes::DUPLICATE_SUBROUTINE,
                format!(
                    "Subroutine `{}` is declared twice in class `{}`. Rename or remove one of them.",
                    name.name, class_name
                ),
                name.span,
                lines.at(name.span.start),
            )
            .related("first declared here", prev.span);
            c.diags.push(d);
            continue;
        }
        c.sigs.push(SubSig {
            name: name.name.clone(),
            kind: sub.kind,
            params: sub.params.len(),
            span: name.span,
        });
    }
    for sub in &class.subroutines {
        c.subroutine(sub, class);
    }
    Analysis {
        diags: c.diags,
        vm: c.vm,
        vm_offsets: c.vm_offsets,
        deferred: c.deferred,
        symbols: c.symbols,
        sig: ClassSig {
            name: class_name,
            subs: c.sigs,
        },
    }
}

impl Checker<'_> {
    fn emit(&mut self, at: usize, cmd: impl Into<String>) {
        self.vm.push(cmd.into());
        self.vm_offsets.push(at);
    }

    fn line(&self, offset: usize) -> u32 {
        self.lines.at(offset)
    }

    fn declare(&mut self, v: &Var, span: Span) {
        self.symbols.push(Symbol {
            span,
            name: v.name.clone(),
            role: Role::Variable {
                kind: v.kind,
                ty: v.ty.name().to_string(),
                index: v.index,
            },
            is_declaration: true,
            declaration: Some(v.decl),
        });
    }

    fn type_symbol(&mut self, ty: &Type) {
        if let TypeKind::Class(name) = &ty.kind {
            let decl = (*name == self.class_name)
                .then(|| self.symbols.first().map(|s| s.span))
                .flatten();
            self.symbols.push(Symbol {
                span: ty.span,
                name: name.clone(),
                role: Role::Class,
                is_declaration: false,
                declaration: decl,
            });
        }
    }

    /// Looks a name up the way the official compiler does: arguments, locals,
    /// fields (not inside functions), statics; the latest declaration wins.
    fn lookup(&self, name: &str) -> Option<Var> {
        let find = |list: &[Var]| list.iter().rev().find(|v| v.name == name).cloned();
        find(&self.args)
            .or_else(|| find(&self.locals))
            .or_else(|| {
                if self.sub_kind == SubroutineKind::Function {
                    None
                } else {
                    find(&self.fields)
                }
            })
            .or_else(|| find(&self.statics))
    }

    fn field_named(&self, name: &str) -> bool {
        self.fields.iter().any(|v| v.name == name)
    }

    fn use_var(&mut self, id: &Ident, v: &Var) {
        self.symbols.push(Symbol {
            span: id.span,
            name: id.name.clone(),
            role: Role::Variable {
                kind: v.kind,
                ty: v.ty.name().to_string(),
                index: v.index,
            },
            is_declaration: false,
            declaration: Some(v.decl),
        });
    }

    fn undefined(&mut self, id: &Ident) {
        self.symbols.push(Symbol {
            span: id.span,
            name: id.name.clone(),
            role: Role::Undefined,
            is_declaration: false,
            declaration: None,
        });
        if id.name.is_empty() {
            return;
        }
        let msg = if self.sub_kind == SubroutineKind::Function && self.field_named(&id.name) {
            format!(
                "`{}` is a field, and a function has no object whose fields it could use. Make \
                 `{}` a method, or pass the value in as a parameter.",
                id.name, self.sub_name
            )
        } else if self.sigs.iter().any(|s| s.name == id.name) {
            format!(
                "`{}` is a subroutine, not a variable. To call it, write `{}(…)`.",
                id.name, id.name
            )
        } else {
            format!(
                "`{}` is not declared. Declare it with `var`, as a parameter, or as a `field` or \
                 `static` of the class.",
                id.name
            )
        };
        self.diags.push(error(
            codes::UNDEFINED_VARIABLE,
            msg,
            id.span,
            self.line(id.span.start),
        ));
    }

    fn push_var(&mut self, at: usize, v: &Var) {
        self.emit(at, format!("push {} {}", v.kind.segment(), v.index));
    }

    // ------------------------------------------------------------- subroutine

    fn subroutine(&mut self, sub: &Subroutine, class: &Class) {
        self.sub_kind = sub.kind;
        self.sub_name = sub.name.as_ref().map_or_else(String::new, |n| n.name.clone());
        self.is_void = sub.kind != SubroutineKind::Constructor
            && matches!(sub.return_type, Some(ReturnType::Void(_)));
        self.args.clear();
        self.locals.clear();
        self.if_count = 0;
        self.while_count = 0;
        self.returned = false;

        if let Some(ReturnType::Type(t)) = &sub.return_type {
            self.type_symbol(t);
        }
        if sub.kind == SubroutineKind::Constructor
            && let Some(rt) = &sub.return_type
            && rt.name() != self.class_name
        {
            let span = rt.span();
            self.diags.push(error(
                codes::CONSTRUCTOR_TYPE,
                format!(
                    "A constructor of class `{}` must return `{}`: write `constructor {} {}(…)`.",
                    self.class_name,
                    self.class_name,
                    self.class_name,
                    if self.sub_name.is_empty() { "new" } else { &self.sub_name }
                ),
                span,
                self.line(span.start),
            ));
        }
        if let Some(n) = &sub.name {
            self.symbols.push(Symbol {
                span: n.span,
                name: n.name.clone(),
                role: Role::Subroutine {
                    class: self.class_name.clone(),
                    kind: Some(sub.kind),
                },
                is_declaration: true,
                declaration: Some(n.span),
            });
        }
        let arg_base = usize::from(sub.kind == SubroutineKind::Method);
        for p in &sub.params {
            let (Some(ty), Some(name)) = (&p.ty, &p.name) else {
                continue;
            };
            self.type_symbol(ty);
            let v = Var {
                name: name.name.clone(),
                kind: VarKind::Argument,
                ty: ty.kind.clone(),
                index: u16::try_from(arg_base + self.args.len()).unwrap_or(u16::MAX),
                decl: name.span,
            };
            self.declare(&v, name.span);
            self.args.push(v);
        }
        for dec in &sub.locals {
            let Some(ty) = &dec.ty else { continue };
            self.type_symbol(ty);
            for n in &dec.names {
                let v = Var {
                    name: n.name.clone(),
                    kind: VarKind::Local,
                    ty: ty.kind.clone(),
                    index: u16::try_from(self.locals.len()).unwrap_or(u16::MAX),
                    decl: n.span,
                };
                self.declare(&v, n.span);
                self.locals.push(v);
            }
        }

        let header_at = sub.name.as_ref().map_or(sub.kind_span.start, |n| n.span.start);
        self.emit(
            header_at,
            format!(
                "function {}.{} {}",
                self.class_name,
                self.sub_name,
                self.locals.len()
            ),
        );
        match sub.kind {
            SubroutineKind::Constructor => {
                let n_fields = class
                    .vars
                    .iter()
                    .filter(|d| d.kind == ClassVarKind::Field && d.ty.is_some())
                    .map(|d| d.names.len())
                    .sum::<usize>();
                self.emit(header_at, format!("push constant {n_fields}"));
                self.emit(header_at, "call Memory.alloc 1");
                self.emit(header_at, "pop pointer 0");
            }
            SubroutineKind::Method => {
                self.emit(header_at, "push argument 0");
                self.emit(header_at, "pop pointer 0");
            }
            SubroutineKind::Function => {}
        }

        self.statements(&sub.statements);

        if !self.returned
            && let Some(close) = sub.body_close
        {
            let d = error(
                codes::MISSING_RETURN,
                if self.is_void {
                    format!(
                        "`{}` can reach its end without a `return`. Every Jack subroutine must end \
                         with a `return;` statement, even a void one.",
                        self.sub_name
                    )
                } else {
                    format!(
                        "`{}` can reach its end without a `return`. Add a `return` statement that \
                         covers every path.",
                        self.sub_name
                    )
                },
                close,
                self.line(close.start),
            );
            self.diags.push(d);
        }
    }

    // ------------------------------------------------------------- statements

    fn statements(&mut self, list: &[Statement]) {
        let start = self.returned;
        let mut warned = false;
        for s in list {
            if self.returned && !warned && !start {
                warned = true;
                let span = s.span();
                self.diags.push(warning(
                    codes::UNREACHABLE,
                    "This code can never run: every path before it has already returned.",
                    span,
                    self.line(span.start),
                ));
            }
            self.statement(s);
        }
    }

    fn statement(&mut self, s: &Statement) {
        match s {
            Statement::Let {
                name,
                index,
                value,
                semicolon,
                span,
            } => {
                let var = self.lookup(&name.name);
                match &var {
                    Some(v) => self.use_var(name, v),
                    None => self.undefined(name),
                }
                if let Some(index) = index {
                    self.expr(index, JType::IntConst);
                    if let Some(v) = &var {
                        self.push_var(name.span.start, v);
                    }
                    self.emit(span.start, "add");
                }
                let vty = value.as_ref().map(|e| self.expr(e, JType::Any));
                if index.is_some() {
                    self.emit(span.start, "pop temp 0");
                    self.emit(span.start, "pop pointer 1");
                    self.emit(span.start, "push temp 0");
                    self.emit(span.start, "pop that 0");
                } else if let Some(v) = &var {
                    self.emit(span.start, format!("pop {} {}", v.kind.segment(), v.index));
                    if let (Some(vty), Some(semi), Some(value)) = (vty, semicolon, value) {
                        self.check_assign(v, vty, value, *semi);
                    }
                }
            }
            Statement::If {
                cond,
                then_block,
                else_block,
                span,
            } => {
                let n = self.if_count;
                self.if_count += 1;
                let at = span.start;
                if let Some(c) = cond {
                    self.expr(c, JType::Any);
                }
                self.emit(at, format!("if-goto IF_TRUE{n}"));
                self.emit(at, format!("goto IF_FALSE{n}"));
                self.emit(at, format!("label IF_TRUE{n}"));
                let before = self.returned;
                self.statements(&then_block.statements);
                match else_block {
                    Some(eb) => {
                        let then_returned = self.returned;
                        self.emit(at, format!("goto IF_END{n}"));
                        self.emit(at, format!("label IF_FALSE{n}"));
                        self.returned = before;
                        self.statements(&eb.statements);
                        self.returned = then_returned && self.returned;
                        self.emit(at, format!("label IF_END{n}"));
                    }
                    None => {
                        self.emit(at, format!("label IF_FALSE{n}"));
                        self.returned = false;
                    }
                }
            }
            Statement::While { cond, body, span } => {
                let n = self.while_count;
                self.while_count += 1;
                let at = span.start;
                self.emit(at, format!("label WHILE_EXP{n}"));
                if let Some(c) = cond {
                    self.expr(c, JType::Any);
                }
                self.emit(at, "not");
                self.emit(at, format!("if-goto WHILE_END{n}"));
                self.statements(&body.statements);
                self.emit(at, format!("goto WHILE_EXP{n}"));
                self.emit(at, format!("label WHILE_END{n}"));
            }
            Statement::Do { call, span } => {
                if let Some(c) = call {
                    self.call(c);
                }
                self.emit(span.start, "pop temp 0");
            }
            Statement::Return {
                value,
                semicolon,
                span,
            } => {
                match value {
                    Some(e) => {
                        let first = e.first.span();
                        if self.is_void {
                            self.diags.push(error(
                                codes::VOID_RETURN_VALUE,
                                format!(
                                    "`{}` is declared `void`, so it cannot return a value. Write \
                                     `return;` or change the return type.",
                                    self.sub_name
                                ),
                                e.span,
                                self.line(first.start),
                            ));
                        }
                        if self.sub_kind == SubroutineKind::Constructor
                            && !matches!(
                                e.first,
                                Term::Keyword {
                                    value: KeywordConst::This,
                                    ..
                                }
                            )
                        {
                            self.diags.push(error(
                                codes::CONSTRUCTOR_RETURN,
                                "A constructor must end with `return this;`.",
                                e.span,
                                self.line(first.start),
                            ));
                        }
                        self.expr(e, JType::Any);
                    }
                    None => {
                        if let Some(semi) = semicolon {
                            if self.sub_kind == SubroutineKind::Constructor {
                                self.diags.push(error(
                                    codes::CONSTRUCTOR_RETURN,
                                    "A constructor must end with `return this;`.",
                                    *span,
                                    self.line(semi.start),
                                ).at_order(semi.start));
                            }
                            if !self.is_void {
                                let ty = if self.sub_kind == SubroutineKind::Constructor {
                                    self.class_name.clone()
                                } else {
                                    String::from("a value")
                                };
                                self.diags.push(error(
                                    codes::MISSING_RETURN_VALUE,
                                    if self.sub_kind == SubroutineKind::Constructor {
                                        format!("A constructor must return the new object (`{ty}`): write `return this;`.")
                                    } else {
                                        format!(
                                            "`{}` is not `void`, so `return` needs a value: \
                                             `return expression;`.",
                                            self.sub_name
                                        )
                                    },
                                    *span,
                                    self.line(semi.start),
                                ).at_order(semi.start));
                            }
                        }
                        self.emit(span.start, "push constant 0");
                    }
                }
                self.emit(span.start, "return");
                self.returned = true;
            }
        }
    }

    fn check_assign(&mut self, v: &Var, ty: JType, value: &Expr, semi: Span) {
        let bad = match &v.ty {
            TypeKind::Int => matches!(
                ty,
                JType::StrConst
                    | JType::BoolConst
                    | JType::Null
                    | JType::This
                    | JType::Char
                    | JType::Boolean
            ),
            TypeKind::Char => matches!(
                ty,
                JType::StrConst
                    | JType::BoolConst
                    | JType::Null
                    | JType::This
                    | JType::Int
                    | JType::Boolean
            ),
            TypeKind::Boolean => {
                matches!(ty, JType::StrConst | JType::Null | JType::This | JType::Char)
            }
            TypeKind::Class(_) => false,
        };
        if !bad {
            return;
        }
        let tname = v.ty.name();
        let hint = match (&v.ty, ty) {
            (TypeKind::Int, JType::Char) | (TypeKind::Char, JType::Int) => {
                " The official compiler does not convert between int and char automatically."
            }
            (_, JType::Null) => " `null` can only be assigned to object variables.",
            _ => "",
        };
        // The official compiler reports this one line above the `;`.
        let line = self.line(semi.start).saturating_sub(1);
        self.diags.push(
            error(
                codes::TYPE_MISMATCH,
                format!(
                    "`{}` is declared `{tname}`, but this expression is {}.{hint}",
                    v.name,
                    ty.describe()
                ),
                value.span,
                line,
            )
            .at_order(semi.start),
        );
    }

    // ------------------------------------------------------------ expressions

    fn expr(&mut self, e: &Expr, seed: JType) -> JType {
        let first = self.term(&e.first);
        let mut t = self.combine(seed, first, None);
        for (op, op_span, term) in &e.rest {
            let r = self.term(term);
            t = self.combine(t, r, Some(*op));
            let cmd = match op {
                BinOp::Add => "add",
                BinOp::Sub => "sub",
                BinOp::Mul => "call Math.multiply 2",
                BinOp::Div => "call Math.divide 2",
                BinOp::And => "and",
                BinOp::Or => "or",
                BinOp::Lt => "lt",
                BinOp::Gt => "gt",
                BinOp::Eq => "eq",
            };
            self.emit(op_span.start, cmd);
        }
        t
    }

    /// The official compiler's operand compatibility rule.
    fn combine(&mut self, t: JType, r: TermType, op: Option<BinOp>) -> JType {
        if t == JType::StrConst && r.ident {
            self.diags.push(error(
                codes::STRING_CAST,
                "The official compiler does not allow a variable, array element or call after a \
                 string constant in an expression. Jack cannot concatenate strings with `+`; use \
                 the `String` methods instead.",
                r.primary,
                self.line(r.primary.start),
            ));
        }
        if t == JType::Any {
            return r.ty;
        }
        if r.ty == JType::Any {
            return t;
        }
        use JType::*;
        let legal = match r.ty {
            IntConst => matches!(t, IntConst | Int | Char),
            StrConst | Null | This => false,
            BoolConst => t == IntConst,
            Int => matches!(t, IntConst | Int),
            Char => matches!(t, IntConst | Char),
            Boolean => matches!(t, IntConst | Boolean | BoolConst),
            Any => true,
        };
        if legal {
            return if t == IntConst { r.ty } else { t };
        }
        let msg = match op {
            None => format!(
                "An array index must be a number, but this is {}.",
                r.ty.describe()
            ),
            Some(op) => format!(
                "{} cannot be used here: the expression before `{}` is {}, and the official \
                 compiler only combines operands of compatible types (an int with an int, a char \
                 with a char, a boolean with a boolean).",
                capitalize(r.ty.describe()),
                op.symbol(),
                t.describe()
            ),
        };
        self.diags.push(error(
            codes::ILLEGAL_OPERAND,
            msg,
            r.primary,
            self.line(r.primary.start),
        ));
        t
    }

    fn term(&mut self, term: &Term) -> TermType {
        match term {
            Term::Int { value, span } => {
                if *value > 32767 {
                    self.diags.push(error(
                        codes::INTEGER_TOO_BIG,
                        format!(
                            "The integer constant `{value}` is too big: Jack constants go up to \
                             32767. (For -32768 write `-32767 - 1`.)"
                        ),
                        *span,
                        self.line(span.start),
                    ));
                }
                self.emit(span.start, format!("push constant {value}"));
                TermType {
                    ty: JType::IntConst,
                    ident: false,
                    primary: *span,
                }
            }
            Term::Str { units, span } => {
                self.emit(span.start, format!("push constant {}", units.len()));
                self.emit(span.start, "call String.new 1");
                for u in units {
                    self.emit(span.start, format!("push constant {}", *u as i16));
                    self.emit(span.start, "call String.appendChar 2");
                }
                TermType {
                    ty: JType::StrConst,
                    ident: false,
                    primary: *span,
                }
            }
            Term::Keyword { value, span } => {
                let ty = match value {
                    KeywordConst::True => {
                        self.emit(span.start, "push constant 0");
                        self.emit(span.start, "not");
                        JType::BoolConst
                    }
                    KeywordConst::False => {
                        self.emit(span.start, "push constant 0");
                        JType::BoolConst
                    }
                    KeywordConst::Null => {
                        self.emit(span.start, "push constant 0");
                        JType::Null
                    }
                    KeywordConst::This => {
                        if self.sub_kind == SubroutineKind::Function {
                            self.diags.push(error(
                                codes::THIS_IN_FUNCTION,
                                format!(
                                    "`this` cannot be used in a function: `{}` is not called on \
                                     an object. Make it a method if it needs one.",
                                    self.sub_name
                                ),
                                *span,
                                self.line(span.start),
                            ));
                        }
                        self.emit(span.start, "push pointer 0");
                        JType::This
                    }
                };
                TermType {
                    ty,
                    ident: false,
                    primary: *span,
                }
            }
            Term::Var(id) => {
                let ty = match self.lookup(&id.name) {
                    Some(v) => {
                        self.use_var(id, &v);
                        self.push_var(id.span.start, &v);
                        var_jtype(&v.ty)
                    }
                    None => {
                        self.undefined(id);
                        JType::Any
                    }
                };
                TermType {
                    ty,
                    ident: true,
                    primary: id.span,
                }
            }
            Term::Index { name, index, span } => {
                let var = self.lookup(&name.name);
                match &var {
                    Some(v) => self.use_var(name, v),
                    None => self.undefined(name),
                }
                self.expr(index, JType::IntConst);
                if let Some(v) = &var {
                    self.push_var(name.span.start, v);
                }
                self.emit(span.start, "add");
                self.emit(span.start, "pop pointer 1");
                self.emit(span.start, "push that 0");
                TermType {
                    ty: JType::Any,
                    ident: true,
                    primary: name.span,
                }
            }
            Term::Call(c) => {
                self.call(c);
                TermType {
                    ty: JType::Any,
                    ident: true,
                    primary: c.receiver.as_ref().map_or(c.name.span, |r| r.span),
                }
            }
            Term::Paren { expr, span } => {
                self.expr(expr, JType::Any);
                TermType {
                    ty: JType::Any,
                    ident: false,
                    primary: *span,
                }
            }
            Term::Unary { op, term, span, .. } => {
                let inner = self.term(term);
                self.emit(
                    span.start,
                    match op {
                        UnaryOp::Neg => "neg",
                        UnaryOp::Not => "not",
                    },
                );
                inner
            }
            Term::Missing(span) => TermType {
                ty: JType::Any,
                ident: false,
                primary: *span,
            },
        }
    }

    fn call(&mut self, c: &Call) {
        let at = c.span.start;
        let n = c.args.len();
        match &c.receiver {
            None => {
                let line = self.line(c.name.span.start);
                if self.sub_kind == SubroutineKind::Function {
                    self.diags.push(error(
                        codes::METHOD_FROM_FUNCTION,
                        format!(
                            "`{}()` without an object calls a method on `this`, but `{}` is a \
                             function and has no `this`. Write `{}.{}(…)` for a function, or \
                             call the method on an object: `obj.{}(…)`.",
                            c.name.name,
                            self.sub_name,
                            self.class_name,
                            c.name.name,
                            c.name.name
                        ),
                        c.name.span,
                        line,
                    ));
                }
                self.sub_symbol(&c.name, &self.class_name.clone());
                self.emit(at, "push pointer 0");
                self.args(c);
                self.emit(
                    c.name.span.start,
                    format!("call {}.{} {}", self.class_name, c.name.name, n + 1),
                );
                self.deferred.push(DeferredCall {
                    class: self.class_name.clone(),
                    name: c.name.name.clone(),
                    name_span: c.name.span,
                    static_form: false,
                    args: n,
                    java_line: line,
                    field_in_function: false,
                });
            }
            Some(r) => {
                let line = self.line(c.name.span.start);
                if let Some(v) = self.lookup(&r.name) {
                    self.use_var(r, &v);
                    let class = v.ty.name().to_string();
                    self.sub_symbol(&c.name, &class);
                    self.push_var(r.span.start, &v);
                    self.args(c);
                    self.emit(
                        c.name.span.start,
                        format!("call {}.{} {}", class, c.name.name, n + 1),
                    );
                    self.deferred.push(DeferredCall {
                        class,
                        name: c.name.name.clone(),
                        name_span: c.name.span,
                        static_form: false,
                        args: n,
                        java_line: line,
                            field_in_function: false,
                    });
                } else {
                    let field_in_function =
                        self.sub_kind == SubroutineKind::Function && self.field_named(&r.name);
                    if field_in_function {
                        self.diags.push(warning(
                            codes::UNDEFINED_VARIABLE,
                            format!(
                                "`{}` is a field, which a function cannot use; the official \
                                 compiler therefore treats `{}` as a class name and calls the \
                                 function `{}.{}`. Make `{}` a method to call the method on the \
                                 field.",
                                r.name, r.name, r.name, c.name.name, self.sub_name
                            ),
                            r.span,
                            0,
                        ));
                    }
                    let decl = (r.name == self.class_name)
                        .then(|| self.symbols.first().map(|s| s.span))
                        .flatten();
                    self.symbols.push(Symbol {
                        span: r.span,
                        name: r.name.clone(),
                        role: Role::Class,
                        is_declaration: false,
                        declaration: decl,
                    });
                    self.sub_symbol(&c.name, &r.name.clone());
                    self.args(c);
                    self.emit(
                        c.name.span.start,
                        format!("call {}.{} {}", r.name, c.name.name, n),
                    );
                    self.deferred.push(DeferredCall {
                        class: r.name.clone(),
                        name: c.name.name.clone(),
                        name_span: c.name.span,
                        static_form: true,
                        args: n,
                        java_line: line,
                            field_in_function,
                    });
                }
            }
        }
    }

    fn args(&mut self, c: &Call) {
        for a in &c.args {
            self.expr(a, JType::Any);
        }
    }

    fn sub_symbol(&mut self, name: &Ident, class: &str) {
        if name.name.is_empty() {
            return;
        }
        let local = (class == self.class_name)
            .then(|| self.sigs.iter().find(|s| s.name == name.name))
            .flatten();
        let os = crate::os::find(class, &name.name);
        let kind = local.map(|s| s.kind).or(os.map(|f| f.kind));
        self.symbols.push(Symbol {
            span: name.span,
            name: name.name.clone(),
            role: Role::Subroutine {
                class: class.to_string(),
                kind,
            },
            is_declaration: false,
            declaration: local.map(|s| s.span),
        });
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
