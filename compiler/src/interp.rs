//! Tree-walking interpreter with enforced resource limits: every statement
//! and call costs a step, and call depth is capped, so runaway programs stop
//! with an error instead of hanging.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::io::Write;
use std::rc::Rc;

use crate::ast::*;
use crate::error::{Kind, LimError, Span};

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_steps: u64,
    pub max_depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_steps: 10_000_000,
            max_depth: 256,
        }
    }
}

#[derive(Clone)]
pub enum Value {
    Int(i64),
    Float(f64),
    Str(Rc<str>),
    Bool(bool),
    Nil,
    List(Rc<RefCell<Vec<Value>>>),
    Map(Rc<RefCell<BTreeMap<String, Value>>>),
    Func(Rc<Func>),
    Builtin(&'static str),
    VmFunc(Rc<crate::vm::Closure>),
}

pub struct Func {
    name: String,
    params: Vec<String>,
    body: Vec<Stmt>,
    env: Env,
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Str(_) => "string",
            Value::Bool(_) => "bool",
            Value::Nil => "nil",
            Value::List(_) => "list",
            Value::Map(_) => "map",
            Value::Func(_) | Value::VmFunc(_) | Value::Builtin(_) => "fn",
        }
    }

    pub(crate) fn truthy(&self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }
}

impl PartialEq for Value {
    fn eq(&self, o: &Value) -> bool {
        use Value::*;
        match (self, o) {
            (Int(a), Int(b)) => a == b,
            (Float(a), Float(b)) => a == b,
            (Int(a), Float(b)) | (Float(b), Int(a)) => (*a as f64) == *b,
            (Str(a), Str(b)) => a == b,
            (Bool(a), Bool(b)) => a == b,
            (Nil, Nil) => true,
            (List(a), List(b)) => *a.borrow() == *b.borrow(),
            (Map(a), Map(b)) => *a.borrow() == *b.borrow(),
            (Func(a), Func(b)) => Rc::ptr_eq(a, b),
            (VmFunc(a), VmFunc(b)) => Rc::ptr_eq(a, b),
            (Builtin(a), Builtin(b)) => a == b,
            _ => false,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Value::Int(n) => write!(f, "{}", n),
            Value::Float(x) => {
                if x.fract() == 0.0 && x.is_finite() {
                    write!(f, "{:.1}", x)
                } else {
                    write!(f, "{}", x)
                }
            }
            Value::Str(s) => write!(f, "{}", s),
            Value::Bool(b) => write!(f, "{}", b),
            Value::Nil => write!(f, "nil"),
            Value::List(l) => {
                write!(f, "[")?;
                for (i, v) in l.borrow().iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    v.fmt_repr(f)?;
                }
                write!(f, "]")
            }
            Value::Map(m) => {
                write!(f, "{{")?;
                for (i, (k, v)) in m.borrow().iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{:?}: ", k)?;
                    v.fmt_repr(f)?;
                }
                write!(f, "}}")
            }
            Value::Func(func) => write!(f, "<fn {}>", func.name),
            Value::VmFunc(func) => write!(f, "<fn {}>", func.function.name),
            Value::Builtin(n) => write!(f, "<builtin {}>", n),
        }
    }
}

impl Value {
    /// Like Display, but quotes strings (used inside containers).
    fn fmt_repr(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Value::Str(s) => write!(f, "{:?}", s),
            other => write!(f, "{}", other),
        }
    }
}

type Env = Rc<RefCell<Scope>>;

#[derive(Default)]
pub struct Scope {
    vars: HashMap<String, Value>,
    parent: Option<Env>,
}

fn child(parent: &Env) -> Env {
    Rc::new(RefCell::new(Scope {
        vars: HashMap::new(),
        parent: Some(parent.clone()),
    }))
}

fn lookup(env: &Env, name: &str) -> Option<Value> {
    let s = env.borrow();
    match s.vars.get(name) {
        Some(v) => Some(v.clone()),
        None => s.parent.as_ref().and_then(|p| lookup(p, name)),
    }
}

