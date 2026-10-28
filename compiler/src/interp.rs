//! Tree-walking interpreter with enforced resource limits: every statement
//! and call costs a step, and call depth is capped, so runaway programs stop
//! with an error instead of hanging.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
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

pub use crate::value::Value;

pub struct Func {
    pub(crate) name: String,
    params: Vec<String>,
    body: Rc<Vec<Stmt>>,
    env: Env,
}

type Env = Rc<RefCell<Scope>>;

#[derive(Default)]
pub struct Scope {
    vars: HashMap<String, Value>,
    parent: Option<Env>,
}

fn lookup(env: &Env, name: &str) -> Option<Value> {
    let mut env = env.clone();
    loop {
        let parent = {
            let scope = env.borrow();
            if let Some(v) = scope.vars.get(name) {
                return Some(v.clone());
            }
            scope.parent.clone()
        };
        env = parent?;
    }
}
fn assign(env: &Env, name: &str, value: Value) -> bool {
    let mut env = env.clone();
    loop {
        let parent = {
            let mut scope = env.borrow_mut();
            if let Some(slot) = scope.vars.get_mut(name) {
                *slot = value;
                return true;
            }
            scope.parent.clone()
        };
        match parent {
            Some(parent) => env = parent,
            None => return false,
        }
    }
}

pub const BUILTINS: &[&str] = &[
    "print", "len", "push", "pop", "str", "int", "float", "type", "keys", "assert", "get", "has",
    "join", "sum",
];

pub struct Interpreter<'a> {
    out: &'a mut dyn Write,
    limits: Limits,
    steps: u64,
    depth: usize,
    globals: Env,
    heap: TreeHeap,
    sandbox: crate::sandbox::State,
}

fn rt<T>(msg: impl Into<String>, span: Span) -> Result<T, LimError> {
    Err(LimError::new(Kind::Runtime, msg, span))
}

impl<'a> Interpreter<'a> {
    pub fn new(out: &'a mut dyn Write) -> Self {
        Self::with_limits(out, Limits::default())
    }

    pub fn with_limits(out: &'a mut dyn Write, limits: Limits) -> Self {
        Self::with_policy(out, limits, crate::sandbox::SandboxPolicy::default())
    }

