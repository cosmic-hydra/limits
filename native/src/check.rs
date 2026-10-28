//! Name resolution and type checking. Collects every error instead of stopping
//! at the first, and annotates `Expr::ty` for codegen.

use crate::ast::*;
use crate::module::Module;
use crate::Error;
use std::collections::{HashMap, HashSet};

pub const BUILTINS: &[&str] = &[
    "print", "len", "push", "pop", "str", "int", "float", "assert", "sqrt",
];

/// Struct name -> fields in declaration order.
pub type StructMap = HashMap<String, Vec<(String, Ty)>>;

pub struct Checker {
    pub structs: HashMap<String, Vec<(String, Ty)>>,
    funcs: HashMap<String, (Vec<Ty>, Ty)>,
    /// Qualified names of `pub` functions and structs.
    pub_fns: HashSet<String>,
    pub_structs: HashSet<String>,
    /// Per module: qualifier and import aliases.
    names: Vec<String>,
    aliases: Vec<HashMap<String, usize>>,
    files: Vec<String>,
    cur: usize,
    scopes: Vec<HashMap<String, Ty>>,
    ret: Ty,
    loops: usize,
    errs: Vec<Error>,
}

fn qualify(module: &str, item: &str) -> String {
    if module.is_empty() {
        item.to_string()
    } else {
        format!("{}.{}", module, item)
    }
}

