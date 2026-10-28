//! Syntax tree. `Expr::ty` is filled in by the checker and read by codegen.

#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    Int,
    Float,
    Bool,
    Str,
    Unit,
    List(Box<Ty>),
    Struct(String),
    /// A user `enum` (qualified name).
    Enum(String),
    /// Built-in generic sum types, monomorphized in C.
    Option(Box<Ty>),
    Result(Box<Ty>, Box<Ty>),
    /// A capability value (`Env`, `Fs`, ...): passed as a parameter, never
    /// stored in a struct, list or sum type.
    Cap(Cap),
    /// Result of an earlier error; compatible with everything to avoid cascades.
    Err,
}

impl std::fmt::Display for Ty {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Ty::Int => write!(f, "int"),
            Ty::Float => write!(f, "float"),
            Ty::Bool => write!(f, "bool"),
            Ty::Str => write!(f, "str"),
            Ty::Unit => write!(f, "unit"),
            Ty::List(t) => write!(f, "[{}]", t),
            Ty::Struct(n) | Ty::Enum(n) => write!(f, "{}", n),
            Ty::Option(t) => write!(f, "Option[{}]", t),
            Ty::Result(t, e) => write!(f, "Result[{}, {}]", t, e),
            Ty::Cap(c) => write!(f, "{}", c.name()),
            Ty::Err => write!(f, "<error>"),
        }
    }
}

/// Capability types. `Env` is what the program's entry receives; its fields
/// are the narrower capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cap {
    Env,
    Fs,
    Vars,
    Clock,
    Args,
    Stdin,
}

impl Cap {
    pub const ALL: [Cap; 6] = [
        Cap::Env,
        Cap::Fs,
        Cap::Vars,
        Cap::Clock,
        Cap::Args,
        Cap::Stdin,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Cap::Env => "Env",
            Cap::Fs => "Fs",
            Cap::Vars => "Vars",
            Cap::Clock => "Clock",
            Cap::Args => "Args",
            Cap::Stdin => "Stdin",
        }
    }

    /// `env.<field>`: the capability fields of `Env`.
    pub fn env_field(f: &str) -> Option<Cap> {
        Some(match f {
            "fs" => Cap::Fs,
            "vars" => Cap::Vars,
            "clock" => Cap::Clock,
            "args" => Cap::Args,
            "stdin" => Cap::Stdin,
            _ => return None,
        })
    }
}

/// Does a value of this type contain a capability?
pub fn has_cap(t: &Ty) -> bool {
    match t {
        Ty::Cap(_) => true,
        Ty::List(x) | Ty::Option(x) => has_cap(x),
        Ty::Result(a, b) => has_cap(a) || has_cap(b),
        _ => false,
    }
}

