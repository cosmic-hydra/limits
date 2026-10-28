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
            Ty::Struct(n) => write!(f, "{}", n),
            Ty::Err => write!(f, "<error>"),
        }
    }
}

/// A type as written in source; resolved by the checker.
#[derive(Debug, Clone)]
pub enum TyExpr {
    Name(String, u32, u32),
    List(Box<TyExpr>),
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
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Let(String, Option<TyExpr>, Expr, u32, u32),
    Assign(Expr, Expr),
    Expr(Expr),
    If(Vec<(Expr, Vec<Stmt>)>, Option<Vec<Stmt>>),
    While(Expr, Vec<Stmt>),
    ForRange(String, Expr, Expr, Vec<Stmt>),
    ForList(String, Expr, Vec<Stmt>),
    Break(u32, u32),
    Continue(u32, u32),
    Return(Option<Expr>, u32, u32),
}

#[derive(Debug, Clone)]
pub struct Func {
    pub name: String,
    /// `pub`: visible to importing modules.
    pub public: bool,
    pub params: Vec<(String, TyExpr)>,
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

#[derive(Debug, Clone, Default)]
pub struct Program {
    /// Dotted import paths (`util.strings`) with their position.
    pub imports: Vec<(String, u32, u32)>,
    /// Capabilities from `uses a, b.c;` with their position.
    pub uses: Vec<(String, u32, u32)>,
    pub funcs: Vec<Func>,
    pub structs: Vec<Struct>,
    /// Display path of each module's file ("" for the root), by module index.
    pub files: Vec<String>,
}