/// Check every module and merge them into one program whose item names (and
/// every call, struct literal and type that refers to them) are qualified.
pub fn check(mods: &mut [Module]) -> Result<(Program, StructMap), Vec<Error>> {
    let mut c = Checker {
        structs: HashMap::new(),
        funcs: HashMap::new(),
        pub_fns: HashSet::new(),
        pub_structs: HashSet::new(),
        names: mods.iter().map(|m| m.name.clone()).collect(),
        aliases: mods.iter().map(|m| m.aliases.clone()).collect(),
        files: mods.iter().map(|m| m.file.clone()).collect(),
        cur: 0,
        scopes: Vec::new(),
        ret: Ty::Unit,
        loops: 0,
        errs: Vec::new(),
    };
    // Pass 1a: declare every struct name in every module.
    for (i, m) in mods.iter().enumerate() {
        c.cur = i;
        for s in &m.prog.structs {
            let q = qualify(&m.name, &s.name);
            if matches!(s.name.as_str(), "int" | "float" | "bool" | "str") {
                let m = format!("'{}' is a builtin type name", s.name);
                c.err(s.line, s.col, m);
            }
            if s.public {
                c.pub_structs.insert(q.clone());
            }
            if c.structs.insert(q, Vec::new()).is_some() {
                c.err(s.line, s.col, format!("struct '{}' defined twice", s.name));
            }
        }
    }
    // Pass 1b: resolve fields and signatures.
    for (i, m) in mods.iter_mut().enumerate() {
        c.cur = i;
        for s in &mut m.prog.structs {
            let mut fields: Vec<(String, Ty)> = Vec::new();
            for (f, t) in &mut s.fields {
                let t = c.resolve(t);
                if fields.iter().any(|(n, _)| n == f) {
                    c.err(
                        s.line,
                        s.col,
                        format!("duplicate field '{}' in struct '{}'", f, s.name),
                    );
                }
                fields.push((f.clone(), t));
            }
            c.structs.insert(qualify(&m.name, &s.name), fields);
        }
        for f in &mut m.prog.funcs {
            let ps: Vec<Ty> = f.params.iter_mut().map(|(_, t)| c.resolve(t)).collect();
            let r = f.ret.as_mut().map_or(Ty::Unit, |t| c.resolve(t));
            let q = qualify(&m.name, &f.name);
            if f.public {
                c.pub_fns.insert(q.clone());
            }
            if BUILTINS.contains(&f.name.as_str()) {
                c.err(
                    f.line,
                    f.col,
                    format!("'{}' is a builtin and cannot be redefined", f.name),
                );
            } else if c.funcs.insert(q, (ps, r)).is_some() {
                c.err(
                    f.line,
                    f.col,
                    format!("function '{}' defined twice", f.name),
                );
            }
        }
    }
    // Pass 1c: a struct that contains itself by value (not through a list)
    // can never be constructed.
    for (i, m) in mods.iter().enumerate() {
        c.cur = i;
        for s in &m.prog.structs {
            let q = qualify(&m.name, &s.name);
            let (mut todo, mut seen) = (vec![q.clone()], HashSet::new());
            while let Some(n) = todo.pop() {
                for (_, t) in c.structs.get(&n).into_iter().flatten() {
                    if let Ty::Struct(x) = t {
                        if *x == q {
                            let m = format!(
                                "struct '{}' contains itself (only through a list can a struct refer to itself), so it can never be constructed",
                                s.name
                            );
                            c.err(s.line, s.col, m);
                            todo.clear();
                            break;
                        }
                        if seen.insert(x.clone()) {
                            todo.push(x.clone());
                        }
                    }
                }
            }
        }
    }
    c.cur = 0;
    match c.funcs.get("main") {
        None => c.err(1, 1, "missing 'fn main()'".into()),
        Some((p, r)) if !p.is_empty() || *r != Ty::Unit => {
            let f = mods[0]
                .prog
                .funcs
                .iter()
                .find(|f| f.name == "main")
                .unwrap();
            c.err(
                f.line,
                f.col,
                "'main' must take no parameters and return nothing".into(),
            );
        }
        _ => {}
    }
    // Pass 2: bodies.
    for (i, m) in mods.iter_mut().enumerate() {
        c.cur = i;
        for f in &mut m.prog.funcs {
            let Some((ps, r)) = c.funcs.get(&qualify(&m.name, &f.name)).cloned() else {
                continue; // builtin redefinition, already reported
            };
            c.ret = r.clone();
            c.scopes = vec![HashMap::new()];
            for ((n, _), t) in f.params.iter().zip(ps) {
                if c.scopes[0].insert(n.clone(), t).is_some() {
                    c.err(f.line, f.col, format!("duplicate parameter '{}'", n));
                }
            }
            c.block(&mut f.body, false);
            if r != Ty::Unit && !returns(&f.body) {
                c.err(
                    f.line,
                    f.col,
                    format!("function '{}' may finish without returning a {}", f.name, r),
                );
            }
        }
    }
    if !c.errs.is_empty() {
        c.errs
            .sort_by(|a, b| (&a.file, a.line, a.col).cmp(&(&b.file, b.line, b.col)));
        return Err(c.errs);
    }
    let mut out = Program {
        files: mods.iter().map(|m| m.file.clone()).collect(),
        ..Program::default()
    };
    for (i, m) in mods.iter_mut().enumerate() {
        for mut f in std::mem::take(&mut m.prog.funcs) {
            f.name = qualify(&m.name, &f.name);
            f.file = i;
            out.funcs.push(f);
        }
        for mut s in std::mem::take(&mut m.prog.structs) {
            s.name = qualify(&m.name, &s.name);
            out.structs.push(s);
        }
    }
    Ok((out, c.structs))
}

/// Does every path through the block end in `return`?
fn returns(b: &[Stmt]) -> bool {
    b.iter().any(|s| match s {
        Stmt::Return(..) => true,
        Stmt::If(arms, Some(els)) => arms.iter().all(|(_, b)| returns(b)) && returns(els),
        _ => false,
    })
}

fn compat(a: &Ty, b: &Ty) -> bool {
    match (a, b) {
        (Ty::Err, _) | (_, Ty::Err) => true,
        (Ty::List(x), Ty::List(y)) => compat(x, y),
        _ => a == b,
    }
}

impl Checker {
    fn err(&mut self, l: u32, c: u32, m: String) {
        let mut e = Error::new("check", l, c, m);
        e.file = self.files[self.cur].clone();
        self.errs.push(e);
    }