fn assign(env: &Env, name: &str, v: Value) -> bool {
    let mut s = env.borrow_mut();
    if let Some(slot) = s.vars.get_mut(name) {
        *slot = v;
        return true;
    }
    match s.parent.clone() {
        Some(p) => {
            drop(s);
            assign(&p, name, v)
        }
        None => false,
    }
}

enum Flow {
    Normal,
    Return(Value),
    Break(Span),
    Continue(Span),
}

pub const BUILTINS: &[&str] = &[
    "print", "len", "push", "pop", "str", "int", "float", "type", "keys", "assert",
];

pub struct Interpreter<'a> {
    out: &'a mut dyn Write,
    limits: Limits,
    steps: u64,
    depth: usize,
    globals: Env,
}

fn rt<T>(msg: impl Into<String>, span: Span) -> Result<T, LimError> {
    Err(LimError::new(Kind::Runtime, msg, span))
}

impl<'a> Interpreter<'a> {
    pub fn new(out: &'a mut dyn Write) -> Self {
        Self::with_limits(out, Limits::default())
    }

    pub fn with_limits(out: &'a mut dyn Write, limits: Limits) -> Self {
        let globals: Env = Rc::new(RefCell::new(Scope::default()));
        for b in BUILTINS {
            globals
                .borrow_mut()
                .vars
                .insert(b.to_string(), Value::Builtin(b));
        }
        Interpreter {
            out,
            limits,
            steps: 0,
            depth: 0,
            globals,
        }
    }

    pub fn run(&mut self, prog: &Program) -> Result<(), LimError> {
        let env = self.globals.clone();
        match self.exec_block(prog, &env)? {
            Flow::Normal | Flow::Return(_) => Ok(()),
            Flow::Break(s) => rt("'break' outside of a loop", s),
            Flow::Continue(s) => rt("'continue' outside of a loop", s),
        }
    }

    pub(crate) fn tick(&mut self, span: Span) -> Result<(), LimError> {
        self.steps += 1;
        if self.steps > self.limits.max_steps {
            return rt(
                format!("step limit exceeded ({} steps)", self.limits.max_steps),
                span,
            );
        }
        Ok(())
    }

    fn exec_block(&mut self, stmts: &[Stmt], env: &Env) -> Result<Flow, LimError> {
        for s in stmts {
            match self.exec(s, env)? {
                Flow::Normal => {}
                other => return Ok(other),
            }
        }
        Ok(Flow::Normal)
    }

    fn exec(&mut self, stmt: &Stmt, env: &Env) -> Result<Flow, LimError> {
        self.tick(stmt_span(stmt))?;
        match stmt {
            Stmt::Let(name, e, _) => {
                let v = self.eval(e, env)?;
                env.borrow_mut().vars.insert(name.clone(), v);
            }
            Stmt::Assign(..) => return self.exec_assign(stmt, env),
            Stmt::Expr(e, _) => {
                self.eval(e, env)?;
            }
            Stmt::If(..) => return self.exec_if(stmt, env),
            Stmt::While(..) => return self.exec_while(stmt, env),
            Stmt::For(..) => return self.exec_for(stmt, env),
            Stmt::Fn(..) => return self.exec_fn(stmt, env),
            Stmt::Return(e, _) => {
                let v = match e {
                    Some(e) => self.eval(e, env)?,
                    None => Value::Nil,
                };
                return Ok(Flow::Return(v));
            }
            Stmt::Break(s) => return Ok(Flow::Break(*s)),
            Stmt::Continue(s) => return Ok(Flow::Continue(*s)),
        }
        Ok(Flow::Normal)
    }