    pub fn with_policy(
        out: &'a mut dyn Write,
        limits: Limits,
        policy: crate::sandbox::SandboxPolicy,
    ) -> Self {
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
            heap: TreeHeap {
                scopes: vec![globals.clone()],
                lists: HashMap::new(),
                maps: HashMap::new(),
                next_collection: 0,
            },
            globals,
            sandbox: crate::sandbox::State::new(policy),
        }
    }

    pub fn run(&mut self, prog: &Program) -> Result<(), LimError> {
        crate::ast::validate(prog)?;
        self.sandbox.start()?;
        self.depth = 0;
        let result = self.run_tasks(prog);
        self.depth = 0;
        self.heap.collect(&self.globals, &[], &[], &Task::Pop);
        result
    }

    fn run_tasks(&mut self, prog: &Program) -> Result<(), LimError> {
        let mut tasks = vec![Task::Block(Rc::new(prog.clone()), 0, self.globals.clone())];
        let mut values: Vec<Value> = Vec::new();
        while let Some(task) = tasks.pop() {
            if self.heap.needs_collection() {
                self.heap.collect(&self.globals, &tasks, &values, &task);
            }
            match task {
                Task::Block(body, index, env) => {
                    if let Some(stmt) = body.get(index) {
                        tasks.push(Task::Block(body.clone(), index + 1, env.clone()));
                        tasks.push(Task::Statement(stmt.clone(), env));
                    }
                }
                Task::Statement(stmt, env) => {
                    let s = stmt_span(&stmt);
                    self.tick(s)?;
                    match stmt {
                        Stmt::Let(name, e, _) => {
                            tasks.push(Task::Define(name, env.clone()));
                            tasks.push(Task::Expr(e, env));
                        }
                        Stmt::Assign(target, e, _) => {
                            match target {
                                Expr::Var(name, sp) => {
                                    tasks.push(Task::Assign(name, env.clone(), sp))
                                }
                                Expr::Index(o, i, sp) => {
                                    tasks.push(Task::SetIndex(sp));
                                    tasks.push(Task::Expr(*i, env.clone()));
                                    tasks.push(Task::Expr(*o, env.clone()));
                                }
                                _ => return rt("invalid assignment target", s),
                            }
                            tasks.push(Task::Expr(e, env));
                        }
                        Stmt::Expr(e, _) => {
                            tasks.push(Task::Pop);
                            tasks.push(Task::Expr(e, env));
                        }
                        Stmt::If(c, a, b, _) => {
                            tasks.push(Task::If(a, b, env.clone()));
                            tasks.push(Task::Expr(c, env));
                        }
                        Stmt::While(c, b, _) => {
                            tasks.push(Task::WhileNext(Rc::new(c), Rc::new(b), env, s))
                        }
                        Stmt::For(n, e, b, _) => {
                            tasks.push(Task::ForStart(n, Rc::new(b), env.clone(), s));
                            tasks.push(Task::Expr(e, env));
                        }
                        Stmt::Fn(name, params, body, _) => {
                            let f = Func {
                                name: name.clone(),
                                params,
                                body: Rc::new(body),
                                env: env.clone(),
                            };
                            env.borrow_mut().vars.insert(name, Value::Func(Rc::new(f)));
                        }
                        Stmt::Return(e, _) => {
                            tasks.push(Task::Return);
                            if let Some(e) = e {
                                tasks.push(Task::Expr(e, env));
                            } else {
                                values.push(Value::Nil);
                            }
                        }
                        Stmt::Break(_) | Stmt::Continue(_) => {
                            let is_break = matches!(stmt, Stmt::Break(_));
                            loop {
                                match tasks.pop() {
                                    Some(t @ (Task::WhileNext(..) | Task::ForNext(..))) => {
                                        if !is_break {
                                            tasks.push(t);
                                        }
                                        break;
                                    }
                                    Some(Task::CallEnd(_)) | None => {
                                        return rt(
                                            if is_break {
                                                "'break' outside of a loop"
                                            } else {
                                                "'continue' outside of a loop"
                                            },
                                            s,
                                        )
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                Task::Expr(e, env) => match e {
                    Expr::Int(n) => values.push(Value::Int(n)),
                    Expr::Float(n) => values.push(Value::Float(n)),
                    Expr::Str(s) => values.push(Value::Str(s.into())),
                    Expr::Bool(b) => values.push(Value::Bool(b)),
                    Expr::Nil => values.push(Value::Nil),
                    Expr::Var(n, s) => values.push(lookup(&env, &n).ok_or_else(|| {
                        LimError::new(Kind::Runtime, format!("undefined variable '{n}'"), s)
                    })?),
                    Expr::List(es) => {
                        tasks.push(Task::List(es.len()));
                        for e in es.into_iter().rev() {
                            tasks.push(Task::Expr(e, env.clone()));
                        }
                    }
                    Expr::Map(es, s) => {
                        let m = Rc::new(RefCell::new(BTreeMap::new()));
                        self.heap.track(&Value::Map(m.clone()));
                        tasks.push(Task::MapNext(es.into_iter(), m, env, s));
                    }
                    Expr::Unary(op, e, s) => {
                        tasks.push(Task::Unary(op, s));
                        tasks.push(Task::Expr(*e, env));
                    }
                    Expr::Binary(op, a, b, s) => {
                        if matches!(op, BinOp::And | BinOp::Or) {
                            tasks.push(Task::Logic(op, *b, env.clone()));
                            tasks.push(Task::Expr(*a, env));
                        } else {
                            tasks.push(Task::Binary(op, s));
                            tasks.push(Task::Expr(*b, env.clone()));
                            tasks.push(Task::Expr(*a, env));
                        }
                    }
                    Expr::Range(a, b, s) => {
                        tasks.push(Task::Range(s));
                        tasks.push(Task::Expr(*b, env.clone()));
                        tasks.push(Task::Expr(*a, env));
                    }
                    Expr::Index(a, b, s) => {
                        tasks.push(Task::Index(s));
                        tasks.push(Task::Expr(*b, env.clone()));
                        tasks.push(Task::Expr(*a, env));
                    }
                    Expr::Call(f, args, s) => {
                        tasks.push(Task::Call(args.len(), s));
                        for e in args.into_iter().rev() {
                            tasks.push(Task::Expr(e, env.clone()));
                        }
                        tasks.push(Task::Expr(*f, env));
                    }
                },
                Task::Define(n, env) => {
                    env.borrow_mut().vars.insert(n, values.pop().unwrap());
                }
                Task::Assign(n, env, s) => {
                    if !assign(&env, &n, values.pop().unwrap()) {
                        return rt(
                            format!("assignment to undeclared variable '{n}' (use 'let')"),
                            s,
                        );
                    }
                }
                Task::Pop => {
                    values.pop().unwrap();
                }
                Task::List(n) => {
                    let vs = values.split_off(values.len() - n);
                    values.push(Value::List(Rc::new(RefCell::new(vs))));
                }
                Task::MapNext(mut iter, m, env, s) => {
                    if let Some((k, v)) = iter.next() {
                        tasks.push(Task::MapKey(v, iter, m, env.clone(), s));
                        tasks.push(Task::Expr(k, env));
                    } else {
                        values.push(Value::Map(m));
                    }
                }
                Task::MapKey(v, iter, m, env, s) => {
                    let k = match values.pop().unwrap() {
                        Value::Str(k) => k,
                        other => {
                            return rt(
                                format!("map keys must be strings, got {}", other.type_name()),
                                s,
                            )
                        }
                    };
                    tasks.push(Task::MapInsert(k, iter, m, env.clone(), s));
                    tasks.push(Task::Expr(v, env));
                }
                Task::MapInsert(k, iter, m, env, s) => {
                    m.borrow_mut().insert(k.to_string(), values.pop().unwrap());
                    tasks.push(Task::MapNext(iter, m, env, s));
                }
                Task::Unary(op, s) => {
                    let v = values.pop().unwrap();
                    values.push(unary(op, v, s)?);
                }
                Task::Binary(op, s) => {
                    let b = values.pop().unwrap();
                    let a = values.pop().unwrap();
                    values.push(binary(op, a, b, s)?);
                }
                Task::Logic(op, b, env) => {
                    if values.last().unwrap().truthy() == (op == BinOp::And) {
                        values.pop();
                        tasks.push(Task::Expr(b, env));
                    }
                }
                Task::Range(s) => {
                    let b = values.pop().unwrap();
                    let a = values.pop().unwrap();
                    values.push(range(a, b, self.limits.max_steps, s)?);
                }
                Task::Index(s) => {
                    let i = values.pop().unwrap();
                    let o = values.pop().unwrap();
                    values.push(index(o, i, s)?);
                }
                Task::SetIndex(s) => {
                    let i = values.pop().unwrap();
                    let o = values.pop().unwrap();
                    let v = values.pop().unwrap();
                    self.set_index(&o, &i, v, s)?;
                }
                Task::If(a, b, env) => {
                    let body = if values.pop().unwrap().truthy() {
                        Some(a)
                    } else {
                        b
                    };
                    if let Some(body) = body {
                        let scope = self.heap.child(&env);
                        tasks.push(Task::Block(Rc::new(body), 0, scope));
                    }
                }
                Task::WhileNext(c, b, env, s) => {
                    tasks.push(Task::WhileCheck(c.clone(), b, env.clone(), s));
                    tasks.push(Task::Expr((*c).clone(), env));
                }
                Task::WhileCheck(c, b, env, s) => {
                    if values.pop().unwrap().truthy() {
                        self.tick(s)?;
                        let scope = self.heap.child(&env);
                        tasks.push(Task::WhileNext(c, b.clone(), env, s));
                        tasks.push(Task::Block(b, 0, scope));
                    }
                }
                Task::ForStart(n, b, env, s) => {
                    let items = match values.pop().unwrap() {
                        Value::List(v) => v.borrow().clone(),
                        Value::Str(v) => v
                            .chars()
                            .map(|c| Value::Str(c.to_string().into()))
                            .collect(),
                        Value::Map(v) => v
                            .borrow()
                            .keys()
                            .map(|k| Value::Str(k.as_str().into()))
                            .collect(),
                        other => {
                            return rt(format!("cannot iterate over {}", other.type_name()), s)
                        }
                    };
                    tasks.push(Task::ForNext(n, items.into_iter(), b, env, s));
                }
                Task::ForNext(n, mut iter, b, env, s) => {
                    if let Some(v) = iter.next() {
                        self.tick(s)?;
                        let scope = self.heap.child(&env);
                        scope.borrow_mut().vars.insert(n.clone(), v);
                        tasks.push(Task::ForNext(n, iter, b.clone(), env, s));
                        tasks.push(Task::Block(b, 0, scope));
                    }
                }
                Task::Call(n, s) => {
                    self.tick(s)?;
                    let args = values.split_off(values.len() - n);
                    let f = values.pop().unwrap();
                    match f {
                        Value::Builtin(n) => values.push(self.builtin(n, args, s)?),
                        Value::Func(f) => {
                            if args.len() != f.params.len() {
                                return rt(
                                    format!(
                                        "{}() takes {} argument(s), got {}",
                                        f.name,
                                        f.params.len(),
                                        args.len()
                                    ),
                                    s,
                                );
                            }
                            if self.depth >= self.limits.max_depth {
                                return rt(
                                    format!(
                                        "call depth limit exceeded ({})",
                                        self.limits.max_depth
                                    ),
                                    s,
                                );
                            }
                            let scope = self.heap.child(&f.env);
                            for (p, a) in f.params.iter().zip(args) {
                                scope.borrow_mut().vars.insert(p.clone(), a);
                            }
                            self.depth += 1;
                            tasks.push(Task::CallEnd(values.len()));
                            tasks.push(Task::Block(f.body.clone(), 0, scope));
                        }
                        other => return rt(format!("{} is not callable", other.type_name()), s),
                    }
                }
                Task::CallEnd(base) => {
                    self.depth -= 1;
                    values.truncate(base);
                    values.push(Value::Nil);
                }
                Task::Return => {
                    let v = values.pop().unwrap();
                    loop {
                        match tasks.pop() {
                            Some(Task::CallEnd(base)) => {
                                self.depth -= 1;
                                values.truncate(base);
                                values.push(v);
                                break;
                            }
                            None => return Ok(()),
                            _ => {}
                        }
                    }
                }
            }
            if let Some(v) = values.last() {
                self.heap.track(v);
            }
        }
        Ok(())
    }

    pub(crate) fn reset_steps(&mut self) {
        self.steps = 0;
    }

    pub(crate) fn reset_policy(&mut self) -> Result<(), LimError> {
        self.sandbox.reset()
    }

    pub(crate) fn tick(&mut self, span: Span) -> Result<(), LimError> {
        self.sandbox.checkpoint(span)?;
        if self.steps >= self.limits.max_steps {
            return rt(
                format!("step limit exceeded ({} steps)", self.limits.max_steps),
                span,
            );
        }
        self.steps += 1;
        Ok(())
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

    pub(crate) fn builtin(
        &mut self,
        name: &str,
        args: Vec<Value>,
        s: Span,
    ) -> Result<Value, LimError> {
        self.sandbox.check_builtin(name, s)?;
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
            "get" | "has" | "join" | "sum" => crate::stdlib::call(name, args, s),
            "print" => {
                let mut line = String::new();
                for (i, arg) in args.iter().enumerate() {
                    let value = arg
                        .render()
                        .map_err(|m| LimError::new(Kind::Runtime, m, s))?;
                    let framing_bytes = usize::from(i != 0) + 1; // separator and final newline
                    if line
                        .len()
                        .saturating_add(value.len())
                        .saturating_add(framing_bytes)
                        > crate::value::MAX_RENDER_BYTES
                    {
                        return rt("value rendering limit exceeded", s);
                    }
                    if i != 0 {
                        line.push(' ');
                    }
                    line.push_str(&value);
                }
                line.push('\n');
                self.sandbox.write(self.out, line.as_bytes(), s)?;
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
                Ok(Value::Str(
                    args[0]
                        .render()
                        .map_err(|m| LimError::new(Kind::Runtime, m, s))?
                        .into(),
                ))
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
                    .map(|m| m.render().map_err(|m| LimError::new(Kind::Runtime, m, s)))
                    .transpose()?
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
        BinOp::Eq => {
            return a
                .equals(&b)
                .map(Bool)
                .map_err(|m| LimError::new(Kind::Runtime, m, s))
        }
        BinOp::Ne => {
            return a
                .equals(&b)
                .map(|v| Bool(!v))
                .map_err(|m| LimError::new(Kind::Runtime, m, s))
        }
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
            let size = usize::try_from(n).map_err(|_| {
                LimError::new(Kind::Runtime, "range allocation exceeds host capacity", s)
            })?;
            let mut values = Vec::new();
            values.try_reserve_exact(size).map_err(|_| {
                LimError::new(Kind::Runtime, "range allocation exceeds host capacity", s)
            })?;
            values.extend((a..b).map(Value::Int));
            Value::List(Rc::new(RefCell::new(values)))
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

// AST continuations: language calls, blocks, loops, and expression evaluation
// use this heap stack. No Rust call chain grows with max_depth.
type Pairs = std::vec::IntoIter<(Expr, Expr)>;
type TreeMap = Rc<RefCell<BTreeMap<String, Value>>>;
enum Task {
    Block(Rc<Vec<Stmt>>, usize, Env),
    Statement(Stmt, Env),
    Expr(Expr, Env),
    Define(String, Env),
    Assign(String, Env, Span),
    Pop,
    List(usize),
    MapNext(Pairs, TreeMap, Env, Span),
    MapKey(Expr, Pairs, TreeMap, Env, Span),
    MapInsert(Rc<str>, Pairs, TreeMap, Env, Span),
    Unary(UnOp, Span),
    Binary(BinOp, Span),
    Logic(BinOp, Expr, Env),
    Range(Span),
    Index(Span),
    SetIndex(Span),
    If(Vec<Stmt>, Option<Vec<Stmt>>, Env),
    WhileNext(Rc<Expr>, Rc<Vec<Stmt>>, Env, Span),
    WhileCheck(Rc<Expr>, Rc<Vec<Stmt>>, Env, Span),
    ForStart(String, Rc<Vec<Stmt>>, Env, Span),
    ForNext(String, std::vec::IntoIter<Value>, Rc<Vec<Stmt>>, Env, Span),
    Call(usize, Span),
    CallEnd(usize),
    Return,
}
impl Task {
    fn roots(&self, envs: &mut Vec<Env>, values: &mut Vec<Value>) {
        let env = match self {
            Self::Block(_, _, e)
            | Self::Statement(_, e)
            | Self::Expr(_, e)
            | Self::Define(_, e)
            | Self::Assign(_, e, _)
            | Self::Logic(_, _, e)
            | Self::If(_, _, e)
            | Self::WhileNext(_, _, e, _)
            | Self::WhileCheck(_, _, e, _)
            | Self::ForStart(_, _, e, _) => Some(e),
            Self::ForNext(_, iter, _, e, _) => {
                values.extend(iter.as_slice().iter().cloned());
                Some(e)
            }
            Self::MapNext(_, m, e, _)
            | Self::MapKey(_, _, m, e, _)
            | Self::MapInsert(_, _, m, e, _) => {
                values.push(Value::Map(m.clone()));
                Some(e)
            }
            _ => None,
        };
        if let Some(e) = env {
            envs.push(e.clone());
        }
    }
}
#[derive(Default)]
struct TreeHeap {
    scopes: Vec<Env>,
    lists: HashMap<usize, Rc<RefCell<Vec<Value>>>>,
    maps: HashMap<usize, TreeMap>,
    next_collection: usize,
}
impl TreeHeap {
    fn child(&mut self, parent: &Env) -> Env {
        let env = Rc::new(RefCell::new(Scope {
            vars: HashMap::new(),
            parent: Some(parent.clone()),
        }));
        self.scopes.push(env.clone());
        env
    }
    fn track(&mut self, value: &Value) {
        match value {
            Value::List(v) => {
                self.lists
                    .entry(Rc::as_ptr(v) as usize)
                    .or_insert_with(|| v.clone());
            }
            Value::Map(v) => {
                self.maps
                    .entry(Rc::as_ptr(v) as usize)
                    .or_insert_with(|| v.clone());
            }
            _ => {}
        }
    }
    fn len(&self) -> usize {
        self.scopes.len() + self.lists.len() + self.maps.len()
    }
    fn needs_collection(&self) -> bool {
        self.len() >= self.next_collection.max(1024)
    }
    fn collect(&mut self, global: &Env, tasks: &[Task], stack: &[Value], current: &Task) {
        use std::collections::HashSet;
        let mut envs = vec![global.clone()];
        let mut values = stack.to_vec();
        for task in tasks {
            task.roots(&mut envs, &mut values);
        }
        current.roots(&mut envs, &mut values);
        let mut scopes = HashSet::new();
        let mut lists = HashSet::new();
        let mut maps = HashSet::new();
        while !envs.is_empty() || !values.is_empty() {
            if let Some(env) = envs.pop() {
                if !scopes.insert(Rc::as_ptr(&env) as usize) {
                    continue;
                }
                let scope = env.borrow();
                if let Some(parent) = &scope.parent {
                    envs.push(parent.clone());
                }
                values.extend(scope.vars.values().cloned());
            } else if let Some(v) = values.pop() {
                match v {
                    Value::Func(f) => envs.push(f.env.clone()),
                    Value::List(v) => {
                        if lists.insert(Rc::as_ptr(&v) as usize) {
                            values.extend(v.borrow().iter().cloned());
                        }
                    }
                    Value::Map(v) if maps.insert(Rc::as_ptr(&v) as usize) => {
                        values.extend(v.borrow().values().cloned());
                    }
                    _ => {}
                }
            }
        }
        for env in &self.scopes {
            if !scopes.contains(&(Rc::as_ptr(env) as usize)) {
                env.borrow_mut().vars.clear();
            }
        }
        for (id, v) in &self.lists {
            if !lists.contains(id) {
                v.borrow_mut().clear();
            }
        }
        for (id, v) in &self.maps {
            if !maps.contains(id) {
                v.borrow_mut().clear();
            }
        }
        let mut dead = Vec::new();
        self.scopes.retain(|s| {
            if scopes.contains(&(Rc::as_ptr(s) as usize)) {
                true
            } else {
                dead.push(s.clone());
                false
            }
        });
        while dead.pop().is_some() {}
        self.lists.retain(|id, _| lists.contains(id));
        self.maps.retain(|id, _| maps.contains(id));
        self.next_collection = self.len().saturating_mul(2).saturating_add(1024);
    }
}
impl Drop for TreeHeap {
    fn drop(&mut self) {
        for env in &self.scopes {
            env.borrow_mut().vars.clear();
        }
        for v in self.lists.values() {
            v.borrow_mut().clear();
        }
        for v in self.maps.values() {
            v.borrow_mut().clear();
        }
        while self.scopes.pop().is_some() {}
    }
}

#[cfg(test)]
mod counter_tests {
    use super::*;
    #[test]
    fn full_width_step_budget_never_wraps() {
        let mut out = Vec::new();
        let mut interpreter = Interpreter::with_limits(
            &mut out,
            Limits {
                max_steps: u64::MAX,
                max_depth: 1,
            },
        );
        interpreter.steps = u64::MAX - 1;
        let span = Span { line: 2, col: 3 };
        interpreter.tick(span).unwrap();
        let error = interpreter.tick(span).unwrap_err();
        assert_eq!(error.span, span);
        assert_eq!(
            error.msg,
            "step limit exceeded (18446744073709551615 steps)"
        );
    }
}
