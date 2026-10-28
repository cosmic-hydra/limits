//! Non-recursive stack VM. Calls use explicit frames; captured lexical scope
//! slots are heap upvalues, so closures survive scope exit and share writes.
use crate::bytecode::{Binding, CompiledProgram, Function, Op};
use crate::error::{Kind, LimError, Span};
use crate::interp::{self, Interpreter, Limits, Value, BUILTINS};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Write;
use std::rc::Rc;

type Upvalues = Rc<Scope>;
struct Scope {
    slots: RefCell<Vec<Option<Value>>>,
    parent: Option<Upvalues>,
    frame_base: Option<usize>,
}
impl Scope {
    fn new(size: usize, parent: Option<Upvalues>, frame_base: Option<usize>) -> Upvalues {
        Rc::new(Self {
            slots: RefCell::new(vec![None; size]),
            parent,
            frame_base,
        })
    }
    fn read(&self, b: &Binding, skip: usize, s: Span) -> Result<Value, LimError> {
        let mut scope = self;
        let mut depth = 0;
        for &(d, i) in &b.slots {
            while depth < d - skip {
                scope = scope.parent.as_deref().unwrap();
                depth += 1;
            }
            if let Some(v) = &scope.slots.borrow()[i] {
                return Ok(v.clone());
            }
        }
        Err(error(format!("undefined variable '{}'", b.name), s))
    }
    fn find<'a>(&'a self, b: &Binding, skip: usize) -> Option<(&'a Scope, usize)> {
        let mut scope = self;
        let mut depth = 0;
        for &(d, i) in &b.slots {
            while depth < d - skip {
                scope = scope.parent.as_deref()?;
                depth += 1;
            }
            if scope.slots.borrow()[i].is_some() {
                return Some((scope, i));
            }
        }
        None
    }
}
pub struct Closure {
    pub(crate) function: Rc<Function>,
    upvalues: Upvalues,
}
struct Frame {
    function: Rc<Function>,
    ip: usize,
    scope: Upvalues,
    base: usize,
    locals_base: usize,
    inline_scope: bool,
    iterators: Vec<std::vec::IntoIter<Value>>,
}
fn read(frame: &Frame, stack: &[Value], b: &Binding, s: Span) -> Result<Value, LimError> {
    if let Some(i) = b.frame_slot {
        return Ok(stack[frame.locals_base + i].clone());
    }
    frame.scope.read(b, usize::from(frame.inline_scope), s)
}
pub struct Vm<'a> {
    runtime: Interpreter<'a>,
    limits: Limits,
}
fn error(msg: impl Into<String>, s: Span) -> LimError {
    LimError::new(Kind::Runtime, msg, s)
}
impl<'a> Vm<'a> {
    pub fn new(out: &'a mut dyn Write) -> Self {
        Self::with_limits(out, Limits::default())
    }
    pub fn with_limits(out: &'a mut dyn Write, limits: Limits) -> Self {
        Self {
            runtime: Interpreter::with_limits(out, limits),
            limits,
        }
    }
    pub fn run(&mut self, program: CompiledProgram) -> Result<(), LimError> {
        self.runtime.reset_steps();
        let function = program.0;
        let mut heap = Heap::default();
        let scope = heap.scope(function.locals, None, None);
        for (i, b) in BUILTINS.iter().enumerate() {
            scope.slots.borrow_mut()[i] = Some(Value::Builtin(b));
        }
        let mut frames = vec![Frame {
            function,
            ip: 0,
            scope,
            base: 0,
            locals_base: 0,
            inline_scope: false,
            iterators: vec![],
        }];
        let mut stack: Vec<Value> = Vec::new();
        while !frames.is_empty() {
            if heap.needs_collection() {
                heap.collect(&frames, &stack);
            }
            let frame = frames.last_mut().unwrap();
            let function = &frame.function;
            let s = function.chunk.spans[frame.ip];
            let op = &function.chunk.code[frame.ip];
            frame.ip += 1;
            match op {
                Op::Tick => self.runtime.tick(s)?,
                Op::Constant(i) => stack.push(function.chunk.constants[*i].clone()),
                Op::Get(b) => stack.push(read(frame, &stack, b, s)?),
                Op::BinaryConstant(b, var_span, op, i) => {
                    let a = read(frame, &stack, b, *var_span)?;
                    stack.push(interp::binary(
                        *op,
                        a,
                        function.chunk.constants[*i].clone(),
                        s,
                    )?);
                }
                Op::BranchBinaryConstant(b, var_span, op, i, target) => {
                    let a = read(frame, &stack, b, *var_span)?;
                    if !interp::binary(*op, a, function.chunk.constants[*i].clone(), s)?.truthy() {
                        frame.ip = *target;
                    }
                }
                Op::Set(b) => {
                    let v = stack.pop().unwrap();
                    if frame.inline_scope {
                        if let Some(i) = b.frame_slot {
                            stack[frame.locals_base + i] = v;
                            continue;
                        }
                    }
                    let (scope, i) = frame
                        .scope
                        .find(b, usize::from(frame.inline_scope))
                        .ok_or_else(|| {
                            error(
                                format!(
                                    "assignment to undeclared variable '{}' (use 'let')",
                                    b.name
                                ),
                                s,
                            )
                        })?;
                    if let Some(base) = scope.frame_base {
                        stack[base + i] = v.clone();
                    }
                    scope.slots.borrow_mut()[i] = Some(v);
                }
                Op::Define(i) => {
                    let v = stack.pop().unwrap();
                    if let Some(base) = frame.scope.frame_base {
                        stack[base + i] = v.clone();
                    }
                    frame.scope.slots.borrow_mut()[*i] = Some(v);
                }
                Op::Pop => {
                    stack.pop().unwrap();
                }
                Op::List(n) => {
                    let vs = stack.split_off(stack.len() - n);
                    let value = Value::List(Rc::new(RefCell::new(vs)));
                    heap.track(&value);
                    stack.push(value);
                }
                Op::Map => {
                    let value = Value::Map(Rc::new(RefCell::new(BTreeMap::new())));
                    heap.track(&value);
                    stack.push(value);
                }
                Op::MapKey => {
                    if !matches!(stack.last(), Some(Value::Str(_))) {
                        return Err(error(
                            format!(
                                "map keys must be strings, got {}",
                                stack.last().unwrap().type_name()
                            ),
                            s,
                        ));
                    }
                }
                Op::MapInsert => {
                    let v = stack.pop().unwrap();
                    let Value::Str(k) = stack.pop().unwrap() else {
                        unreachable!()
                    };
                    let Value::Map(m) = stack.last().unwrap() else {
                        unreachable!()
                    };
                    m.borrow_mut().insert(k.to_string(), v);
                }
                Op::Unary(op) => {
                    let v = stack.pop().unwrap();
                    stack.push(interp::unary(*op, v, s)?);
                }
                Op::Binary(op) => {
                    let b = stack.pop().unwrap();
                    let a = stack.pop().unwrap();
                    stack.push(interp::binary(*op, a, b, s)?);
                }
                Op::Range => {
                    let b = stack.pop().unwrap();
                    let a = stack.pop().unwrap();
                    stack.push(interp::range(a, b, self.limits.max_steps, s)?);
                }
                Op::Index => {
                    let i = stack.pop().unwrap();
                    let o = stack.pop().unwrap();
                    stack.push(interp::index(o, i, s)?);
                }
                Op::SetIndex => {
                    let i = stack.pop().unwrap();
                    let o = stack.pop().unwrap();
                    let v = stack.pop().unwrap();
                    self.runtime.set_index(&o, &i, v, s)?;
                }
                Op::Jump(i) => frame.ip = *i,
                Op::JumpFalse(i) => {
                    if !stack.pop().unwrap().truthy() {
                        frame.ip = *i;
                    }
                }
                Op::ShortCircuit(truth, i) => {
                    if stack.last().unwrap().truthy() == *truth {
                        frame.ip = *i;
                    } else {
                        stack.pop();
                    }
                }
                Op::Enter(n) => frame.scope = heap.scope(*n, Some(frame.scope.clone()), None),
                Op::Exit => frame.scope = frame.scope.parent.as_ref().unwrap().clone(),
                Op::Closure(f) => stack.push(Value::VmFunc(Rc::new(Closure {
                    function: f.clone(),
                    upvalues: frame.scope.clone(),
                }))),
                Op::Call(_) | Op::CallBinaryConstant(_) => {
                    let n = match op {
                        Op::Call(n) => *n,
                        Op::CallBinaryConstant(site) => {
                            let f = read(frame, &stack, &site.callee, site.callee_span)?;
                            let a = read(frame, &stack, &site.argument, site.argument_span)?;
                            let arg = interp::binary(
                                site.op,
                                a,
                                Value::Int(site.operand),
                                site.expression_span,
                            )?;
                            stack.push(f);
                            stack.push(arg);
                            1
                        }
                        _ => unreachable!(),
                    };
                    self.runtime.tick(s)?;
                    let base = stack.len() - n - 1;
                    let f = std::mem::replace(&mut stack[base], Value::Nil);
                    match f {
                        Value::Builtin(name) => {
                            let args = stack.split_off(base + 1);
                            stack.pop();
                            stack.push(self.runtime.builtin(name, args, s)?);
                        }
                        Value::VmFunc(f) => {
                            if n != f.function.arity {
                                return Err(error(
                                    format!(
                                        "{}() takes {} argument(s), got {}",
                                        f.function.name, f.function.arity, n
                                    ),
                                    s,
                                ));
                            }
                            if frames.len() > self.limits.max_depth {
                                return Err(error(
                                    format!(
                                        "call depth limit exceeded ({})",
                                        self.limits.max_depth
                                    ),
                                    s,
                                ));
                            }
                            if f.function.frame_only {
                                frames.push(Frame {
                                    function: f.function.clone(),
                                    ip: 0,
                                    scope: f.upvalues.clone(),
                                    base,
                                    locals_base: base + 1,
                                    inline_scope: true,
                                    iterators: vec![],
                                });
                                continue;
                            }
                            let scope = heap.scope(
                                f.function.locals,
                                Some(f.upvalues.clone()),
                                f.function.stack_parameters.then_some(base),
                            );
                            {
                                let mut slots = scope.slots.borrow_mut();
                                for (&slot, arg) in
                                    f.function.params.iter().zip(stack.drain(base + 1..))
                                {
                                    slots[slot] = Some(arg);
                                }
                                stack.pop();
                                if f.function.stack_parameters {
                                    stack.extend(
                                        slots.iter().map(|v| v.clone().unwrap_or(Value::Nil)),
                                    );
                                }
                            }
                            frames.push(Frame {
                                function: f.function.clone(),
                                ip: 0,
                                scope,
                                base,
                                locals_base: base,
                                inline_scope: false,
                                iterators: vec![],
                            });
                        }
                        other => {
                            return Err(error(format!("{} is not callable", other.type_name()), s))
                        }
                    }
                }
                Op::Return => {
                    let v = stack.pop().unwrap();
                    let f = frames.pop().unwrap();
                    stack.truncate(f.base);
                    if !frames.is_empty() {
                        stack.push(v);
                    }
                }
                Op::Iterate => {
                    let items = match stack.pop().unwrap() {
                        Value::List(l) => l.borrow().clone(),
                        Value::Str(st) => st
                            .chars()
                            .map(|c| Value::Str(c.to_string().into()))
                            .collect(),
                        Value::Map(m) => m
                            .borrow()
                            .keys()
                            .map(|k| Value::Str(k.as_str().into()))
                            .collect(),
                        other => {
                            return Err(error(
                                format!("cannot iterate over {}", other.type_name()),
                                s,
                            ))
                        }
                    };
                    frame.iterators.push(items.into_iter());
                }
                Op::Next(i) => {
                    if let Some(v) = frame.iterators.last_mut().unwrap().next() {
                        stack.push(v);
                    } else {
                        frame.iterators.pop();
                        frame.ip = *i;
                    }
                }
                Op::EndIter => {
                    frame.iterators.pop().unwrap();
                }
                Op::Error(msg) => return Err(error(*msg, s)),
            }
            if let Some(value) = stack.last() {
                heap.track(value);
            }
        }
        Ok(())
    }
}