    pub(crate) fn set_index(
        &mut self,
        o: &Value,
        i: &Value,
        v: Value,
        s: Span,
    ) -> Result<(), LimError> {
        match (o, i) {
            (Value::List(l), Value::Int(n)) => {
                let mut l = l.borrow_mut();
                let idx = norm_index(*n, l.len()).ok_or_else(|| {
                    LimError::new(
                        Kind::Runtime,
                        format!("index {} out of bounds (len {})", n, l.len()),
                        s,
                    )
                })?;
                l[idx] = v;
                Ok(())
            }
            (Value::Map(m), Value::Str(k)) => {
                m.borrow_mut().insert(k.to_string(), v);
                Ok(())
            }
            _ => rt(
                format!(
                    "cannot index-assign {} with {}",
                    o.type_name(),
                    i.type_name()
                ),
                s,
            ),
        }
    }

    // Keep dispatch small: large match-arm temporaries otherwise make each
    // recursive call consume several kilobytes on debug/test thread stacks.
    fn eval(&mut self, e: &Expr, env: &Env) -> Result<Value, LimError> {
        match e {
            Expr::Int(n) => Ok(Value::Int(*n)),
            Expr::Float(n) => Ok(Value::Float(*n)),
            Expr::Str(s) => Ok(Value::Str(s.as_str().into())),
            Expr::Bool(b) => Ok(Value::Bool(*b)),
            Expr::Nil => Ok(Value::Nil),
            Expr::Var(name, s) => Self::get_variable(env, name, *s),
            Expr::List(_) => self.eval_list(e, env),
            Expr::Map(..) => self.eval_map(e, env),
            Expr::Unary(op, x, s) => self.eval_unary(*op, x, env, *s),
            Expr::Binary(op, l, r, s) => self.eval_binary(*op, l, r, env, *s),
            Expr::Range(a, b, s) => self.eval_range(a, b, env, *s),
            Expr::Index(a, b, s) => self.eval_index(a, b, env, *s),
            Expr::Call(..) => self.eval_call(e, env),
        }
    }
    fn get_variable(env: &Env, name: &str, s: Span) -> Result<Value, LimError> {
        lookup(env, name).ok_or_else(|| {
            LimError::new(Kind::Runtime, format!("undefined variable '{}'", name), s)
        })
    }
    fn eval_unary(&mut self, op: UnOp, x: &Expr, env: &Env, s: Span) -> Result<Value, LimError> {
        unary(op, self.eval(x, env)?, s)
    }
    fn eval_binary(
        &mut self,
        op: BinOp,
        l: &Expr,
        r: &Expr,
        env: &Env,
        s: Span,
    ) -> Result<Value, LimError> {
        let a = self.eval(l, env)?;
        match op {
            BinOp::And if !a.truthy() => Ok(a),
            BinOp::Or if a.truthy() => Ok(a),
            BinOp::And | BinOp::Or => self.eval(r, env),
            _ => binary(op, a, self.eval(r, env)?, s),
        }
    }
    fn eval_range(&mut self, a: &Expr, b: &Expr, env: &Env, s: Span) -> Result<Value, LimError> {
        range(
            self.eval(a, env)?,
            self.eval(b, env)?,
            self.limits.max_steps,
            s,
        )
    }
    fn eval_index(&mut self, a: &Expr, b: &Expr, env: &Env, s: Span) -> Result<Value, LimError> {
        index(self.eval(a, env)?, self.eval(b, env)?, s)
    }

