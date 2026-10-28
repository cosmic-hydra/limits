//! Recursive-descent parser. Precedence (low -> high):
//! or, and, not, comparison, range (..), + -, * / %, unary -, call/index, primary.

use crate::ast::*;
use crate::error::{Kind, LimError, Span};
use crate::lexer::{Tok, Token};

pub fn parse(tokens: Vec<Token>) -> Result<Program, LimError> {
    if tokens.last().is_none_or(|t| t.tok != Tok::Eof) {
        return Err(LimError::new(
            Kind::Parse,
            "missing end-of-file token",
            tokens.last().map_or(Span { line: 1, col: 1 }, |t| t.span),
        ));
    }
    // An EOF sentinel is meaningful only at the end. Otherwise a caller-built
    // token stream could silently discard everything after its first sentinel.
    if let Some(token) = tokens[..tokens.len() - 1]
        .iter()
        .find(|token| token.tok == Tok::Eof)
    {
        return Err(LimError::new(
            Kind::Parse,
            "unexpected end-of-file token",
            token.span,
        ));
    }
    let mut p = Parser {
        toks: tokens,
        pos: 0,
        depth: 0,
    };
    let mut prog = Vec::new();
    while !p.check(&Tok::Eof) {
        prog.push(p.stmt()?);
    }
    crate::ast::validate(&prog)?;
    Ok(prog)
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    depth: usize,
}

const MAX_NESTING: usize = 128;