    /// Resolve a function (`fns`) or struct name as written in the current
    /// module to its qualified name, reporting unknown or private items.
    fn item(&mut self, n: &str, fns: bool, l: u32, c: u32) -> Option<String> {
        let what = if fns { "function" } else { "struct" };
        let Some((alias, item)) = n.split_once('.') else {
            let q = qualify(&self.names[self.cur], n);
            let known = if fns {
                self.funcs.contains_key(&q)
            } else {
                self.structs.contains_key(&q)
            };
            if known {
                return Some(q);
            }
            let m = if fns {
                format!("undefined function '{}'", n)
            } else {
                format!("unknown struct '{}'", n)
            };
            self.err(l, c, m);
            return None;
        };
        let Some(&m) = self.aliases[self.cur].get(alias) else {
            self.err(
                l,
                c,
                format!("unknown module '{}' (missing 'import {};'?)", alias, alias),
            );
            return None;
        };
        let q = qualify(&self.names[m], item);
        let (known, public) = if fns {
            (self.funcs.contains_key(&q), self.pub_fns.contains(&q))
        } else {
            (self.structs.contains_key(&q), self.pub_structs.contains(&q))
        };
        if !known {
            self.err(
                l,
                c,
                format!("module '{}' has no {} '{}'", alias, what, item),
            );
            return None;
        }
        if !public {
            let m = format!(
                "{} '{}' in module '{}' is private (mark it 'pub')",
                what, item, alias
            );
            self.err(l, c, m);
            return None;
        }
        Some(q)
    }

    /// Resolve a written type, rewriting struct names to qualified names.
    fn resolve(&mut self, t: &mut TyExpr) -> Ty {
        match t {
            TyExpr::List(t) => Ty::List(Box::new(self.resolve(t))),
            TyExpr::Name(n, l, c) => match n.as_str() {
                "int" => Ty::Int,
                "float" => Ty::Float,
                "bool" => Ty::Bool,
                "str" => Ty::Str,
                _ if !n.contains('.')
                    && !self
                        .structs
                        .contains_key(&qualify(&self.names[self.cur], n)) =>
                {
                    self.err(*l, *c, format!("unknown type '{}'", n));
                    Ty::Err
                }
                _ => match self.item(&n.clone(), false, *l, *c) {
                    Some(q) => {
                        *n = q.clone();
                        Ty::Struct(q)
                    }
                    None => Ty::Err,
                },
            },
        }
    }

    fn lookup(&self, n: &str) -> Option<&Ty> {
        self.scopes.iter().rev().find_map(|s| s.get(n))
    }

    fn block(&mut self, b: &mut [Stmt], scope: bool) {
        if scope {
            self.scopes.push(HashMap::new());
        }
        for s in b {
            self.stmt(s);
        }
        if scope {
            self.scopes.pop();
        }
    }

    fn scoped_loop(&mut self, var: Option<(&str, Ty)>, b: &mut [Stmt]) {
        self.scopes.push(HashMap::new());
        if let Some((v, t)) = var {
            self.scopes.last_mut().unwrap().insert(v.to_string(), t);
        }
        self.loops += 1;
        self.block(b, true);
        self.loops -= 1;
        self.scopes.pop();
    }

    fn want(&mut self, e: &mut Expr, t: &Ty, what: &str) {
        let got = self.expr(e, Some(t));
        if !compat(&got, t) {
            self.err(
                e.line,
                e.col,
                format!("{} must be {}, got {}", what, t, got),
            );
        }
    }