    fn exec_assign(&mut self, stmt: &Stmt, env: &Env) -> Result<Flow, LimError> {
        let Stmt::Assign(target, e, span) = stmt else {
            unreachable!()
        };

        let v = self.eval(e, env)?;
        match target {
            Expr::Var(name, s) => {
                if !assign(env, name, v) {
                    return rt(
                        format!("assignment to undeclared variable '{}' (use 'let')", name),
                        *s,
                    );
                }
            }
            Expr::Index(obj, idx, s) => {
                let o = self.eval(obj, env)?;
                let i = self.eval(idx, env)?;
                self.set_index(&o, &i, v, *s)?;
            }
            _ => return rt("invalid assignment target", *span),
        }

        Ok(Flow::Normal)
    }
    fn exec_if(&mut self, stmt: &Stmt, env: &Env) -> Result<Flow, LimError> {
        let Stmt::If(c, then, els, _) = stmt else {
            unreachable!()
        };

        if self.eval(c, env)?.truthy() {
            return self.exec_block(then, &child(env));
        } else if let Some(b) = els {
            return self.exec_block(b, &child(env));
        }

        Ok(Flow::Normal)
    }
    fn exec_while(&mut self, stmt: &Stmt, env: &Env) -> Result<Flow, LimError> {
        let Stmt::While(c, body, span) = stmt else {
            unreachable!()
        };

        while self.eval(c, env)?.truthy() {
            // Charge each iteration so even an empty body counts against the budget.
            self.tick(*span)?;
            match self.exec_block(body, &child(env))? {
                Flow::Break(_) => break,
                Flow::Return(v) => return Ok(Flow::Return(v)),
                Flow::Normal | Flow::Continue(_) => {}
            }
        }

        Ok(Flow::Normal)
    }
    fn exec_for(&mut self, stmt: &Stmt, env: &Env) -> Result<Flow, LimError> {
        let Stmt::For(name, it, body, span) = stmt else {
            unreachable!()
        };

        let items: Vec<Value> = match self.eval(it, env)? {
            Value::List(l) => l.borrow().clone(),
            Value::Str(s) => s
                .chars()
                .map(|c| Value::Str(c.to_string().into()))
                .collect(),
            Value::Map(m) => m
                .borrow()
                .keys()
                .map(|k| Value::Str(k.as_str().into()))
                .collect(),
            other => return rt(format!("cannot iterate over {}", other.type_name()), *span),
        };
        for item in items {
            self.tick(*span)?;
            let scope = child(env);
            scope.borrow_mut().vars.insert(name.clone(), item);
            match self.exec_block(body, &scope)? {
                Flow::Break(_) => break,
                Flow::Return(v) => return Ok(Flow::Return(v)),
                Flow::Normal | Flow::Continue(_) => {}
            }
        }

        Ok(Flow::Normal)
    }
    fn exec_fn(&mut self, stmt: &Stmt, env: &Env) -> Result<Flow, LimError> {
        let Stmt::Fn(name, params, body, _) = stmt else {
            unreachable!()
        };

        let f = Func {
            name: name.clone(),
            params: params.clone(),
            body: body.clone(),
            env: env.clone(),
        };
        env.borrow_mut()
            .vars
            .insert(name.clone(), Value::Func(Rc::new(f)));

        Ok(Flow::Normal)
    }
    fn eval_list(&mut self, e: &Expr, env: &Env) -> Result<Value, LimError> {
        let Expr::List(items) = e else { unreachable!() };
        Ok({
            let mut v = Vec::with_capacity(items.len());
            for it in items {
                v.push(self.eval(it, env)?);
            }
            Value::List(Rc::new(RefCell::new(v)))
        })
    }
    fn eval_map(&mut self, e: &Expr, env: &Env) -> Result<Value, LimError> {
        let Expr::Map(pairs, ms) = e else {
            unreachable!()
        };
        Ok({
            let mut m = BTreeMap::new();
            for (k, v) in pairs {
                let key = match self.eval(k, env)? {
                    Value::Str(s) => s.to_string(),
                    other => {
                        return rt(
                            format!("map keys must be strings, got {}", other.type_name()),
                            *ms,
                        )
                    }
                };
                m.insert(key, self.eval(v, env)?);
            }
            Value::Map(Rc::new(RefCell::new(m)))
        })
    }
    fn eval_call(&mut self, e: &Expr, env: &Env) -> Result<Value, LimError> {
        let Expr::Call(callee, args, s) = e else {
            unreachable!()
        };
        Ok({
            let f = self.eval(callee, env)?;
            let mut argv = Vec::with_capacity(args.len());
            for a in args {
                argv.push(self.eval(a, env)?);
            }
            self.call(f, argv, *s)?
        })
    }
    fn call(&mut self, f: Value, args: Vec<Value>, s: Span) -> Result<Value, LimError> {
        self.tick(s)?;
        match f {
            Value::Builtin(name) => self.builtin(name, args, s),
            Value::Func(func) => {
                if args.len() != func.params.len() {
                    return rt(
                        format!(
                            "{}() takes {} argument(s), got {}",
                            func.name,
                            func.params.len(),
                            args.len()
                        ),
                        s,
                    );
                }
                if self.depth >= self.limits.max_depth {
                    return rt(
                        format!("call depth limit exceeded ({})", self.limits.max_depth),
                        s,
                    );
                }
                let scope = child(&func.env);
                for (p, a) in func.params.iter().zip(args) {
                    scope.borrow_mut().vars.insert(p.clone(), a);
                }
                self.depth += 1;
                let r = self.exec_block(&func.body, &scope);
                self.depth -= 1;
                match r? {
                    Flow::Return(v) => Ok(v),
                    Flow::Normal => Ok(Value::Nil),
                    Flow::Break(bs) => rt("'break' outside of a loop", bs),
                    Flow::Continue(cs) => rt("'continue' outside of a loop", cs),
                }
            }
            other => rt(format!("{} is not callable", other.type_name()), s),
        }
    }

