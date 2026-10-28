//! AST to bytecode with lexical slots and captured scope upvalues.
use crate::ast::*;
use crate::bytecode::*;
use crate::error::Span;
use crate::interp::{stmt_span, Value, BUILTINS};
use std::rc::Rc;
struct Loop {
    start: usize,
    depth: usize,
    breaks: Vec<usize>,
    iterator: bool,
}
struct Compiler {
    chunk: Chunk,
    scopes: Vec<Vec<String>>,
    loops: Vec<Loop>,
    parameter_scope: Option<(usize, Vec<usize>)>,
}
fn layout(body: &[Stmt], initial: &[String]) -> Vec<String> {
    let mut names = initial.to_vec();
    for s in body {
        if let Stmt::Let(n, ..) | Stmt::Fn(n, ..) = s {
            if !names.contains(n) {
                names.push(n.clone());
            }
        }
    }
    names
}
pub fn compile(program: &Program) -> Result<CompiledProgram, crate::error::LimError> {
    crate::ast::validate(program)?;
    let builtins: Vec<_> = BUILTINS.iter().map(|s| s.to_string()).collect();
    let names = layout(program, &builtins);
    let locals = names.len();
    let mut c = Compiler {
        chunk: Chunk::default(),
        scopes: vec![names],
        loops: vec![],
        parameter_scope: None,
    };
    c.block(program);
    let s = Span { line: 1, col: 1 };
    c.constant(Value::Nil, s);
    c.emit(Op::Return, s);
    Ok(CompiledProgram(Rc::new(Function {
        name: "<script>".into(),
        arity: 0,
        params: vec![],
        locals,
        stack_parameters: false,
        frame_only: false,
        chunk: c.chunk,
    })))
}
impl Compiler {
    fn emit(&mut self, op: Op, s: Span) -> usize {
        let i = self.chunk.code.len();
        self.chunk.code.push(op);
        self.chunk.spans.push(s);
        i
    }
    fn constant(&mut self, v: Value, s: Span) {
        let i = self.chunk.constants.len();
        self.chunk.constants.push(v);
        self.emit(Op::Constant(i), s);
    }
    fn patch(&mut self, i: usize, target: usize) {
        match &mut self.chunk.code[i] {
            Op::Jump(t)
            | Op::JumpFalse(t)
            | Op::ShortCircuit(_, t)
            | Op::Next(t)
            | Op::BranchBinaryConstant(_, _, _, _, t) => *t = target,
            _ => unreachable!(),
        }
    }
    fn binding(&self, name: &str) -> Binding {
        let slots: Vec<_> = self
            .scopes
            .iter()
            .rev()
            .enumerate()
            .filter_map(|(d, ns)| ns.iter().position(|n| n == name).map(|i| (d, i)))
            .collect();
        let frame_slot = self.parameter_scope.as_ref().and_then(|(scope, params)| {
            slots.first().and_then(|&(d, i)| {
                (self.scopes.len() - 1 - d == *scope && params.contains(&i)).then_some(i)
            })
        });
        Binding {
            name: name.into(),
            slots,
            frame_slot,
        }
    }
    fn slot(&self, name: &str) -> usize {
        self.scopes
            .last()
            .unwrap()
            .iter()
            .position(|n| n == name)
            .unwrap()
    }
    fn block(&mut self, body: &[Stmt]) {
        for s in body {
            self.stmt(s);
        }
    }
    fn enter(&mut self, body: &[Stmt], initial: &[String], s: Span) {
        let ns = layout(body, initial);
        self.emit(Op::Enter(ns.len()), s);
        self.scopes.push(ns);
    }
    fn exit(&mut self, s: Span) {
        self.emit(Op::Exit, s);
        self.scopes.pop();
    }
    fn scoped(&mut self, body: &[Stmt], s: Span) {
        if layout(body, &[]).is_empty() {
            self.block(body);
        } else {
            self.enter(body, &[], s);
            self.block(body);
            self.exit(s);
        }
    }
    fn condition(&mut self, e: &Expr, s: Span) -> usize {
        self.expr(e, s);
        if matches!(self.chunk.code.last(), Some(Op::BinaryConstant(..))) {
            let Op::BinaryConstant(b, sp, op, i) = self.chunk.code.pop().unwrap() else {
                unreachable!()
            };
            let expression_span = self.chunk.spans.pop().unwrap();
            self.emit(Op::BranchBinaryConstant(b, sp, op, i, 0), expression_span)
        } else {
            self.emit(Op::JumpFalse(0), s)
        }
    }
    fn stmt(&mut self, stmt: &Stmt) {
        let s = stmt_span(stmt);
        self.emit(Op::Tick, s);
        match stmt {
            Stmt::Let(n, e, _) => {
                self.expr(e, s);
                self.emit(Op::Define(self.slot(n)), s);
            }
            Stmt::Assign(t, e, _) => {
                self.expr(e, s);
                match t {
                    Expr::Var(n, sp) => {
                        self.emit(Op::Set(self.binding(n)), *sp);
                    }
                    Expr::Index(o, i, sp) => {
                        self.expr(o, s);
                        self.expr(i, s);
                        self.emit(Op::SetIndex, *sp);
                    }
                    _ => unreachable!(),
                }
            }
            Stmt::Expr(e, _) => {
                self.expr(e, s);
                self.emit(Op::Pop, s);
            }
            Stmt::If(e, t, f, _) => {
                let j = self.condition(e, s);
                self.scoped(t, s);
                let end = self.emit(Op::Jump(0), s);
                self.patch(j, self.chunk.code.len());
                if let Some(f) = f {
                    self.scoped(f, s);
                }
                self.patch(end, self.chunk.code.len());
            }
            Stmt::While(e, b, _) => {
                let start = self.chunk.code.len();
                let end = self.condition(e, s);
                self.emit(Op::Tick, s);
                self.loops.push(Loop {
                    start,
                    depth: self.scopes.len(),
                    breaks: vec![],
                    iterator: false,
                });
                self.scoped(b, s);
                self.emit(Op::Jump(start), s);
                self.finish_loop();
                self.patch(end, self.chunk.code.len());
            }
            Stmt::For(n, e, b, _) => {
                self.expr(e, s);
                self.emit(Op::Iterate, s);
                let start = self.chunk.code.len();
                let end = self.emit(Op::Next(0), s);
                self.emit(Op::Tick, s);
                self.loops.push(Loop {
                    start,
                    depth: self.scopes.len(),
                    breaks: vec![],
                    iterator: true,
                });
                self.enter(b, std::slice::from_ref(n), s);
                self.emit(Op::Define(self.slot(n)), s);
                self.block(b);
                self.exit(s);
                self.emit(Op::Jump(start), s);
                self.finish_loop();
                self.patch(end, self.chunk.code.len());
            }
            Stmt::Fn(n, params, b, _) => {
                let names = layout(b, params);
                let locals = names.len();
                let slots: Vec<usize> = params
                    .iter()
                    .map(|p| names.iter().position(|n| n == p).unwrap())
                    .collect();
                let mut scopes = self.scopes.clone();
                scopes.push(names);
                let stack_parameters = !has_closures(b);
                let parameter_scope = stack_parameters.then(|| (scopes.len() - 1, slots.clone()));
                let mut c = Compiler {
                    chunk: Chunk::default(),
                    scopes,
                    loops: vec![],
                    parameter_scope,
                };
                c.block(b);
                c.constant(Value::Nil, s);
                c.emit(Op::Return, s);
                self.emit(
                    Op::Closure(Rc::new(Function {
                        name: n.clone(),
                        arity: params.len(),
                        params: slots,
                        locals,
                        stack_parameters,
                        frame_only: frame_only(b) && slots_are_unique(params),
                        chunk: c.chunk,
                    })),
                    s,
                );
                self.emit(Op::Define(self.slot(n)), s);
            }
            Stmt::Return(e, _) => {
                if let Some(e) = e {
                    self.expr(e, s);
                } else {
                    self.constant(Value::Nil, s);
                }
                self.emit(Op::Return, s);
            }
            Stmt::Break(_) | Stmt::Continue(_) => {
                let br = matches!(stmt, Stmt::Break(_));
                if let Some(l) = self.loops.last() {
                    let (depth, start, iterator) = (l.depth, l.start, l.iterator);
                    for _ in depth..self.scopes.len() {
                        self.emit(Op::Exit, s);
                    }
                    if br {
                        if iterator {
                            self.emit(Op::EndIter, s);
                        }
                        let j = self.emit(Op::Jump(0), s);
                        self.loops.last_mut().unwrap().breaks.push(j);
                    } else {
                        self.emit(Op::Jump(start), s);
                    }
                } else {
                    self.emit(
                        Op::Error(if br {
                            "'break' outside of a loop"
                        } else {
                            "'continue' outside of a loop"
                        }),
                        s,
                    );
                }
            }
        }
    }
    fn finish_loop(&mut self) {
        let l = self.loops.pop().unwrap();
        for i in l.breaks {
            self.patch(i, self.chunk.code.len());
        }
    }
    fn expr(&mut self, e: &Expr, parent: Span) {
        match e {
            Expr::Int(v) => self.constant(Value::Int(*v), parent),
            Expr::Float(v) => self.constant(Value::Float(*v), parent),
            Expr::Str(v) => self.constant(Value::Str(v.as_str().into()), parent),
            Expr::Bool(v) => self.constant(Value::Bool(*v), parent),
            Expr::Nil => self.constant(Value::Nil, parent),
            Expr::Var(n, s) => {
                self.emit(Op::Get(self.binding(n)), *s);
            }
            Expr::List(es) => {
                for e in es {
                    self.expr(e, parent);
                }
                self.emit(Op::List(es.len()), parent);
            }
            Expr::Map(es, s) => {
                self.emit(Op::Map, *s);
                for (k, v) in es {
                    self.expr(k, *s);
                    self.emit(Op::MapKey, *s);
                    self.expr(v, *s);
                    self.emit(Op::MapInsert, *s);
                }
            }
            Expr::Unary(op, e, s) => {
                self.expr(e, *s);
                self.emit(Op::Unary(*op), *s);
            }
            Expr::Binary(op, a, b, s) => {
                // Fuse a pure variable/literal pair without changing evaluation
                // order, step charges, or the two possible error locations.
                if !matches!(op, BinOp::And | BinOp::Or) {
                    if let (Expr::Var(name, var_span), Expr::Int(n)) = (&**a, &**b) {
                        let i = self.chunk.constants.len();
                        self.chunk.constants.push(Value::Int(*n));
                        self.emit(
                            Op::BinaryConstant(self.binding(name), *var_span, *op, i),
                            *s,
                        );
                        return;
                    }
                }
                self.expr(a, *s);
                if matches!(op, BinOp::And | BinOp::Or) {
                    let j = self.emit(Op::ShortCircuit(*op == BinOp::Or, 0), *s);
                    self.expr(b, *s);
                    self.patch(j, self.chunk.code.len());
                } else {
                    self.expr(b, *s);
                    self.emit(Op::Binary(*op), *s);
                }
            }
            Expr::Range(a, b, s) => {
                self.expr(a, *s);
                self.expr(b, *s);
                self.emit(Op::Range, *s);
            }
            Expr::Index(a, b, s) => {
                self.expr(a, *s);
                self.expr(b, *s);
                self.emit(Op::Index, *s);
            }
            Expr::Call(f, args, s) => {
                if let (Expr::Var(name, callee_span), [Expr::Binary(op, a, b, expression_span)]) =
                    (&**f, args.as_slice())
                {
                    if !matches!(op, BinOp::And | BinOp::Or) {
                        if let (Expr::Var(argument, argument_span), Expr::Int(operand)) =
                            (&**a, &**b)
                        {
                            self.emit(
                                Op::CallBinaryConstant(Rc::new(CallSite {
                                    callee: self.binding(name),
                                    callee_span: *callee_span,
                                    argument: self.binding(argument),
                                    argument_span: *argument_span,
                                    op: *op,
                                    operand: *operand,
                                    expression_span: *expression_span,
                                })),
                                *s,
                            );
                            return;
                        }
                    }
                }
                self.expr(f, *s);
                for a in args {
                    self.expr(a, *s);
                }
                self.emit(Op::Call(args.len()), *s);
            }
        }
    }
}

fn has_closures(body: &[Stmt]) -> bool {
    body.iter().any(|s| match s {
        Stmt::Fn(..) => true,
        Stmt::If(_, a, b, _) => has_closures(a) || b.as_ref().is_some_and(|b| has_closures(b)),
        Stmt::For(_, _, b, _) | Stmt::While(_, b, _) => has_closures(b),
        _ => false,
    })
}

// With no local declarations or captured locals, the call's lexical scope can
// live entirely in operand-stack parameter slots.
fn frame_only(body: &[Stmt]) -> bool {
    body.iter().all(|s| match s {
        Stmt::Let(..) | Stmt::Fn(..) | Stmt::For(..) => false,
        Stmt::If(_, a, b, _) => frame_only(a) && b.as_ref().is_none_or(|b| frame_only(b)),
        Stmt::While(_, b, _) => frame_only(b),
        _ => true,
    })
}
fn slots_are_unique(params: &[String]) -> bool {
    params
        .iter()
        .enumerate()
        .all(|(i, p)| !params[..i].contains(p))
}
