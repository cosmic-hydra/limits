//! Recursive-descent parser. Stops at the first syntax error.

use crate::ast::*;
use crate::lexer::{Tok, Token};
use crate::Error;

type R<T> = Result<T, Error>;

pub struct Parser {
    toks: Vec<Token>,
    pos: usize,
    /// Struct literals are disabled in `if`/`while`/`for` heads (like Rust).
    no_struct: bool,
    /// Current syntax nesting depth, capped at MAX_DEPTH so later recursive
    /// passes (and the C compiler) cannot overflow their stacks.
    depth: u32,
}

const MAX_DEPTH: u32 = 3000;

pub fn parse(toks: Vec<Token>) -> R<Program> {
    let mut p = Parser {
        toks,
        pos: 0,
        no_struct: false,
        depth: 0,
    };
    let mut prog = Program::default();
    while p.peek() != &Tok::Eof {
        let (line, col) = p.here();
        if p.eat_kw("import") {
            let mut name = p.ident()?;
            while p.eat(".") {
                name = format!("{}.{}", name, p.ident()?);
            }
            p.expect(";")?;
            prog.imports.push((name, line, col));
            continue;
        }
        if matches!(p.peek(), Tok::Ident(s) if s == "uses")
            && matches!(p.toks.get(p.pos + 1).map(|t| &t.tok), Some(Tok::Ident(_)))
        {
            p.next();
            loop {
                let (line, col) = p.here();
                let mut name = p.ident()?;
                while p.eat(".") {
                    name = format!("{}.{}", name, p.ident()?);
                }
                prog.uses.push((name, line, col));
                if !p.eat(",") {
                    break;
                }
            }
            p.expect(";")?;
            continue;
        }
        if matches!(p.peek(), Tok::Ident(s) if s == "test")
            && matches!(p.toks.get(p.pos + 1).map(|t| &t.tok), Some(Tok::Str(_)))
        {
            p.next();
            let Tok::Str(name) = p.next() else {
                unreachable!()
            };
            // Item names cannot start with a digit, so this never collides.
            let n = prog.funcs.iter().filter(|f| f.test.is_some()).count();
            let body = p.block()?;
            prog.funcs.push(Func {
                name: format!("{}test", n),
                public: false,
                params: Vec::new(),
                ret: None,
                body,
                requires: Vec::new(),
                ensures: Vec::new(),
                test: Some(name),
                file: 0,
                line,
                col,
            });
            continue;
        }
        let public = p.eat_kw("pub");
        if p.eat_kw("fn") {
            prog.funcs.push(p.func(public, line, col)?);
        } else if p.eat_kw("struct") {
            prog.structs.push(p.strukt(public, line, col)?);
        } else {
            return Err(p.err("expected 'fn', 'struct' or 'import' at top level"));
        }
    }
    Ok(prog)
}