/// A type as written in source; resolved by the checker.
#[derive(Debug, Clone)]
pub enum TyExpr {
    Name(String, u32, u32),
    List(Box<TyExpr>),
    /// `Option[T]` / `Result[T, E]`.
    Generic(String, Vec<TyExpr>, u32, u32),
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub line: u32,
    pub col: u32,
    pub ty: Ty,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Var(String),
    List(Vec<Expr>),
    StructLit(String, Vec<(String, Expr)>),
    Unary(&'static str, Box<Expr>),
    Binary(&'static str, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    Index(Box<Expr>, Box<Expr>),
    Field(Box<Expr>, String),
    /// `a..b`: a half-open int range. As an expression it builds an `[int]`;
    /// `for` loops and comprehensions iterate it without allocating.
    Range(Box<Expr>, Box<Expr>),
    /// `[elem for x in xs if cond ...]`: clauses nest left to right.
    Comp(Box<Expr>, Vec<Clause>),
    /// A sum-type value, built by the checker from a call (`Circle(1.0)`,
    /// `Some(x)`) or a name (`None`, `Shape.Empty`): variant index and
    /// payload. The String is the enum (`Option`, `Result` or a qualified
    /// user enum); the instance is identified by `Expr::ty`.
    Variant(String, usize, Vec<Expr>),
    /// `match e:` used as a value (right side of `let`/`=`/`return`). Each
    /// arm ends in an expression statement or leaves the function/loop.
    Match(Box<Expr>, Vec<Arm>),
    /// Postfix `e?` on an Option/Result: unwrap or return early.
    Try(Box<Expr>),
    /// `&place` in a call argument: passes a variable (or a field or
    /// element of one) to an `inout` parameter, which may change it.
    Ref(Box<Expr>),
    /// `recv.name(args)`: a method call on a capability value (`fs.read(p)`).
    /// The checker lowers it to a `Call` of an internal `$...` builtin with
    /// the receiver as first argument.
    Method(Box<Expr>, String, Vec<Expr>),
}

#[derive(Debug, Clone)]
pub enum Pat {
    Wild,
    Int(i64),
    Str(String),
    Bool(bool),
    /// Variant name as written (maybe qualified), binders (`_` ignores), and
    /// the variant index (set by the checker).
    Variant(String, Vec<String>, usize),
}

#[derive(Debug, Clone)]
pub struct Arm {
    pub pat: Pat,
    pub body: Vec<Stmt>,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone)]
pub enum Clause {
    For(String, Expr),
    If(Expr),
}

/// How a name was bound; only `var` bindings may be reassigned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Bind {
    Var,
    Let,
    Param,
    /// An `inout` parameter: may be changed (the change is seen by the caller).
    Inout,
    Loop,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    /// `let` (immutable) or `var` (mutable, `true`) binding.
    Let(String, Option<TyExpr>, Expr, u32, u32, bool),
    /// `t = v`, or `t op= v` with the operator and its position.
    Assign(Expr, Option<(&'static str, u32, u32)>, Expr),
    Pass,
    Expr(Expr),
    If(Vec<(Expr, Vec<Stmt>)>, Option<Vec<Stmt>>),
    While(Expr, Vec<Stmt>),
    ForRange(String, Expr, Expr, Vec<Stmt>),
    ForList(String, Expr, Vec<Stmt>),
    Break(u32, u32),
    Continue(u32, u32),
    Return(Option<Expr>, u32, u32),
    Match(Expr, Vec<Arm>),
}

#[derive(Debug, Clone)]
pub struct Func {
    pub name: String,
    /// `pub`: visible to importing modules.
    pub public: bool,
    pub params: Vec<(String, TyExpr)>,
    /// Per parameter: declared `inout` (`xs: inout [int]`)?
    pub inout: Vec<bool>,
    pub ret: Option<TyExpr>,
    pub body: Vec<Stmt>,
    /// Contract clauses: `requires e;` before the body, `ensures e;` (may use
    /// `result`) checked on every return. Empty for most functions.
    pub requires: Vec<Expr>,
    pub ensures: Vec<Expr>,
    /// `test "name" { }` blocks are parameterless functions with
    /// this display name (qualified by module after checking).
    pub test: Option<String>,
    /// Index of the defining module (set by the checker; 0 is the root file).
    pub file: usize,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone)]
pub struct Struct {
    pub name: String,
    /// `pub`: visible to importing modules.
    pub public: bool,
    pub fields: Vec<(String, TyExpr)>,
    pub line: u32,
    pub col: u32,
}

/// `enum Name:` with one variant per line; a variant has zero or more
/// named payload fields.
#[derive(Debug, Clone)]
pub struct Enum {
    pub name: String,
    pub public: bool,
    pub variants: Vec<Variant>,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone)]
pub struct Variant {
    pub name: String,
    pub fields: Vec<(String, TyExpr)>,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone, Default)]
pub struct Program {
    /// Dotted import paths (`util.strings`) with their position.
    pub imports: Vec<(String, u32, u32)>,
    /// Capabilities from `uses a, b.c;` with their position.
    pub uses: Vec<(String, u32, u32)>,
    pub funcs: Vec<Func>,
    pub structs: Vec<Struct>,
    pub enums: Vec<Enum>,
    /// Top-level statements (script mode): the body of the implicit `main`.
    pub top: Vec<Stmt>,
    /// Position of the first top-level statement.
    pub top_pos: (u32, u32),
    /// Display path of each module's file ("" for the root), by module index.
    pub files: Vec<String>,
}
