//! Recursive-descent parser. Precedence (low -> high):
//! or, and, not, comparison, range (..), + -, * / %, unary -, call/index, primary.

use crate::ast::*;
use crate::error::{Kind, LimError, Span};
use crate::lexer::{Tok, Token};

pub fn parse(tokens: Vec<Token>) -> Result<Program, LimError> {
    let mut p = Parser {
        toks: tokens,
        pos: 0,
    };
    let mut prog = Vec::new();
    while !p.check(&Tok::Eof) {
        prog.push(p.stmt()?);
    }
    Ok(prog)
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
}

impl Parser {
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
                self.pos -= 1;
                self.err(format!("expected identifier, found {:?}", other))
            }
        }
    }

    fn block(&mut self) -> Result<Vec<Stmt>, LimError> {
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
        self.or()
    }

    fn or(&mut self) -> Result<Expr, LimError> {
        let mut l = self.and()?;
        while self.check(&Tok::Or) {
            let s = self.span();
            self.bump();
            l = Expr::Binary(BinOp::Or, Box::new(l), Box::new(self.and()?), s);
        }
        Ok(l)
    }

    fn and(&mut self) -> Result<Expr, LimError> {
        let mut l = self.not()?;
        while self.check(&Tok::And) {
            let s = self.span();
            self.bump();
            l = Expr::Binary(BinOp::And, Box::new(l), Box::new(self.not()?), s);
        }
        Ok(l)
    }

    fn not(&mut self) -> Result<Expr, LimError> {
        if self.check(&Tok::Not) {
            let s = self.span();
            self.bump();
            return Ok(Expr::Unary(UnOp::Not, Box::new(self.not()?), s));
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
        Ok(Expr::Binary(op, Box::new(l), Box::new(r), s))
    }

    fn range(&mut self) -> Result<Expr, LimError> {
        let l = self.add()?;
        if self.check(&Tok::DotDot) {
            let s = self.span();
            self.bump();
            return Ok(Expr::Range(Box::new(l), Box::new(self.add()?), s));
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
            l = Expr::Binary(op, Box::new(l), Box::new(self.mul()?), s);
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
            l = Expr::Binary(op, Box::new(l), Box::new(self.unary()?), s);
        }
    }

    fn unary(&mut self) -> Result<Expr, LimError> {
        if self.check(&Tok::Minus) {
            let s = self.span();
            self.bump();
            return Ok(Expr::Unary(UnOp::Neg, Box::new(self.unary()?), s));
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
                e = Expr::Call(Box::new(e), args, s);
            } else if self.eat(&Tok::LBracket) {
                let i = self.expr()?;
                self.expect(Tok::RBracket, "']'")?;
                e = Expr::Index(Box::new(e), Box::new(i), s);
            } else {
                return Ok(e);
            }
        }
    }

    fn primary(&mut self) -> Result<Expr, LimError> {
        let s = self.span();
        Ok(match self.bump() {
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
        })
    }
}