fn ex(kind: ExprKind, line: u32, col: u32) -> Expr {
    Expr {
        kind,
        line,
        col,
        ty: Ty::Err,
    }
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }
    fn here(&self) -> (u32, u32) {
        let t = &self.toks[self.pos];
        (t.line, t.col)
    }
    fn next(&mut self) -> Tok {
        // Tokens are consumed exactly once, so move them out instead of cloning
        // (the final Eof is never moved past and stays in place).
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
            std::mem::replace(&mut self.toks[self.pos - 1].tok, Tok::Eof)
        } else {
            Tok::Eof
        }
    }
    fn err(&self, msg: &str) -> Error {
        let (l, c) = self.here();
        let found = match self.peek() {
            Tok::Eof => "end of file".to_string(),
            Tok::Int(n) => n.to_string(),
            Tok::Float(f) => f.to_string(),
            Tok::Str(s) => format!("\"{}\"", s),
            Tok::Ident(s) => s.clone(),
            Tok::Kw(s) | Tok::Sym(s) => s.to_string(),
        };
        Error::new("parse", l, c, format!("{}, found '{}'", msg, found))
    }
    fn enter(&mut self) -> R<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.err("nesting too deep"));
        }
        Ok(())
    }
    fn is(&self, s: &str) -> bool {
        matches!(self.peek(), Tok::Sym(x) if *x == s)
    }
    fn eat(&mut self, s: &str) -> bool {
        let y = self.is(s);
        if y {
            self.next();
        }
        y
    }
    fn eat_kw(&mut self, k: &str) -> bool {
        let y = matches!(self.peek(), Tok::Kw(x) if *x == k);
        if y {
            self.next();
        }
        y
    }
    fn expect(&mut self, s: &str) -> R<()> {
        if self.eat(s) {
            Ok(())
        } else {
            Err(self.err(&format!("expected '{}'", s)))
        }
    }
    fn ident(&mut self) -> R<String> {
        match self.peek() {
            Tok::Ident(_) => match self.next() {
                Tok::Ident(s) => Ok(s),
                _ => unreachable!(),
            },
            _ => Err(self.err("expected identifier")),
        }
    }

    fn ty(&mut self) -> R<TyExpr> {
        let (l, c) = self.here();
        if self.eat("[") {
            let t = self.ty()?;
            self.expect("]")?;
            Ok(TyExpr::List(Box::new(t)))
        } else {
            let mut n = self.ident()?;
            if self.eat(".") {
                n = format!("{}.{}", n, self.ident()?);
            }
            Ok(TyExpr::Name(n, l, c))
        }
    }

    fn func(&mut self, public: bool, line: u32, col: u32) -> R<Func> {
        let name = self.ident()?;
        self.expect("(")?;
        let mut params = Vec::new();
        while !self.eat(")") {
            let pn = self.ident()?;
            self.expect(":")?;
            params.push((pn, self.ty()?));
            if !self.is(")") {
                self.expect(",")?;
            }
        }
        let ret = if self.eat("->") {
            Some(self.ty()?)
        } else {
            None
        };
        let (mut requires, mut ensures) = (Vec::new(), Vec::new());
        loop {
            let list = match self.peek() {
                Tok::Ident(s) if s == "requires" => &mut requires,
                Tok::Ident(s) if s == "ensures" => &mut ensures,
                _ => break,
            };
            self.next();
            list.push(self.head()?);
            self.expect(";")?;
        }
        let body = self.block()?;
        Ok(Func {
            name,
            public,
            params,
            ret,
            body,
            requires,
            ensures,
            test: None,
            file: 0,
            line,
            col,
        })
    }

    fn strukt(&mut self, public: bool, line: u32, col: u32) -> R<Struct> {
        let name = self.ident()?;
        self.expect("{")?;
        let mut fields = Vec::new();
        while !self.eat("}") {
            let f = self.ident()?;
            self.expect(":")?;
            fields.push((f, self.ty()?));
            if !self.is("}") {
                self.expect(",")?;
            }
        }
        Ok(Struct {
            name,
            public,
            fields,
            line,
            col,
        })
    }

    fn block(&mut self) -> R<Vec<Stmt>> {
        self.enter()?;
        let b = self.block_inner();
        self.depth -= 1;
        b
    }

    fn block_inner(&mut self) -> R<Vec<Stmt>> {
        self.expect("{")?;
        let saved = std::mem::replace(&mut self.no_struct, false);
        let mut out = Vec::new();
        while !self.eat("}") {
            if self.peek() == &Tok::Eof {
                return Err(self.err("expected '}'"));
            }
            out.push(self.stmt()?);
        }
        self.no_struct = saved;
        Ok(out)
    }

    fn head(&mut self) -> R<Expr> {
        self.no_struct = true;
        let e = self.expr();
        self.no_struct = false;
        e
    }

    fn stmt(&mut self) -> R<Stmt> {
        let (l, c) = self.here();
        if self.eat_kw("let") {
            let name = self.ident()?;
            let ty = if self.eat(":") {
                Some(self.ty()?)
            } else {
                None
            };
            self.expect("=")?;
            let e = self.expr()?;
            self.expect(";")?;
            return Ok(Stmt::Let(name, ty, e, l, c));
        }
        if self.eat_kw("if") {
            let mut arms = vec![(self.head()?, self.block()?)];
            let mut els = None;
            while self.eat_kw("else") {
                if self.eat_kw("if") {
                    arms.push((self.head()?, self.block()?));
                } else {
                    els = Some(self.block()?);
                    break;
                }
            }
            return Ok(Stmt::If(arms, els));
        }
        if self.eat_kw("while") {
            return Ok(Stmt::While(self.head()?, self.block()?));
        }
        if self.eat_kw("for") {
            let v = self.ident()?;
            if !self.eat_kw("in") {
                return Err(self.err("expected 'in'"));
            }
            let a = self.head()?;
            if self.eat("..") {
                let b = self.head()?;
                return Ok(Stmt::ForRange(v, a, b, self.block()?));
            }
            return Ok(Stmt::ForList(v, a, self.block()?));
        }
        if self.eat_kw("break") {
            self.expect(";")?;
            return Ok(Stmt::Break(l, c));
        }
        if self.eat_kw("continue") {
            self.expect(";")?;
            return Ok(Stmt::Continue(l, c));
        }
        if self.eat_kw("return") {
            let e = if self.is(";") {
                None
            } else {
                Some(self.expr()?)
            };
            self.expect(";")?;
            return Ok(Stmt::Return(e, l, c));
        }
        if matches!(self.peek(), Tok::Kw("fn") | Tok::Kw("struct")) {
            return Err(self.err("nested definitions are not allowed"));
        }
        let e = self.expr()?;
        if self.eat("=") {
            if !matches!(
                e.kind,
                ExprKind::Var(_) | ExprKind::Index(..) | ExprKind::Field(..)
            ) {
                return Err(Error::new(
                    "parse",
                    l,
                    c,
                    "invalid assignment target".into(),
                ));
            }
            let v = self.expr()?;
            self.expect(";")?;
            return Ok(Stmt::Assign(e, v));
        }
        self.expect(";")?;
        Ok(Stmt::Expr(e))
    }

    pub fn expr(&mut self) -> R<Expr> {
        self.enter()?;
        let e = self.binary(0);
        self.depth -= 1;
        e
    }

    fn binary(&mut self, level: usize) -> R<Expr> {
        const LEVELS: &[&[&str]] = &[
            &["or"],
            &["and"],
            &[],
            &["==", "!=", "<", "<=", ">", ">="],
            &["+", "-"],
            &["*", "/", "%"],
        ];
        if level == 2 {
            let (l, c) = self.here();
            if self.eat_kw("not") {
                self.enter()?;
                let e = self.binary(2);
                self.depth -= 1;
                let e = e?;
                return Ok(ex(ExprKind::Unary("not", Box::new(e)), l, c));
            }
            return self.binary(3);
        }
        if level == LEVELS.len() {
            return self.unary();
        }
        let mut lhs = self.binary(level + 1)?;
        // Each operator in a left-associative chain nests the tree one deeper.
        let base = self.depth;
        // (Errors end the parse, so depth need not be restored on that path.)
        loop {
            let (l, c) = self.here();
            let op = match self.peek() {
                Tok::Sym(s) | Tok::Kw(s) if LEVELS[level].contains(s) => *s,
                _ => {
                    self.depth = base;
                    return Ok(lhs);
                }
            };
            self.enter()?;
            self.next();
            let rhs = self.binary(level + 1)?;
            lhs = ex(ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)), l, c);
            if level == 3 && matches!(self.peek(), Tok::Sym(s) if LEVELS[3].contains(s)) {
                return Err(self.err("comparison operators cannot be chained"));
            }
        }
    }

    fn unary(&mut self) -> R<Expr> {
        let (l, c) = self.here();
        if self.eat("-") {
            if self.peek() == &Tok::Int(i64::MIN) {
                self.next();
                return self.postfix(ex(ExprKind::Int(i64::MIN), l, c));
            }
            self.enter()?;
            let e = self.unary();
            self.depth -= 1;
            let e = e?;
            return Ok(ex(ExprKind::Unary("-", Box::new(e)), l, c));
        }
        let e = self.primary()?;
        self.postfix(e)
    }

    fn postfix(&mut self, mut e: Expr) -> R<Expr> {
        loop {
            let (l, c) = self.here();
            if self.eat("[") {
                let saved = std::mem::replace(&mut self.no_struct, false);
                let i = self.expr()?;
                self.no_struct = saved;
                self.expect("]")?;
                e = ex(ExprKind::Index(Box::new(e), Box::new(i)), l, c);
            } else if self.eat(".") {
                let f = self.ident()?;
                e = ex(ExprKind::Field(Box::new(e), f), l, c);
            } else {
                return Ok(e);
            }
        }
    }

    fn args(&mut self, close: &str) -> R<Vec<Expr>> {
        let saved = std::mem::replace(&mut self.no_struct, false);
        let mut out = Vec::new();
        while !self.eat(close) {
            out.push(self.expr()?);
            if !self.is(close) {
                self.expect(",")?;
            }
        }
        self.no_struct = saved;
        Ok(out)
    }

    fn primary(&mut self) -> R<Expr> {
        let (l, c) = self.here();
        let start = self.pos;
        let k = match self.next() {
            Tok::Int(i64::MIN) => {
                return Err(Error::new(
                    "lex",
                    l,
                    c,
                    "integer literal '9223372036854775808' out of range".into(),
                ))
            }
            Tok::Int(n) => ExprKind::Int(n),
            Tok::Float(f) => ExprKind::Float(f),
            Tok::Str(s) => ExprKind::Str(s),
            Tok::Kw("true") => ExprKind::Bool(true),
            Tok::Kw("false") => ExprKind::Bool(false),
            Tok::Sym("(") => {
                let saved = std::mem::replace(&mut self.no_struct, false);
                let e = self.expr()?;
                self.no_struct = saved;
                self.expect(")")?;
                return Ok(e);
            }
            Tok::Sym("[") => ExprKind::List(self.args("]")?),
            Tok::Ident(mut name) => {
                // `module.item(` and `module.Item {` are qualified references.
                let tok = |i: usize| self.toks.get(self.pos + i).map(|t| &t.tok);
                if self.is(".")
                    && matches!(tok(1), Some(Tok::Ident(_)))
                    && (matches!(tok(2), Some(Tok::Sym("(")))
                        || (!self.no_struct && matches!(tok(2), Some(Tok::Sym("{")))))
                {
                    self.next();
                    name = format!("{}.{}", name, self.ident()?);
                }
                if self.eat("(") {
                    ExprKind::Call(name, self.args(")")?)
                } else if !self.no_struct && self.is("{") {
                    self.next();
                    let saved = std::mem::replace(&mut self.no_struct, false);
                    let mut fields = Vec::new();
                    while !self.eat("}") {
                        let f = self.ident()?;
                        self.expect(":")?;
                        fields.push((f, self.expr()?));
                        if !self.is("}") {
                            self.expect(",")?;
                        }
                    }
                    self.no_struct = saved;
                    ExprKind::StructLit(name, fields)
                } else {
                    ExprKind::Var(name)
                }
            }
            other => {
                self.pos = start;
                self.toks[start].tok = other;
                return Err(self.err("expected expression"));
            }
        };
        Ok(ex(k, l, c))
    }
}
