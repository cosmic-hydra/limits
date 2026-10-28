//! Shared values with iterative, cycle-aware comparison and rendering.
use crate::interp::Func;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::fmt::{self, Write};
use std::rc::Rc;

// Host-work guards are separate from statement/call step accounting. They stop
// a compact shared DAG from requesting exponential work in one operation.
const MAX_VALUE_WORK: usize = 1_000_000;
pub(crate) const MAX_RENDER_BYTES: usize = 16 * 1024 * 1024;

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

impl Value {
    pub(crate) fn equals(&self, other: &Value) -> Result<bool, &'static str> {
        let mut work = vec![(self.clone(), other.clone())];
        let mut visited = HashSet::new();
        let mut count = 0;
        while let Some((a, b)) = work.pop() {
            count += 1;
            if count > MAX_VALUE_WORK {
                return Err("value comparison limit exceeded");
            }
            match (a, b) {
                (Self::Int(a), Self::Int(b)) if a == b => {}
                (Self::Float(a), Self::Float(b)) if a == b => {}
                (Self::Int(a), Self::Float(b)) | (Self::Float(b), Self::Int(a))
                    if a as f64 == b => {}
                (Self::Str(a), Self::Str(b)) if a == b => {}
                (Self::Bool(a), Self::Bool(b)) if a == b => {}
                (Self::Nil, Self::Nil) => {}
                (Self::Func(a), Self::Func(b)) if Rc::ptr_eq(&a, &b) => {}
                (Self::VmFunc(a), Self::VmFunc(b)) if Rc::ptr_eq(&a, &b) => {}
                (Self::Builtin(a), Self::Builtin(b)) if a == b => {}
                (Self::List(a), Self::List(b)) => {
                    if !visited.insert((0, Rc::as_ptr(&a) as usize, Rc::as_ptr(&b) as usize)) {
                        continue;
                    }
                    let (a, b) = (a.borrow(), b.borrow());
                    if a.len() != b.len() {
                        return Ok(false);
                    }
                    if a.len() > MAX_VALUE_WORK.saturating_sub(count + work.len()) {
                        return Err("value comparison limit exceeded");
                    }
                    work.extend(a.iter().cloned().zip(b.iter().cloned()));
                }
                (Self::Map(a), Self::Map(b)) => {
                    if !visited.insert((1, Rc::as_ptr(&a) as usize, Rc::as_ptr(&b) as usize)) {
                        continue;
                    }
                    let (a, b) = (a.borrow(), b.borrow());
                    if a.len() != b.len() {
                        return Ok(false);
                    }
                    if a.len() > MAX_VALUE_WORK.saturating_sub(count + work.len()) {
                        return Err("value comparison limit exceeded");
                    }
                    for ((ka, va), (kb, vb)) in a.iter().zip(b.iter()) {
                        if ka != kb {
                            return Ok(false);
                        }
                        work.push((va.clone(), vb.clone()));
                    }
                }
                _ => return Ok(false),
            }
        }
        Ok(true)
    }
    pub(crate) fn render(&self) -> Result<String, &'static str> {
        enum Part {
            Value(Value, bool),
            Text(String),
            End((u8, usize), char),
        }
        let mut out = String::new();
        let mut work = vec![Part::Value(self.clone(), false)];
        let mut active = HashSet::new();
        let mut count = 0;
        while let Some(part) = work.pop() {
            count += 1;
            if count > MAX_VALUE_WORK {
                return Err("value rendering limit exceeded");
            }
            match part {
                Part::Text(s) => out.push_str(&s),
                Part::End(id, c) => {
                    active.remove(&id);
                    out.push(c);
                }
                Part::Value(v, quote) => match v {
                    Self::Int(n) => {
                        write!(out, "{n}").unwrap();
                    }
                    Self::Float(x) if x.fract() == 0.0 && x.is_finite() => {
                        write!(out, "{x:.1}").unwrap();
                    }
                    Self::Float(x) => {
                        write!(out, "{x}").unwrap();
                    }
                    Self::Str(s) if quote => {
                        write!(out, "{s:?}").unwrap();
                    }
                    Self::Str(s) => out.push_str(&s),
                    Self::Bool(b) => {
                        write!(out, "{b}").unwrap();
                    }
                    Self::Nil => out.push_str("nil"),
                    Self::Func(f) => {
                        write!(out, "<fn {}>", f.name).unwrap();
                    }
                    Self::VmFunc(f) => {
                        write!(out, "<fn {}>", f.function.name).unwrap();
                    }
                    Self::Builtin(n) => {
                        write!(out, "<builtin {n}>").unwrap();
                    }
                    Self::List(v) => {
                        let id = (0, Rc::as_ptr(&v) as usize);
                        if !active.insert(id) {
                            out.push_str("[...]");
                            continue;
                        }
                        let v = v.borrow();
                        if v.len() > (MAX_VALUE_WORK.saturating_sub(count + work.len())) / 2 {
                            return Err("value rendering limit exceeded");
                        }
                        out.push('[');
                        work.push(Part::End(id, ']'));
                        for (i, v) in v.iter().enumerate().rev() {
                            work.push(Part::Value(v.clone(), true));
                            if i > 0 {
                                work.push(Part::Text(", ".into()));
                            }
                        }
                    }
                    Self::Map(v) => {
                        let id = (1, Rc::as_ptr(&v) as usize);
                        if !active.insert(id) {
                            out.push_str("{...}");
                            continue;
                        }
                        let v = v.borrow();
                        if v.len() > (MAX_VALUE_WORK.saturating_sub(count + work.len())) / 3 {
                            return Err("value rendering limit exceeded");
                        }
                        out.push('{');
                        work.push(Part::End(id, '}'));
                        for (i, (k, v)) in v.iter().enumerate().rev() {
                            work.push(Part::Value(v.clone(), true));
                            work.push(Part::Text(format!("{k:?}: ")));
                            if i > 0 {
                                work.push(Part::Text(", ".into()));
                            }
                        }
                    }
                },
            }
            if out.len() > MAX_RENDER_BYTES {
                return Err("value rendering limit exceeded");
            }
        }
        Ok(out)
    }
}
impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        self.equals(other).unwrap_or(false)
    }
}
impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.render() {
            Ok(s) => f.write_str(&s),
            Err(_) => f.write_str("<value rendering limit exceeded>"),
        }
    }
}
