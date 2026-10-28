use crate::error::Span;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Nil,
    Var(String, Span),
    List(Vec<Expr>),
    Map(Vec<(Expr, Expr)>, Span),
    Unary(UnOp, Box<Expr>, Span),
    Binary(BinOp, Box<Expr>, Box<Expr>, Span),
    Range(Box<Expr>, Box<Expr>, Span),
    Call(Box<Expr>, Vec<Expr>, Span),
    Index(Box<Expr>, Box<Expr>, Span),
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Let(String, Expr, Span),
    Assign(Expr, Expr, Span),
    Expr(Expr, Span),
    If(Expr, Vec<Stmt>, Option<Vec<Stmt>>, Span),
    While(Expr, Vec<Stmt>, Span),
    For(String, Expr, Vec<Stmt>, Span),
    Fn(String, Vec<String>, Vec<Stmt>, Span),
    Return(Option<Expr>, Span),
    Break(Span),
    Continue(Span),
}

pub type Program = Vec<Stmt>;

/// Validate caller-built ASTs before recursive compilation or AST cloning.
/// Parsed programs have stricter expression and grammar guards already.
pub(crate) fn validate(program: &Program) -> Result<(), crate::error::LimError> {
    use crate::error::{Kind, LimError};
    enum Node<'a> {
        Stmt(&'a Stmt),
        Expr(&'a Expr),
    }
    let initial = Span { line: 1, col: 1 };
    let mut work: Vec<_> = program
        .iter()
        .map(|s| (Node::Stmt(s), 0, initial))
        .collect();
    while let Some((node, depth, span)) = work.pop() {
        if depth > 256 {
            return Err(LimError::new(
                Kind::Parse,
                "AST nesting limit exceeded (256)",
                span,
            ));
        }
        match node {
            Node::Stmt(s) => {
                let span = crate::interp::stmt_span(s);
                match s {
                    Stmt::Let(_, e, _) | Stmt::Expr(e, _) => {
                        work.push((Node::Expr(e), depth + 1, span))
                    }
                    Stmt::Assign(a, b, _) => {
                        if !matches!(a, Expr::Var(..) | Expr::Index(..)) {
                            return Err(LimError::new(
                                Kind::Parse,
                                "invalid assignment target",
                                span,
                            ));
                        }
                        work.push((Node::Expr(a), depth + 1, span));
                        work.push((Node::Expr(b), depth + 1, span));
                    }
                    Stmt::If(e, a, b, _) => {
                        work.push((Node::Expr(e), depth + 1, span));
                        work.extend(a.iter().map(|s| (Node::Stmt(s), depth + 1, span)));
                        if let Some(b) = b {
                            work.extend(b.iter().map(|s| (Node::Stmt(s), depth + 1, span)));
                        }
                    }
                    Stmt::For(_, e, b, _) | Stmt::While(e, b, _) => {
                        work.push((Node::Expr(e), depth + 1, span));
                        work.extend(b.iter().map(|s| (Node::Stmt(s), depth + 1, span)));
                    }
                    Stmt::Fn(_, _, b, _) => {
                        work.extend(b.iter().map(|s| (Node::Stmt(s), depth + 1, span)))
                    }
                    Stmt::Return(Some(e), _) => work.push((Node::Expr(e), depth + 1, span)),
                    _ => {}
                }
            }
            Node::Expr(e) => match e {
                Expr::List(es) => work.extend(es.iter().map(|e| (Node::Expr(e), depth + 1, span))),
                Expr::Map(es, s) => {
                    for (a, b) in es {
                        work.push((Node::Expr(a), depth + 1, *s));
                        work.push((Node::Expr(b), depth + 1, *s));
                    }
                }
                Expr::Unary(_, e, s) => work.push((Node::Expr(e), depth + 1, *s)),
                Expr::Binary(_, a, b, s) | Expr::Range(a, b, s) | Expr::Index(a, b, s) => {
                    work.push((Node::Expr(a), depth + 1, *s));
                    work.push((Node::Expr(b), depth + 1, *s));
                }
                Expr::Call(f, args, s) => {
                    work.push((Node::Expr(f), depth + 1, *s));
                    work.extend(args.iter().map(|e| (Node::Expr(e), depth + 1, *s)));
                }
                _ => {}
            },
        }
    }
    Ok(())
}
