//! Stack bytecode with a constant pool and a source span for each instruction.
use crate::ast::{BinOp, UnOp};
use crate::error::Span;
use crate::interp::Value;
use std::rc::Rc;

/// Lexical slots, nearest first. Uninitialized declarations fall through to
/// outer bindings, preserving the tree walker's late declaration semantics.
#[derive(Clone)]
pub struct Binding {
    pub name: String,
    /// Initialized parameter in this frame; valid only when no child closure
    /// can mutate its scope. Other references use captured heap slots.
    pub frame_slot: Option<usize>,
    pub slots: Vec<(usize, usize)>,
}
#[derive(Clone)]
pub enum Op {
    Tick,
    Constant(usize),
    Get(Binding),
    Set(Binding),
    Define(usize),
    Pop,
    List(usize),
    Map,
    MapKey,
    MapInsert,
    Unary(UnOp),
    Binary(BinOp),
    BinaryConstant(Binding, Span, BinOp, usize),
    BranchBinaryConstant(Binding, Span, BinOp, usize, usize),
    Range,
    Index,
    SetIndex,
    Jump(usize),
    JumpFalse(usize),
    ShortCircuit(bool, usize),
    Enter(usize),
    Exit,
    Closure(Rc<Function>),
    Call(usize),
    CallBinaryConstant(Rc<CallSite>),
    Return,
    Iterate,
    Next(usize),
    EndIter,
    Error(&'static str),
}
#[derive(Default)]
pub struct Chunk {
    pub code: Vec<Op>,
    pub constants: Vec<Value>,
    pub spans: Vec<Span>,
}
pub struct Function {
    pub name: String,
    pub arity: usize,
    pub params: Vec<usize>,
    pub locals: usize,
    /// Cache uncaptured parameters in the operand stack.
    pub stack_parameters: bool,
    /// No declarations require a heap scope; parameters are the entire scope.
    pub frame_only: bool,
    pub chunk: Chunk,
}

/// A pure argument expression fused with a call. All intermediate spans are
/// retained, and callee and argument are still evaluated before charging a call.
pub struct CallSite {
    pub callee: Binding,
    pub callee_span: Span,
    pub argument: Binding,
    pub argument_span: Span,
    pub op: BinOp,
    pub operand: i64,
    pub expression_span: Span,
}