    fn stmt(&mut self, s: &mut Stmt) {
        let is_break = matches!(s, Stmt::Break(..));
        match s {
            Stmt::Let(name, ann, e, l, c) => {
                let t = match ann {
                    Some(a) => {
                        let t = self.resolve(a);
                        self.want(e, &t, &format!("value of '{}'", name));
                        t
                    }
                    None => self.expr(e, None),
                };
                if t == Ty::Unit {
                    self.err(e.line, e.col, "cannot bind a unit value".into());
                }
                if self
                    .scopes
                    .last_mut()
                    .unwrap()
                    .insert(name.clone(), t)
                    .is_some()
                {
                    self.err(
                        *l,
                        *c,
                        format!("'{}' is already declared in this block", name),
                    );
                }
            }
            Stmt::Assign(target, v) => {
                if let ExprKind::Var(n) = &target.kind {
                    if self.lookup(n).is_none() {
                        self.err(
                            target.line,
                            target.col,
                            format!("assignment to undeclared variable '{}'", n),
                        );
                        self.expr(v, None);
                        return;
                    }
                }
                let t = self.expr(target, None);
                self.want(v, &t, "assigned value");
            }
            Stmt::Expr(e) => {
                self.expr(e, None);
            }
            Stmt::If(arms, els) => {
                for (cond, b) in arms {
                    self.want(cond, &Ty::Bool, "condition");
                    self.block(b, true);
                }
                if let Some(b) = els {
                    self.block(b, true);
                }
            }
            Stmt::While(cond, b) => {
                self.want(cond, &Ty::Bool, "condition");
                self.scoped_loop(None, b);
            }
            Stmt::ForRange(v, a, b, body) => {
                self.want(a, &Ty::Int, "range start");
                self.want(b, &Ty::Int, "range end");
                self.scoped_loop(Some((v, Ty::Int)), body);
            }
            Stmt::ForList(v, e, body) => {
                let t = match self.expr(e, None) {
                    Ty::List(t) => *t,
                    Ty::Err => Ty::Err,
                    t => {
                        self.err(
                            e.line,
                            e.col,
                            format!("can only iterate over a list or int range, got {}", t),
                        );
                        Ty::Err
                    }
                };
                self.scoped_loop(Some((v, t)), body);
            }
            Stmt::Break(l, c) | Stmt::Continue(l, c) => {
                if self.loops == 0 {
                    let w = if is_break { "break" } else { "continue" };
                    self.err(*l, *c, format!("'{}' outside of a loop", w));
                }
            }
            Stmt::Return(e, l, c) => {
                let r = self.ret.clone();
                match e {
                    Some(e) if r == Ty::Unit => {
                        self.expr(e, None);
                        self.err(
                            *l,
                            *c,
                            "function returns nothing but 'return' has a value".into(),
                        );
                    }
                    Some(e) => self.want(e, &r, "return value"),
                    None if r != Ty::Unit => {
                        self.err(*l, *c, format!("missing return value of type {}", r))
                    }
                    None => {}
                }
            }
        }
    }

    fn expr(&mut self, e: &mut Expr, hint: Option<&Ty>) -> Ty {
        let t = self.expr_inner(e, hint);
        e.ty = t.clone();
        t
    }

