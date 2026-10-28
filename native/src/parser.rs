//! Recursive-descent parser. Stops at the first syntax error.

use crate::ast::*;
use crate::lexer::{Tok, Token};
use crate::Error;

type R<T> = Result<T, Error>;

pub struct Parser {
    toks: Vec<Token>,
    pos: usize,
    /// Current syntax nesting depth, capped at MAX_DEPTH so later recursive
    /// passes (and the C compiler) cannot overflow their stacks.
    depth: u32,
}

const MAX_DEPTH: u32 = 3000;

pub fn parse(toks: Vec<Token>) -> R<Program> {
    let mut p = Parser {
        toks,
        pos: 0,
        depth: 0,
    };
    let mut prog = Program::default();
    loop {
        match p.peek() {
            Tok::Eof => break,
            Tok::Newline => {
                p.next();
                continue;
            }
            _ => {}
        }
        let (line, col) = p.here();
        if p.eat_kw("import") {
            let mut name = p.ident()?;
            while p.eat(".") {
                name = format!("{}.{}", name, p.ident()?);
            }
            p.end_line()?;
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
            p.end_line()?;
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
                inout: Vec::new(),
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
        } else if p.eat_kw("enum") {
            prog.enums.push(p.enum_decl(public, line, col)?);
        } else if public {
            return Err(p.err("expected 'fn', 'struct' or 'enum' after 'pub'"));
        } else {
            // Script mode: top-level statements form the program body.
            if prog.top.is_empty() {
                prog.top_pos = (line, col);
            }
            let s = p.stmt()?;
            prog.top.push(s);
        }
    }
    Ok(prog)
}