impl Parser {
    fn nested<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, LimError>,
    ) -> Result<T, LimError> {
        if self.depth >= MAX_NESTING {
            return self.err("syntax nesting limit exceeded (128)");
        }
        self.depth += 1;
        let result = f(self);
        self.depth -= 1;
        result
    }
    fn checked(&self, e: Expr) -> Result<Expr, LimError> {
        let mut work = vec![(&e, 1)];
        while let Some((e, d)) = work.pop() {
            if d > MAX_NESTING {
                return self.err("expression nesting limit exceeded (128)");
            }
            match e {
                Expr::List(es) => work.extend(es.iter().map(|e| (e, d + 1))),
                Expr::Map(es, _) => {
                    for (a, b) in es {
                        work.push((a, d + 1));
                        work.push((b, d + 1));
                    }
                }
                Expr::Unary(_, e, _) => work.push((e, d + 1)),
                Expr::Binary(_, a, b, _) | Expr::Range(a, b, _) | Expr::Index(a, b, _) => {
                    work.push((a, d + 1));
                    work.push((b, d + 1));
                }
                Expr::Call(f, args, _) => {
                    work.push((f, d + 1));
                    work.extend(args.iter().map(|e| (e, d + 1)));
                }
                _ => {}
            }
        }
        Ok(e)
    }
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }
    fn span(&self) -> Span {
        self.toks[self.pos].span
    }
    fn check(&self, t: &Tok) -> bool {
        self.peek() == t
    }

    fn bump(&mut self) -> Tok {
        let t = self.toks[self.pos].tok.clone();
        if t != Tok::Eof {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.check(t) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn err<T>(&self, msg: impl Into<String>) -> Result<T, LimError> {
        Err(LimError::new(Kind::Parse, msg, self.span()))
    }

    fn expect(&mut self, t: Tok, what: &str) -> Result<(), LimError> {
        if self.eat(&t) {
            Ok(())
        } else {
            self.err(format!("expected {}, found {:?}", what, self.peek()))
        }
    }

    fn ident(&mut self) -> Result<String, LimError> {
        match self.bump() {
            Tok::Ident(s) => Ok(s),
            other => {
                // bump deliberately leaves the EOF sentinel in place.
                if other != Tok::Eof {
                    self.pos -= 1;
                }
                self.err(format!("expected identifier, found {:?}", other))
            }
        }
    }

    fn block(&mut self) -> Result<Vec<Stmt>, LimError> {
        self.nested(|p| p.block_inner())
    }
    fn block_inner(&mut self) -> Result<Vec<Stmt>, LimError> {
        self.expect(Tok::LBrace, "'{'")?;
        let mut body = Vec::new();
        while !self.check(&Tok::RBrace) {
            if self.check(&Tok::Eof) {
                return self.err("unclosed block, expected '}'");
            }
            body.push(self.stmt()?);
        }
        self.bump();
        Ok(body)
    }

    fn stmt(&mut self) -> Result<Stmt, LimError> {
        let span = self.span();
        match self.peek() {
            Tok::Let => {
                self.bump();
                let name = self.ident()?;
                self.expect(Tok::Assign, "'='")?;
                let e = self.expr()?;
                self.expect(Tok::Semi, "';'")?;
                Ok(Stmt::Let(name, e, span))
            }
            Tok::Fn => {
                self.bump();
                let name = self.ident()?;
                self.expect(Tok::LParen, "'('")?;
                let mut params = Vec::new();
                if !self.check(&Tok::RParen) {
                    loop {
                        params.push(self.ident()?);
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                }
                self.expect(Tok::RParen, "')'")?;
                Ok(Stmt::Fn(name, params, self.block()?, span))
            }
            Tok::Return => {
                self.bump();
                let e = if self.check(&Tok::Semi) {
                    None
                } else {
                    Some(self.expr()?)
                };
                self.expect(Tok::Semi, "';'")?;
                Ok(Stmt::Return(e, span))
            }
            Tok::If => self.if_stmt(),
            Tok::While => {
                self.bump();
                let c = self.expr()?;
                Ok(Stmt::While(c, self.block()?, span))
            }
            Tok::For => {
                self.bump();
                let name = self.ident()?;
                self.expect(Tok::In, "'in'")?;
                let it = self.expr()?;
                Ok(Stmt::For(name, it, self.block()?, span))
            }
            Tok::Break => {
                self.bump();
                self.expect(Tok::Semi, "';'")?;
                Ok(Stmt::Break(span))
            }
            Tok::Continue => {
                self.bump();
                self.expect(Tok::Semi, "';'")?;
                Ok(Stmt::Continue(span))
            }
            _ => {
                let e = self.expr()?;
                if self.eat(&Tok::Assign) {
                    if !matches!(e, Expr::Var(..) | Expr::Index(..)) {
                        return Err(LimError::new(
                            Kind::Parse,
                            "invalid assignment target",
                            span,
                        ));
                    }
                    let v = self.expr()?;
                    self.expect(Tok::Semi, "';'")?;
                    return Ok(Stmt::Assign(e, v, span));
                }
                self.expect(Tok::Semi, "';'")?;
                Ok(Stmt::Expr(e, span))
            }
        }
    }

    fn if_stmt(&mut self) -> Result<Stmt, LimError> {
        self.nested(|p| p.if_stmt_inner())
    }
    fn if_stmt_inner(&mut self) -> Result<Stmt, LimError> {
        let span = self.span();
        self.bump(); // `if`
        let c = self.expr()?;
        let then = self.block()?;
        let els = if self.eat(&Tok::Else) {
            if self.check(&Tok::If) {
                Some(vec![self.if_stmt()?])
            } else {
                Some(self.block()?)
            }
        } else {
            None
        };
        Ok(Stmt::If(c, then, els, span))
    }

    pub fn expr(&mut self) -> Result<Expr, LimError> {
        self.nested(|p| p.expr_inner())
    }
    fn expr_inner(&mut self) -> Result<Expr, LimError> {
        self.or()
    }

    fn or(&mut self) -> Result<Expr, LimError> {
        let mut l = self.and()?;
        while self.check(&Tok::Or) {
            let s = self.span();
            self.bump();
            let r = self.and()?;
            l = self.checked(Expr::Binary(BinOp::Or, Box::new(l), Box::new(r), s))?;
        }
        Ok(l)
    }

    fn and(&mut self) -> Result<Expr, LimError> {
        let mut l = self.not()?;
        while self.check(&Tok::And) {
            let s = self.span();
            self.bump();
            let r = self.not()?;
            l = self.checked(Expr::Binary(BinOp::And, Box::new(l), Box::new(r), s))?;
        }
        Ok(l)
    }

    fn not(&mut self) -> Result<Expr, LimError> {
        if self.check(&Tok::Not) {
            let s = self.span();
            self.bump();
            let x = self.nested(|p| p.not())?;
            return self.checked(Expr::Unary(UnOp::Not, Box::new(x), s));
        }
        self.cmp()
    }

    fn cmp(&mut self) -> Result<Expr, LimError> {
        let l = self.range()?;
        let op = match self.peek() {
            Tok::Eq => BinOp::Eq,
            Tok::Ne => BinOp::Ne,
            Tok::Lt => BinOp::Lt,
            Tok::Le => BinOp::Le,
            Tok::Gt => BinOp::Gt,
            Tok::Ge => BinOp::Ge,
            _ => return Ok(l),
        };
        let s = self.span();
        self.bump();
        let r = self.range()?;
        // Chained comparisons (a < b < c) are rejected rather than silently misread.
        if matches!(
            self.peek(),
            Tok::Eq | Tok::Ne | Tok::Lt | Tok::Le | Tok::Gt | Tok::Ge
        ) {
            return self.err("comparisons cannot be chained; use 'and'");
        }
        self.checked(Expr::Binary(op, Box::new(l), Box::new(r), s))
    }

    fn range(&mut self) -> Result<Expr, LimError> {
        let l = self.add()?;
        if self.check(&Tok::DotDot) {
            let s = self.span();
            self.bump();
            let r = self.add()?;
            return self.checked(Expr::Range(Box::new(l), Box::new(r), s));
        }
        Ok(l)
    }

    fn add(&mut self) -> Result<Expr, LimError> {
        let mut l = self.mul()?;
        loop {
            let op = match self.peek() {
                Tok::Plus => BinOp::Add,
                Tok::Minus => BinOp::Sub,
                _ => return Ok(l),
            };
            let s = self.span();
            self.bump();
            let r = self.mul()?;
            l = self.checked(Expr::Binary(op, Box::new(l), Box::new(r), s))?;
        }
    }

    fn mul(&mut self) -> Result<Expr, LimError> {
        let mut l = self.unary()?;
        loop {
            let op = match self.peek() {
                Tok::Star => BinOp::Mul,
                Tok::Slash => BinOp::Div,
                Tok::Percent => BinOp::Mod,
                _ => return Ok(l),
            };
            let s = self.span();
            self.bump();
            let r = self.unary()?;
            l = self.checked(Expr::Binary(op, Box::new(l), Box::new(r), s))?;
        }
    }

    fn unary(&mut self) -> Result<Expr, LimError> {
        if self.check(&Tok::Minus) {
            let s = self.span();
            self.bump();
            let x = self.nested(|p| p.unary())?;
            return self.checked(Expr::Unary(UnOp::Neg, Box::new(x), s));
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Result<Expr, LimError> {
        let mut e = self.primary()?;
        loop {
            let s = self.span();
            if self.eat(&Tok::LParen) {
                let mut args = Vec::new();
                if !self.check(&Tok::RParen) {
                    loop {
                        args.push(self.expr()?);
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                }
                self.expect(Tok::RParen, "')'")?;
                e = self.checked(Expr::Call(Box::new(e), args, s))?;
            } else if self.eat(&Tok::LBracket) {
                let i = self.expr()?;
                self.expect(Tok::RBracket, "']'")?;
                e = self.checked(Expr::Index(Box::new(e), Box::new(i), s))?;
            } else {
                return Ok(e);
            }
        }
    }

    fn primary(&mut self) -> Result<Expr, LimError> {
        let s = self.span();
        let result = match self.bump() {
            Tok::Int(n) => Expr::Int(n),
            Tok::Float(f) => Expr::Float(f),
            Tok::Str(st) => Expr::Str(st),
            Tok::True => Expr::Bool(true),
            Tok::False => Expr::Bool(false),
            Tok::Nil => Expr::Nil,
            Tok::Ident(n) => Expr::Var(n, s),
            Tok::LParen => {
                let e = self.expr()?;
                self.expect(Tok::RParen, "')'")?;
                e
            }
            Tok::LBracket => {
                let mut items = Vec::new();
                while !self.check(&Tok::RBracket) {
                    items.push(self.expr()?);
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
                self.expect(Tok::RBracket, "']'")?;
                Expr::List(items)
            }
            Tok::LBrace => {
                let mut pairs = Vec::new();
                while !self.check(&Tok::RBrace) {
                    let k = self.expr()?;
                    self.expect(Tok::Colon, "':'")?;
                    pairs.push((k, self.expr()?));
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
                self.expect(Tok::RBrace, "'}'")?;
                Expr::Map(pairs, s)
            }
            other => {
                if other != Tok::Eof {
                    self.pos -= 1;
                }
                return self.err(format!("expected expression, found {:?}", other));
            }
        };
        self.checked(result)
    }
}