    fn expr_inner(&mut self, e: &mut Expr, hint: Option<&Ty>) -> Ty {
        let (l, c) = (e.line, e.col);
        match &mut e.kind {
            ExprKind::Int(_) => Ty::Int,
            ExprKind::Float(_) => Ty::Float,
            ExprKind::Str(_) => Ty::Str,
            ExprKind::Bool(_) => Ty::Bool,
            ExprKind::Var(n) => match self.lookup(n) {
                Some(t) => t.clone(),
                None => {
                    let n = n.clone();
                    self.err(l, c, format!("undefined variable '{}'", n));
                    Ty::Err
                }
            },
            ExprKind::List(items) => {
                let mut elem = match hint {
                    Some(Ty::List(t)) => Some((**t).clone()),
                    _ => None,
                };
                for it in items.iter_mut() {
                    match &elem {
                        Some(t) => {
                            let t = t.clone();
                            self.want(it, &t, "list element");
                        }
                        None => elem = Some(self.expr(it, None)),
                    }
                }
                match elem {
                    Some(t) => Ty::List(Box::new(t)),
                    None => {
                        self.err(l, c, "cannot infer the type of an empty list; add an annotation like 'let xs: [int] = [];'".into());
                        Ty::Err
                    }
                }
            }
            ExprKind::StructLit(name, fields) => {
                let q = self.item(&name.clone(), false, l, c);
                let Some(decl) = q.as_ref().map(|q| self.structs[q].clone()) else {
                    for (_, v) in fields.iter_mut() {
                        self.expr(v, None);
                    }
                    return Ty::Err;
                };
                for (f, v) in fields.iter_mut() {
                    match decl.iter().find(|(n, _)| n == f) {
                        Some((_, t)) => self.want(v, t, &format!("field '{}'", f)),
                        None => {
                            let m = format!("struct '{}' has no field '{}'", name, f);
                            self.err(v.line, v.col, m);
                            self.expr(v, None);
                        }
                    }
                }
                for (f, _) in &decl {
                    let n = fields.iter().filter(|(x, _)| x == f).count();
                    if n != 1 {
                        let m = format!(
                            "field '{}' {} in '{}' literal",
                            f,
                            if n == 0 { "missing" } else { "repeated" },
                            name
                        );
                        self.err(l, c, m);
                    }
                }
                let q = q.unwrap();
                *name = q.clone();
                Ty::Struct(q)
            }
            ExprKind::Unary(op, x) => {
                let t = self.expr(x, None);
                match (*op, &t) {
                    (_, Ty::Err) => Ty::Err,
                    ("not", Ty::Bool) => Ty::Bool,
                    ("-", Ty::Int | Ty::Float) => t,
                    _ => {
                        self.err(l, c, format!("cannot apply '{}' to {}", op, t));
                        Ty::Err
                    }
                }
            }
            ExprKind::Binary(op, a, b) => {
                let ta = self.expr(a, None);
                let tb = self.expr(b, Some(&ta));
                if ta == Ty::Err || tb == Ty::Err {
                    return if matches!(*op, "+" | "-" | "*" | "/" | "%") {
                        Ty::Err
                    } else {
                        Ty::Bool
                    };
                }
                let ok = match *op {
                    "and" | "or" => ta == Ty::Bool && tb == Ty::Bool,
                    "+" => ta == tb && matches!(ta, Ty::Int | Ty::Float | Ty::Str),
                    "-" | "*" | "/" | "%" => ta == tb && matches!(ta, Ty::Int | Ty::Float),
                    "==" | "!=" => {
                        ta == tb && matches!(ta, Ty::Int | Ty::Float | Ty::Str | Ty::Bool)
                    }
                    _ => ta == tb && matches!(ta, Ty::Int | Ty::Float | Ty::Str),
                };
                if !ok {
                    let hint = if matches!((&ta, &tb), (Ty::Int, Ty::Float) | (Ty::Float, Ty::Int))
                    {
                        " (use float(x) or int(x) to convert)"
                    } else {
                        ""
                    };
                    self.err(
                        l,
                        c,
                        format!("cannot apply '{}' to {} and {}{}", op, ta, tb, hint),
                    );
                    return Ty::Err;
                }
                if matches!(*op, "+" | "-" | "*" | "/" | "%") {
                    ta
                } else {
                    Ty::Bool
                }
            }
            ExprKind::Index(x, i) => {
                let t = self.expr(x, None);
                self.want(i, &Ty::Int, "index");
                match t {
                    Ty::List(t) => *t,
                    Ty::Err => Ty::Err,
                    t => {
                        self.err(l, c, format!("cannot index {}", t));
                        Ty::Err
                    }
                }
            }
            ExprKind::Field(x, f) => {
                let t = self.expr(x, None);
                match &t {
                    Ty::Struct(s) => match self.structs[s].iter().find(|(n, _)| n == f) {
                        Some((_, t)) => t.clone(),
                        None => {
                            let m = format!("struct '{}' has no field '{}'", s, f);
                            self.err(l, c, m);
                            Ty::Err
                        }
                    },
                    Ty::Err => Ty::Err,
                    t => {
                        let m = format!("{} has no fields", t);
                        self.err(l, c, m);
                        Ty::Err
                    }
                }
            }
            ExprKind::Call(name, args) => {
                if !BUILTINS.contains(&name.as_str()) {
                    match self.item(&name.clone(), true, l, c) {
                        Some(q) => *name = q,
                        None => {
                            for a in args.iter_mut() {
                                self.expr(a, None);
                            }
                            return Ty::Err;
                        }
                    }
                }
                let name = name.clone();
                self.call(&name, args, l, c)
            }
        }
    }