    pub(crate) fn builtin(
        &mut self,
        name: &str,
        args: Vec<Value>,
        s: Span,
    ) -> Result<Value, LimError> {
        let arity = |n: usize| -> Result<(), LimError> {
            if args.len() != n {
                rt(
                    format!("{}() takes {} argument(s), got {}", name, n, args.len()),
                    s,
                )
            } else {
                Ok(())
            }
        };
        match name {
            "print" => {
                let line: Vec<String> = args.iter().map(|a| a.to_string()).collect();
                writeln!(self.out, "{}", line.join(" "))
                    .map_err(|e| LimError::new(Kind::Runtime, e.to_string(), s))?;
                Ok(Value::Nil)
            }
            "len" => {
                arity(1)?;
                Ok(Value::Int(match &args[0] {
                    Value::Str(st) => st.chars().count() as i64,
                    Value::List(l) => l.borrow().len() as i64,
                    Value::Map(m) => m.borrow().len() as i64,
                    v => return rt(format!("len() of {}", v.type_name()), s),
                }))
            }
            "push" => {
                arity(2)?;
                match &args[0] {
                    Value::List(l) => {
                        l.borrow_mut().push(args[1].clone());
                        Ok(Value::Nil)
                    }
                    v => rt(format!("push() expects a list, got {}", v.type_name()), s),
                }
            }
            "pop" => {
                arity(1)?;
                match &args[0] {
                    Value::List(l) => l
                        .borrow_mut()
                        .pop()
                        .map_or_else(|| rt("pop() from empty list", s), Ok),
                    v => rt(format!("pop() expects a list, got {}", v.type_name()), s),
                }
            }
            "str" => {
                arity(1)?;
                Ok(Value::Str(args[0].to_string().into()))
            }
            "int" => {
                arity(1)?;
                match &args[0] {
                    Value::Int(n) => Ok(Value::Int(*n)),
                    Value::Float(x) if x.is_finite() => Ok(Value::Int(x.trunc() as i64)),
                    Value::Str(st) => st
                        .trim()
                        .parse()
                        .map(Value::Int)
                        .or_else(|_| rt(format!("cannot parse {:?} as int", st), s)),
                    Value::Bool(b) => Ok(Value::Int(*b as i64)),
                    v => rt(format!("cannot convert {} to int", v.type_name()), s),
                }
            }
            "float" => {
                arity(1)?;
                match &args[0] {
                    Value::Int(n) => Ok(Value::Float(*n as f64)),
                    Value::Float(x) => Ok(Value::Float(*x)),
                    Value::Str(st) => st
                        .trim()
                        .parse()
                        .map(Value::Float)
                        .or_else(|_| rt(format!("cannot parse {:?} as float", st), s)),
                    v => rt(format!("cannot convert {} to float", v.type_name()), s),
                }
            }
            "type" => {
                arity(1)?;
                Ok(Value::Str(args[0].type_name().into()))
            }
            "keys" => {
                arity(1)?;
                match &args[0] {
                    Value::Map(m) => Ok(Value::List(Rc::new(RefCell::new(
                        m.borrow()
                            .keys()
                            .map(|k| Value::Str(k.as_str().into()))
                            .collect(),
                    )))),
                    v => rt(format!("keys() expects a map, got {}", v.type_name()), s),
                }
            }
            "assert" => {
                if args.is_empty() || args.len() > 2 {
                    return rt("assert() takes 1 or 2 arguments", s);
                }
                if args[0].truthy() {
                    return Ok(Value::Nil);
                }
                let msg = args
                    .get(1)
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| "assertion failed".into());
                rt(msg, s)
            }
            _ => unreachable!("unknown builtin {}", name),
        }
    }
}

