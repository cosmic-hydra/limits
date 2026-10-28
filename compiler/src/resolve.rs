//! Static checks run by `lim check` (and before `lim run`): report every
//! undefined name, undeclared assignment, stray break/continue, and duplicate
//! parameter in one pass, so an agent can fix them all at once.
//!
//! Names declared anywhere in a block are visible throughout it (and in nested
//! functions), because function bodies look names up at call time. Use-before-
//! definition in straight-line code is therefore left to the runtime.

use std::collections::HashSet;

use crate::ast::*;
use crate::error::{Kind, LimError, Span};
use crate::interp::BUILTINS;

pub fn check(prog: &Program) -> Vec<LimError> {
    let mut r = Resolver {
        scopes: vec![BUILTINS.iter().map(|b| b.to_string()).collect()],
        loops: 0,
        errors: Vec::new(),
    };
    r.block(prog, Vec::new());
    r.errors
}

struct Resolver {
    scopes: Vec<HashSet<String>>,
    loops: usize,
    errors: Vec<LimError>,
}

impl Resolver {
    fn err(&mut self, msg: String, span: Span) {
        self.errors.push(LimError::new(Kind::Check, msg, span));
    }

    fn known(&self, name: &str) -> bool {
        self.scopes.iter().any(|s| s.contains(name))
    }

    /// Check a block whose scope starts with `pre` (params or loop variable).
    fn block(&mut self, stmts: &[Stmt], pre: Vec<String>) {
        let mut scope: HashSet<String> = pre.into_iter().collect();
        for s in stmts {
            match s {
                Stmt::Let(n, ..) | Stmt::Fn(n, ..) => {
                    scope.insert(n.clone());
                }
                _ => {}
            }
        }
        self.scopes.push(scope);
        for s in stmts {
            self.stmt(s);
        }
        self.scopes.pop();
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let(_, e, _) | Stmt::Expr(e, _) => self.expr(e),
            Stmt::Assign(target, v, _) => {
                match target {
                    Expr::Var(n, sp) => {
                        if !self.known(n) {
                            self.err(
                                format!("assignment to undeclared variable '{}' (use 'let')", n),
                                *sp,
                            );
                        }
                    }
                    other => self.expr(other),
                }
                self.expr(v);
            }
            Stmt::If(c, then, els, _) => {
                self.expr(c);
                self.block(then, Vec::new());
                if let Some(b) = els {
                    self.block(b, Vec::new());
                }
            }
            Stmt::While(c, body, _) => {
                self.expr(c);
                self.loops += 1;
                self.block(body, Vec::new());
                self.loops -= 1;
            }
            Stmt::For(n, it, body, _) => {
                self.expr(it);
                self.loops += 1;
                self.block(body, vec![n.clone()]);
                self.loops -= 1;
            }
            Stmt::Fn(name, params, body, sp) => {
                let mut seen = HashSet::new();
                for p in params {
                    if !seen.insert(p) {
                        self.err(format!("duplicate parameter '{}' in fn {}", p, name), *sp);
                    }
                }
                // A loop outside the function does not make break legal inside it.
                let outer = std::mem::replace(&mut self.loops, 0);
                self.block(body, params.clone());
                self.loops = outer;
            }
            Stmt::Return(e, _) => {
                if let Some(e) = e {
                    self.expr(e);
                }
            }
            Stmt::Break(sp) | Stmt::Continue(sp) => {
                if self.loops == 0 {
                    let kw = if matches!(s, Stmt::Break(_)) {
                        "break"
                    } else {
                        "continue"
                    };
                    self.err(format!("'{}' outside of a loop", kw), *sp);
                }
            }
        }
    }

    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Nil => {}
            Expr::Var(n, sp) => {
                if !self.known(n) {
                    self.err(format!("undefined variable '{}'", n), *sp);
                }
            }
            Expr::List(items) => items.iter().for_each(|i| self.expr(i)),
            Expr::Map(pairs, _) => {
                for (k, v) in pairs {
                    self.expr(k);
                    self.expr(v);
                }
            }
            Expr::Unary(_, x, _) => self.expr(x),
            Expr::Binary(_, a, b, _) | Expr::Range(a, b, _) | Expr::Index(a, b, _) => {
                self.expr(a);
                self.expr(b);
            }
            Expr::Call(f, args, _) => {
                self.expr(f);
                args.iter().for_each(|a| self.expr(a));
            }
        }
    }
}