    fn call(&mut self, name: &str, args: &mut [Expr], l: u32, c: u32) -> Ty {
        let nargs = args.len();
        let pos: Vec<(u32, u32)> = args.iter().map(|a| (a.line, a.col)).collect();
        let arity = |n: std::ops::RangeInclusive<usize>, me: &mut Self| {
            if !n.contains(&nargs) {
                let want = if n.start() == n.end() {
                    n.start().to_string()
                } else {
                    format!("{} to {}", n.start(), n.end())
                };
                me.err(
                    l,
                    c,
                    format!("'{}' takes {} argument(s), got {}", name, want, nargs),
                );
                false
            } else {
                true
            }
        };
        if BUILTINS.contains(&name) {
            let ts: Vec<Ty> = match name {
                "push" => Vec::new(),
                _ => args.iter_mut().map(|a| self.expr(a, None)).collect(),
            };
            let bad = |me: &mut Self, i: usize, what: &str, ts: &[Ty]| {
                let m = format!("'{}' expects {}, got {}", name, what, ts[i]);
                me.err(pos[i].0, pos[i].1, m);
            };
            return match name {
                "print" => {
                    for (i, t) in ts.iter().enumerate() {
                        if *t == Ty::Unit {
                            bad(self, i, "a printable value", &ts);
                        }
                    }
                    Ty::Unit
                }
                "len" => {
                    if arity(1..=1, self) && !matches!(ts[0], Ty::List(_) | Ty::Str | Ty::Err) {
                        bad(self, 0, "a list or str", &ts);
                    }
                    Ty::Int
                }
                "push" => {
                    if !arity(2..=2, self) {
                        return Ty::Unit;
                    }
                    match self.expr(&mut args[0], None) {
                        Ty::List(t) => self.want(&mut args[1], &t, "pushed value"),
                        Ty::Err => {
                            self.expr(&mut args[1], None);
                        }
                        t => {
                            let m = format!("'push' expects a list, got {}", t);
                            self.err(args[0].line, args[0].col, m);
                        }
                    }
                    Ty::Unit
                }
                "pop" => {
                    if !arity(1..=1, self) {
                        return Ty::Err;
                    }
                    match &ts[0] {
                        Ty::List(t) => (**t).clone(),
                        Ty::Err => Ty::Err,
                        _ => {
                            bad(self, 0, "a list", &ts);
                            Ty::Err
                        }
                    }
                }
                "str" => {
                    if arity(1..=1, self) && ts[0] == Ty::Unit {
                        bad(self, 0, "a value", &ts);
                    }
                    Ty::Str
                }
                "int" | "float" => {
                    if arity(1..=1, self)
                        && !matches!(ts[0], Ty::Int | Ty::Float | Ty::Str | Ty::Err)
                    {
                        bad(self, 0, "an int, float or str", &ts);
                    }
                    if name == "int" {
                        Ty::Int
                    } else {
                        Ty::Float
                    }
                }
                "sqrt" => {
                    if arity(1..=1, self) && !compat(&ts[0], &Ty::Float) {
                        bad(self, 0, "a float", &ts);
                    }
                    Ty::Float
                }
                _ => {
                    if arity(1..=2, self) {
                        if !compat(&ts[0], &Ty::Bool) {
                            bad(self, 0, "a bool condition", &ts);
                        }
                        if ts.len() == 2 && !compat(&ts[1], &Ty::Str) {
                            bad(self, 1, "a str message", &ts);
                        }
                    }
                    Ty::Unit
                }
            };
        }
        let (ps, r) = self.funcs[name].clone();
        if ps.len() != args.len() {
            self.err(
                l,
                c,
                format!(
                    "'{}' takes {} argument(s), got {}",
                    name,
                    ps.len(),
                    args.len()
                ),
            );
        }
        for (i, a) in args.iter_mut().enumerate() {
            match ps.get(i) {
                Some(t) => self.want(a, t, &format!("argument {} of '{}'", i + 1, name)),
                None => {
                    self.expr(a, None);
                }
            }
        }
        r
    }
}