pub(crate) fn overflow(s: Span) -> LimError {
    LimError::new(Kind::Runtime, "integer overflow", s)
}

fn norm_index(n: i64, len: usize) -> Option<usize> {
    let i = if n < 0 { n + len as i64 } else { n };
    if i >= 0 && (i as usize) < len {
        Some(i as usize)
    } else {
        None
    }
}

#[inline]
pub(crate) fn binary(op: BinOp, a: Value, b: Value, s: Span) -> Result<Value, LimError> {
    if let (Value::Int(x), Value::Int(y)) = (&a, &b) {
        let n = match op {
            BinOp::Add => x.checked_add(*y),
            BinOp::Sub => x.checked_sub(*y),
            BinOp::Mul => x.checked_mul(*y),
            BinOp::Div | BinOp::Mod if *y == 0 => return rt("division by zero", s),
            BinOp::Div => x.checked_div(*y),
            BinOp::Mod => x.checked_rem(*y),
            BinOp::Eq => return Ok(Value::Bool(x == y)),
            BinOp::Ne => return Ok(Value::Bool(x != y)),
            // Preserve the tree walker's numeric comparison rounding.
            BinOp::Lt => return Ok(Value::Bool((*x as f64) < (*y as f64))),
            BinOp::Le => return Ok(Value::Bool((*x as f64) <= (*y as f64))),
            BinOp::Gt => return Ok(Value::Bool((*x as f64) > (*y as f64))),
            BinOp::Ge => return Ok(Value::Bool((*x as f64) >= (*y as f64))),
            _ => unreachable!(),
        };
        return n.map(Value::Int).ok_or_else(|| overflow(s));
    }
    binary_other(op, a, b, s)
}

fn binary_other(op: BinOp, a: Value, b: Value, s: Span) -> Result<Value, LimError> {
    use Value::*;
    match op {
        BinOp::Eq => return Ok(Bool(a == b)),
        BinOp::Ne => return Ok(Bool(a != b)),
        _ => {}
    }
    Ok(match (op, &a, &b) {
        (BinOp::Add, Int(x), Int(y)) => Int(x.checked_add(*y).ok_or_else(|| overflow(s))?),
        (BinOp::Sub, Int(x), Int(y)) => Int(x.checked_sub(*y).ok_or_else(|| overflow(s))?),
        (BinOp::Mul, Int(x), Int(y)) => Int(x.checked_mul(*y).ok_or_else(|| overflow(s))?),
        (BinOp::Div | BinOp::Mod, Int(_), Int(0)) => return rt("division by zero", s),
        (BinOp::Div, Int(x), Int(y)) => Int(x.checked_div(*y).ok_or_else(|| overflow(s))?),
        (BinOp::Mod, Int(x), Int(y)) => Int(x.checked_rem(*y).ok_or_else(|| overflow(s))?),
        (BinOp::Add, Str(x), Str(y)) => Str(format!("{}{}", x, y).into()),
        (BinOp::Add, List(x), List(y)) => {
            let mut v = x.borrow().clone();
            v.extend(y.borrow().iter().cloned());
            List(Rc::new(RefCell::new(v)))
        }
        (BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge, Str(x), Str(y)) => {
            Bool(cmp_op(op, x.cmp(y)))
        }
        _ => {
            let (x, y) = match (&a, &b) {
                (Int(x), Float(y)) => (*x as f64, *y),
                (Float(x), Int(y)) => (*x, *y as f64),
                (Float(x), Float(y)) => (*x, *y),
                (Int(x), Int(y)) => (*x as f64, *y as f64),
                _ => {
                    return rt(
                        format!(
                            "unsupported operands for {:?}: {} and {}",
                            op,
                            a.type_name(),
                            b.type_name()
                        ),
                        s,
                    )
                }
            };
            match op {
                BinOp::Add => Float(x + y),
                BinOp::Sub => Float(x - y),
                BinOp::Mul => Float(x * y),
                BinOp::Div => Float(x / y),
                BinOp::Mod => Float(x % y),
                _ => match x.partial_cmp(&y) {
                    Some(o) => Bool(cmp_op(op, o)),
                    None => Bool(false),
                },
            }
        }
    })
}