// The registry owns every VM allocation until collection. Clearing unreachable
// edges before releasing nodes handles reference cycles and deep graphs without
// running a recursive chain of Rc destructors. Scope parents always precede
// their children, so scopes are released in reverse allocation order.
type List = Rc<RefCell<Vec<Value>>>;
type Map = Rc<RefCell<BTreeMap<String, Value>>>;
#[derive(Default)]
struct Heap {
    scopes: Vec<Upvalues>,
    lists: std::collections::HashMap<usize, List>,
    maps: std::collections::HashMap<usize, Map>,
    next_collection: usize,
}
impl Heap {
    fn scope(&mut self, size: usize, parent: Option<Upvalues>, base: Option<usize>) -> Upvalues {
        let scope = Scope::new(size, parent, base);
        self.scopes.push(scope.clone());
        scope
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
    fn collect(&mut self, frames: &[Frame], stack: &[Value]) {
        use std::collections::HashSet;
        enum Root {
            Scope(Upvalues),
            Value(Value),
        }
        let mut work: Vec<_> = stack.iter().cloned().map(Root::Value).collect();
        for frame in frames {
            work.push(Root::Scope(frame.scope.clone()));
            for iterator in &frame.iterators {
                work.extend(iterator.as_slice().iter().cloned().map(Root::Value));
            }
        }
        let mut scopes = HashSet::new();
        let mut lists = HashSet::new();
        let mut maps = HashSet::new();
        while let Some(root) = work.pop() {
            match root {
                Root::Scope(scope) => {
                    if !scopes.insert(Rc::as_ptr(&scope) as usize) {
                        continue;
                    }
                    if let Some(parent) = &scope.parent {
                        work.push(Root::Scope(parent.clone()));
                    }
                    work.extend(
                        scope
                            .slots
                            .borrow()
                            .iter()
                            .flatten()
                            .cloned()
                            .map(Root::Value),
                    );
                }
                Root::Value(Value::VmFunc(f)) => work.push(Root::Scope(f.upvalues.clone())),
                Root::Value(Value::List(v)) => {
                    if lists.insert(Rc::as_ptr(&v) as usize) {
                        work.extend(v.borrow().iter().cloned().map(Root::Value));
                    }
                }
                Root::Value(Value::Map(v)) if maps.insert(Rc::as_ptr(&v) as usize) => {
                    work.extend(v.borrow().values().cloned().map(Root::Value));
                }
                _ => {}
            }
        }
        // Keep strong guards until every unreachable edge has been removed.
        for scope in &self.scopes {
            if !scopes.contains(&(Rc::as_ptr(scope) as usize)) {
                scope.slots.borrow_mut().clear();
            }
        }
        for (&id, list) in &self.lists {
            if !lists.contains(&id) {
                list.borrow_mut().clear();
            }
        }
        for (&id, map) in &self.maps {
            if !maps.contains(&id) {
                map.borrow_mut().clear();
            }
        }
        let mut dead = Vec::new();
        self.scopes.retain(|scope| {
            if scopes.contains(&(Rc::as_ptr(scope) as usize)) {
                true
            } else {
                dead.push(scope.clone());
                false
            }
        });
        while dead.pop().is_some() {}
        self.lists.retain(|id, _| lists.contains(id));
        self.maps.retain(|id, _| maps.contains(id));
        self.next_collection = self.len().saturating_mul(2).saturating_add(1024);
    }
}
impl Drop for Heap {
    fn drop(&mut self) {
        for scope in &self.scopes {
            scope.slots.borrow_mut().clear();
        }
        for list in self.lists.values() {
            list.borrow_mut().clear();
        }
        for map in self.maps.values() {
            map.borrow_mut().clear();
        }
        while self.scopes.pop().is_some() {}
    }
}

#[cfg(test)]
mod heap_tests {
    use super::*;
    #[test]
    fn unreachable_cycles_are_collected() {
        let mut heap = Heap::default();
        let v = Rc::new(RefCell::new(Vec::new()));
        v.borrow_mut().push(Value::List(v.clone()));
        let weak = Rc::downgrade(&v);
        heap.track(&Value::List(v));
        heap.collect(&[], &[]);
        assert!(weak.upgrade().is_none());
        assert_eq!(heap.len(), 0);
    }
    #[test]
    fn reachable_cycles_survive_until_heap_drop() {
        let weak;
        {
            let mut heap = Heap::default();
            let v = Rc::new(RefCell::new(Vec::new()));
            v.borrow_mut().push(Value::List(v.clone()));
            weak = Rc::downgrade(&v);
            let roots = vec![Value::List(v)];
            heap.track(&roots[0]);
            heap.collect(&[], &roots);
            assert_eq!(heap.len(), 1);
            assert_eq!(weak.upgrade().unwrap().borrow().len(), 1);
        }
        assert!(weak.upgrade().is_none());
    }
    #[test]
    fn deep_graph_cleanup_does_not_recurse() {
        let mut heap = Heap::default();
        let mut v = Value::Nil;
        for _ in 0..100_000 {
            v = Value::List(Rc::new(RefCell::new(vec![v])));
            heap.track(&v);
        }
        drop(v);
        heap.collect(&[], &[]);
        assert_eq!(heap.len(), 0);
    }
}

#[cfg(test)]
mod cycle_regressions {
    use super::*;
    #[test]
    fn short_lived_recursive_scopes_have_bounded_retention() {
        let program = crate::codegen::compile(&Vec::new()).unwrap();
        let root = Scope::new(0, None, None);
        let frames = vec![Frame {
            function: program.0.clone(),
            ip: 0,
            scope: root.clone(),
            base: 0,
            locals_base: 0,
            inline_scope: false,
            iterators: Vec::new(),
        }];
        let mut heap = Heap::default();
        for _ in 0..100_000 {
            let scope = heap.scope(1, Some(root.clone()), None);
            scope.slots.borrow_mut()[0] = Some(Value::VmFunc(Rc::new(Closure {
                function: program.0.clone(),
                upvalues: scope.clone(),
            })));
            if heap.needs_collection() {
                heap.collect(&frames, &[]);
            }
            assert!(heap.len() <= 1024);
        }
        heap.collect(&frames, &[]);
        assert_eq!(heap.len(), 0);
    }
    #[test]
    fn error_unwind_releases_recursive_closures() {
        let mut weak = None;
        let mut fail = || -> Result<(), LimError> {
            let mut heap = Heap::default();
            let scope = heap.scope(1, None, None);
            weak = Some(Rc::downgrade(&scope));
            let program = crate::codegen::compile(&Vec::new()).unwrap();
            scope.slots.borrow_mut()[0] = Some(Value::VmFunc(Rc::new(Closure {
                function: program.0,
                upvalues: scope.clone(),
            })));
            Err(error("test error", Span { line: 1, col: 1 }))
        };
        let result = fail();
        assert!(result.is_err());
        assert!(weak.unwrap().upgrade().is_none());
    }
}