/// Implicit return: in a fn with a return type, a trailing expression
/// statement (also at the end of every branch of a final if/else) is the
/// returned value.
fn tail_return(b: &mut Vec<Stmt>) {
    match b.last_mut() {
        Some(Stmt::Expr(_)) => {
            let Some(Stmt::Expr(e)) = b.pop() else {
                unreachable!()
            };
            let (l, c) = (e.line, e.col);
            b.push(Stmt::Return(Some(e), l, c));
        }
        Some(Stmt::If(arms, Some(els))) => {
            for (_, a) in arms.iter_mut() {
                tail_return(a);
            }
            tail_return(els);
        }
        Some(Stmt::Match(_, arms)) => {
            for a in arms.iter_mut() {
                tail_return(&mut a.body);
            }
        }
        _ => {}
    }
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
            Tok::Newline => "end of line".to_string(),
            Tok::Indent => "indent".to_string(),
            Tok::Dedent => "end of block".to_string(),
        };
        let mut e = Error::new("parse", l, c, format!("{}, found '{}'", msg, found));
        e.hint = match self.peek() {
            Tok::Sym(";") => "Limits has no semicolons: put one statement per line".into(),
            Tok::Sym("{") if msg.contains("':'") => {
                "blocks are indented: end the line with ':' and indent the body 4 spaces".into()
            }
            Tok::Indent => "unexpected indentation: indent only after a line ending in ':'".into(),
            Tok::Sym("&") => {
                "'&x' is only allowed as a call argument, to pass x to an 'inout' parameter".into()
            }
            Tok::Kw("match") => {
                "a 'match' value must be the whole right side of 'let', 'var', '=' or 'return'; or use a match statement".into()
            }
            _ => String::new(),
        };
        e
    }
    fn end_line(&mut self) -> R<()> {
        if self.peek() == &Tok::Newline {
            self.next();
            Ok(())
        } else {
            Err(self.err("expected end of line"))
        }
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
            if self.eat("[") {
                let mut args = vec![self.ty()?];
                while self.eat(",") {
                    args.push(self.ty()?);
                }
                self.expect("]")?;
                return Ok(TyExpr::Generic(n, args, l, c));
            }
            Ok(TyExpr::Name(n, l, c))
        }
    }

    fn func(&mut self, public: bool, line: u32, col: u32) -> R<Func> {
        let name = self.ident()?;
        self.expect("(")?;
        let (mut params, mut inout) = (Vec::new(), Vec::new());
        while !self.eat(")") {
            let pn = self.ident()?;
            self.expect(":")?;
            // `inout` is contextual: a modifier only when a type follows.
            let io = matches!(self.peek(), Tok::Ident(s) if s == "inout")
                && matches!(
                    self.toks.get(self.pos + 1).map(|t| &t.tok),
                    Some(Tok::Ident(_) | Tok::Sym("["))
                );
            if io {
                self.next();
            }
            inout.push(io);
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
        self.expect(":")?;
        self.enter()?;
        let mut body = Vec::new();
        if self.peek() == &Tok::Newline {
            self.next();
            self.indent()?;
            // Contract clauses come first in the body.
            loop {
                let list = match self.peek() {
                    Tok::Ident(s) if s == "requires" && !self.assign_follows() => &mut requires,
                    Tok::Ident(s) if s == "ensures" && !self.assign_follows() => &mut ensures,
                    _ => break,
                };
                self.next();
                list.push(self.expr()?);
                self.end_line()?;
            }
            while !self.eat_tok(&Tok::Dedent) {
                body.push(self.stmt()?);
            }
        } else {
            body.push(self.stmt()?);
        }
        self.depth -= 1;
        if ret.is_some() {
            tail_return(&mut body);
        }
        Ok(Func {
            name,
            public,
            params,
            inout,
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

    /// Is the token after the current one an assignment operator? (Then a
    /// contextual keyword like `requires` is an ordinary variable.)
    fn assign_follows(&self) -> bool {
        matches!(
            self.toks.get(self.pos + 1).map(|t| &t.tok),
            Some(Tok::Sym("=" | "+=" | "-=" | "*=" | "/=" | "%=" | "." | "["))
        )
    }

    fn strukt(&mut self, public: bool, line: u32, col: u32) -> R<Struct> {
        let name = self.ident()?;
        self.expect(":")?;
        self.end_line()?;
        self.indent()?;
        let mut fields = Vec::new();
        while !self.eat_tok(&Tok::Dedent) {
            let f = self.ident()?;
            self.expect(":")?;
            fields.push((f, self.ty()?));
            self.end_line()?;
        }
        Ok(Struct {
            name,
            public,
            fields,
            line,
            col,
        })
    }

    /// `enum Name:` then one variant per line: `Circle(r: float)` (field
    /// names may be left out: `Circle(float)`) or a unit variant `Empty`.
    fn enum_decl(&mut self, public: bool, line: u32, col: u32) -> R<Enum> {
        let name = self.ident()?;
        self.expect(":")?;
        self.end_line()?;
        self.indent()?;
        let mut variants = Vec::new();
        while !self.eat_tok(&Tok::Dedent) {
            let (vl, vc) = self.here();
            let vn = self.ident()?;
            let mut fields = Vec::new();
            if self.eat("(") {
                while !self.eat(")") {
                    let named = matches!(self.peek(), Tok::Ident(_))
                        && matches!(
                            self.toks.get(self.pos + 1).map(|t| &t.tok),
                            Some(Tok::Sym(":"))
                        );
                    let f = if named {
                        let f = self.ident()?;
                        self.next();
                        f
                    } else {
                        format!("_{}", fields.len())
                    };
                    fields.push((f, self.ty()?));
                    if !self.is(")") {
                        self.expect(",")?;
                    }
                }
            }
            self.end_line()?;
            variants.push(Variant {
                name: vn,
                fields,
                line: vl,
                col: vc,
            });
        }
        Ok(Enum {
            name,
            public,
            variants,
            line,
            col,
        })
    }

    /// `match e:` and its indented arms (after the `match` keyword).
    fn match_arms(&mut self) -> R<(Expr, Vec<Arm>)> {
        let e = self.expr()?;
        self.expect(":")?;
        self.end_line()?;
        self.indent()?;
        self.enter()?;
        let mut arms = Vec::new();
        while !self.eat_tok(&Tok::Dedent) {
            let (l, c) = self.here();
            let pat = self.pattern()?;
            let body = self.block()?;
            arms.push(Arm {
                pat,
                body,
                line: l,
                col: c,
            });
        }
        self.depth -= 1;
        Ok((e, arms))
    }

    fn pattern(&mut self) -> R<Pat> {
        let neg = self.eat("-");
        match self.peek() {
            Tok::Int(_) => {
                let Tok::Int(n) = self.next() else {
                    unreachable!()
                };
                return Ok(Pat::Int(if neg { n.wrapping_neg() } else { n }));
            }
            _ if neg => return Err(self.err("expected an int after '-' in a pattern")),
            Tok::Str(_) => {
                let Tok::Str(x) = self.next() else {
                    unreachable!()
                };
                return Ok(Pat::Str(x));
            }
            Tok::Kw("true") => {
                self.next();
                return Ok(Pat::Bool(true));
            }
            Tok::Kw("false") => {
                self.next();
                return Ok(Pat::Bool(false));
            }
            Tok::Ident(_) => {}
            _ => {
                let mut e = self.err("expected a pattern");
                e.hint = "patterns are a variant like 'Some(x)' or 'Circle(r)', a literal, or '_'"
                    .into();
                return Err(e);
            }
        }
        let mut name = self.ident()?;
        if name == "_" {
            return Ok(Pat::Wild);
        }
        while self.eat(".") {
            name = format!("{}.{}", name, self.ident()?);
        }
        let mut binds = Vec::new();
        if self.eat("(") {
            while !self.eat(")") {
                binds.push(self.ident()?);
                if !self.is(")") {
                    self.expect(",")?;
                }
            }
        }
        Ok(Pat::Variant(name, binds, 0))
    }

    /// Right side of `let`/`=`/`return`: an expression, or a `match` whose
    /// arms give the value (it ends with its block, so no end of line follows).
    fn rhs(&mut self) -> R<(Expr, bool)> {
        let (l, c) = self.here();
        if self.eat_kw("match") {
            let (e, arms) = self.match_arms()?;
            return Ok((ex(ExprKind::Match(Box::new(e), arms), l, c), true));
        }
        Ok((self.expr()?, false))
    }

    /// Does a struct literal body start `i` tokens ahead? It is `{}` or
    /// `{ field: ...`; any other `{` (e.g. a C-style block) is not one.
    fn lit(&self, i: usize) -> bool {
        let tok = |i: usize| self.toks.get(self.pos + i).map(|t| &t.tok);
        matches!(tok(i), Some(Tok::Sym("{")))
            && (matches!(tok(i + 1), Some(Tok::Sym("}")))
                || (matches!(tok(i + 1), Some(Tok::Ident(_)))
                    && matches!(tok(i + 2), Some(Tok::Sym(":")))))
    }

    fn eat_tok(&mut self, t: &Tok) -> bool {
        let y = self.peek() == t;
        if y {
            self.next();
        }
        y
    }

    fn indent(&mut self) -> R<()> {
        if self.eat_tok(&Tok::Indent) {
            Ok(())
        } else {
            Err(self.err("expected an indented block"))
        }
    }

    /// `: NEWLINE INDENT stmt+ DEDENT`, or `: stmt` on the same line.
    fn block(&mut self) -> R<Vec<Stmt>> {
        self.expect(":")?;
        self.enter()?;
        let mut out = Vec::new();
        if self.peek() == &Tok::Newline {
            self.next();
            self.indent()?;
            while !self.eat_tok(&Tok::Dedent) {
                out.push(self.stmt()?);
            }
        } else {
            out.push(self.stmt()?);
        }
        self.depth -= 1;
        Ok(out)
    }

    fn stmt(&mut self) -> R<Stmt> {
        let (l, c) = self.here();
        let mutable = matches!(self.peek(), Tok::Kw("var"));
        if self.eat_kw("let") || self.eat_kw("var") {
            let name = self.ident()?;
            let ty = if self.eat(":") {
                Some(self.ty()?)
            } else {
                None
            };
            self.expect("=")?;
            let (e, block) = self.rhs()?;
            if !block {
                self.end_line()?;
            }
            return Ok(Stmt::Let(name, ty, e, l, c, mutable));
        }
        if self.eat_kw("if") {
            let mut arms = vec![(self.expr()?, self.block()?)];
            let mut els = None;
            loop {
                if self.eat_kw("elif") {
                    arms.push((self.expr()?, self.block()?));
                } else if self.eat_kw("else") {
                    if self.eat_kw("if") {
                        arms.push((self.expr()?, self.block()?));
                    } else {
                        els = Some(self.block()?);
                        break;
                    }
                } else {
                    break;
                }
            }
            return Ok(Stmt::If(arms, els));
        }
        if self.eat_kw("match") {
            let (e, arms) = self.match_arms()?;
            return Ok(Stmt::Match(e, arms));
        }
        if self.eat_kw("while") {
            return Ok(Stmt::While(self.expr()?, self.block()?));
        }
        if self.eat_kw("for") {
            let v = self.ident()?;
            if !self.eat_kw("in") {
                return Err(self.err("expected 'in'"));
            }
            let e = self.expr()?;
            let body = self.block()?;
            return Ok(match e.kind {
                ExprKind::Range(a, b) => Stmt::ForRange(v, *a, *b, body),
                _ => Stmt::ForList(v, e, body),
            });
        }
        let simple = if self.eat_kw("break") {
            Stmt::Break(l, c)
        } else if self.eat_kw("continue") {
            Stmt::Continue(l, c)
        } else if self.eat_kw("pass") {
            Stmt::Pass
        } else if self.eat_kw("return") {
            let e = if self.peek() == &Tok::Newline {
                None
            } else {
                let (e, block) = self.rhs()?;
                if block {
                    return Ok(Stmt::Return(Some(e), l, c));
                }
                Some(e)
            };
            Stmt::Return(e, l, c)
        } else if matches!(
            self.peek(),
            Tok::Kw("fn") | Tok::Kw("struct") | Tok::Kw("enum")
        ) {
            return Err(self.err("nested definitions are not allowed"));
        } else {
            let e = self.expr()?;
            let (ol, oc) = self.here();
            let op = match self.peek() {
                Tok::Sym("=") => Some(None),
                Tok::Sym("+=") => Some(Some("+")),
                Tok::Sym("-=") => Some(Some("-")),
                Tok::Sym("*=") => Some(Some("*")),
                Tok::Sym("/=") => Some(Some("/")),
                Tok::Sym("%=") => Some(Some("%")),
                _ => None,
            };
            match op {
                Some(op) => {
                    self.next();
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
                    let (v, block) = self.rhs()?;
                    if block {
                        return Ok(Stmt::Assign(e, op.map(|o| (o, ol, oc)), v));
                    }
                    Stmt::Assign(e, op.map(|o| (o, ol, oc)), v)
                }
                None => Stmt::Expr(e),
            }
        };
        self.end_line()?;
        Ok(simple)
    }

    pub fn expr(&mut self) -> R<Expr> {
        self.enter()?;
        let e = self.range();
        self.depth -= 1;
        e
    }

    /// `a..b` binds loosest of all.
    fn range(&mut self) -> R<Expr> {
        let a = self.binary(0)?;
        if !self.eat("..") {
            return Ok(a);
        }
        let b = self.binary(0)?;
        let (l, c) = (a.line, a.col);
        Ok(ex(ExprKind::Range(Box::new(a), Box::new(b)), l, c))
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
                let i = self.expr()?;
                self.expect("]")?;
                e = ex(ExprKind::Index(Box::new(e), Box::new(i)), l, c);
            } else if self.eat(".") {
                let f = self.ident()?;
                if self.eat("(") {
                    let args = self.args(")")?;
                    // Located at the receiver: `env.fs.read(p)` is one call.
                    let mut b = &e;
                    while let ExprKind::Field(x, _)
                    | ExprKind::Index(x, _)
                    | ExprKind::Method(x, ..)
                    | ExprKind::Try(x) = &b.kind
                    {
                        b = x;
                    }
                    let (rl, rc) = (b.line, b.col);
                    e = ex(ExprKind::Method(Box::new(e), f, args), rl, rc);
                } else {
                    e = ex(ExprKind::Field(Box::new(e), f), l, c);
                }
            } else if self.eat("?") {
                e = ex(ExprKind::Try(Box::new(e)), l, c);
            } else {
                return Ok(e);
            }
        }
    }

    fn args(&mut self, close: &str) -> R<Vec<Expr>> {
        let mut out = Vec::new();
        while !self.eat(close) {
            let (l, c) = self.here();
            if self.eat("&") {
                let e = self.expr()?;
                out.push(ex(ExprKind::Ref(Box::new(e)), l, c));
            } else {
                out.push(self.expr()?);
            }
            if !self.is(close) {
                self.expect(",")?;
            }
        }
        Ok(out)
    }

    /// After `[`: a list literal or a comprehension `[e for x in xs if c]`.
    fn list(&mut self) -> R<ExprKind> {
        if self.eat("]") {
            return Ok(ExprKind::List(Vec::new()));
        }
        let first = self.expr()?;
        if !matches!(self.peek(), Tok::Kw("for")) {
            let mut items = vec![first];
            while !self.eat("]") {
                self.expect(",")?;
                if self.eat("]") {
                    break;
                }
                items.push(self.expr()?);
            }
            return Ok(ExprKind::List(items));
        }
        let mut clauses = Vec::new();
        loop {
            if self.eat_kw("for") {
                let v = self.ident()?;
                if !self.eat_kw("in") {
                    return Err(self.err("expected 'in'"));
                }
                clauses.push(Clause::For(v, self.expr()?));
            } else if self.eat_kw("if") {
                clauses.push(Clause::If(self.expr()?));
            } else {
                self.expect("]")?;
                return Ok(ExprKind::Comp(Box::new(first), clauses));
            }
        }
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
                let e = self.expr()?;
                self.expect(")")?;
                return Ok(e);
            }
            Tok::Sym("[") => self.list()?,
            Tok::Ident(mut name) => {
                // `module.item(` and `module.Item {` are qualified references.
                let tok = |i: usize| self.toks.get(self.pos + i).map(|t| &t.tok);
                if self.is(".")
                    && matches!(tok(1), Some(Tok::Ident(_)))
                    && (matches!(tok(2), Some(Tok::Sym("("))) || self.lit(2))
                {
                    self.next();
                    name = format!("{}.{}", name, self.ident()?);
                }
                if self.eat("(") {
                    ExprKind::Call(name, self.args(")")?)
                } else if self.lit(0) {
                    self.next();
                    let mut fields = Vec::new();
                    while !self.eat("}") {
                        let f = self.ident()?;
                        self.expect(":")?;
                        fields.push((f, self.expr()?));
                        if !self.is("}") {
                            self.expect(",")?;
                        }
                    }
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