fn cmp_op(op: BinOp, o: std::cmp::Ordering) -> bool {
    use std::cmp::Ordering::*;
    match op {
        BinOp::Lt => o == Less,
        BinOp::Le => o != Greater,
        BinOp::Gt => o == Greater,
        BinOp::Ge => o != Less,
        _ => unreachable!(),
    }
}

pub(crate) fn stmt_span(s: &Stmt) -> Span {
    match s {
        Stmt::Let(.., sp)
        | Stmt::Assign(.., sp)
        | Stmt::Expr(_, sp)
        | Stmt::If(.., sp)
        | Stmt::While(.., sp)
        | Stmt::For(.., sp)
        | Stmt::Fn(.., sp)
        | Stmt::Return(_, sp)
        | Stmt::Break(sp)
        | Stmt::Continue(sp) => *sp,
    }
}

pub(crate) fn unary(op: UnOp, v: Value, s: Span) -> Result<Value, LimError> {
    Ok(match (op, v) {
        (UnOp::Not, v) => Value::Bool(!v.truthy()),
        (UnOp::Neg, Value::Int(n)) => Value::Int(n.checked_neg().ok_or_else(|| overflow(s))?),
        (UnOp::Neg, Value::Float(x)) => Value::Float(-x),
        (UnOp::Neg, v) => return rt(format!("cannot negate {}", v.type_name()), s),
    })
}

pub(crate) fn range(a: Value, b: Value, max_steps: u64, s: Span) -> Result<Value, LimError> {
    Ok(match (a, b) {
        (Value::Int(a), Value::Int(b)) => {
            let n = if b > a { b.abs_diff(a) } else { 0 };
            if n > max_steps {
                return rt(format!("range of {} elements exceeds step limit", n), s);
            }
            Value::List(Rc::new(RefCell::new((a..b).map(Value::Int).collect())))
        }
        (a, b) => {
            return rt(
                format!(
                    "range bounds must be ints, got {} and {}",
                    a.type_name(),
                    b.type_name()
                ),
                s,
            )
        }
    })
}

pub(crate) fn index(o: Value, i: Value, s: Span) -> Result<Value, LimError> {
    Ok(match (&o, &i) {
        (Value::List(l), Value::Int(n)) => {
            let l = l.borrow();
            match norm_index(*n, l.len()) {
                Some(idx) => l[idx].clone(),
                None => return rt(format!("index {} out of bounds (len {})", n, l.len()), s),
            }
        }
        (Value::Str(st), Value::Int(n)) => {
            let chars: Vec<char> = st.chars().collect();
            match norm_index(*n, chars.len()) {
                Some(idx) => Value::Str(chars[idx].to_string().into()),
                None => {
                    return rt(
                        format!("index {} out of bounds (len {})", n, chars.len()),
                        s,
                    )
                }
            }
        }
        (Value::Map(m), Value::Str(k)) => m.borrow().get(&**k).cloned().unwrap_or(Value::Nil),
        _ => {
            return rt(
                format!("cannot index {} with {}", o.type_name(), i.type_name()),
                s,
            )
        }
    })
}
