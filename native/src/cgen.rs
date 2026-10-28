//! C11 code generation from a checked program.
//!
//! Memory management: str, list, struct and enum values are reference
//! counted (see runtime.h) and have value semantics. Every expression the
//! generator emits yields either an owned reference (+1, the consumer must
//! store or drop it), a borrowed one (valid while its owner lives), or an
//! immortal/scalar value. Owned results used only by borrowing consumers go
//! into per-statement temporaries (`tN`, declared null and dropped at the end
//! of the statement). Locals own their value; parameters are borrowed; an
//! `inout` parameter receives a pointer to the caller's variable, works on a
//! local copy of it and stores it back at every return. A local is
//! dropped right after the statement holding its last use, and that use
//! moves the value instead of copying the reference when it can. Writes go
//! through "places" (`xs[i].f = v`, `push(xs, v)`) that make every value on
//! the path unique first (copy-on-write).

use crate::ast::*;
use crate::check::{place_root, variants_of, Variants};
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

pub struct Opts {
    pub checked: bool,
    pub max_steps: u64,
    pub max_depth: u64,
    /// Heap budget in bytes (all modes): live bytes.
    pub max_mem: u64,
    /// Upper bound on the number of C translation units (for parallel cc).
    pub parts: usize,
    /// Runtime errors as JSON lines (`--json`).
    pub json: bool,
    /// Build the inline tests (`limits test`) instead of `main`.
    pub tests: bool,
    /// `--leak-check`: report heap blocks still live when the program ends.
    pub leak_check: bool,
    /// The C compiler is TinyCC: emit the overflow-builtin shim.
    pub tcc: bool,
    /// `--allow` grants: canonical path prefixes for fs.read / fs.write
    /// (empty = current directory) and env var names (empty = all).
    pub allow_read: Vec<String>,
    pub allow_write: Vec<String>,
    pub allow_env: Vec<String>,
}

/// Generated code below this many bytes always stays in one C file.
const SPLIT_BYTES: usize = 64 * 1024;

/// Ownership of an emitted expression's value.
#[derive(Clone, Copy, PartialEq, Debug)]
enum O {
    /// A +1 reference the consumer must store or drop.
    Own,
    /// Borrowed from a variable, a temporary or a container.
    Bor,
    /// A scalar, or an immortal static (literal, unit variant).
    Imm,
}

#[derive(Default, Clone)]
struct Scope {
    /// Source name -> C name.
    names: HashMap<String, String>,
    /// Source name -> type.
    types: HashMap<String, Ty>,
    /// Heap values this scope owns, in order: source name ("" for hidden
    /// ones such as a loop's copy of its list), C name, type.
    owned: Vec<(String, String, Ty)>,
}

struct Gen<'a> {
    out: String,
    strs: Vec<String>,
    struct_idx: HashMap<&'a str, usize>,
    structs: &'a [Struct],
    sfields: &'a crate::check::StructMap,
    scopes: Vec<Scope>,
    tmp: usize,
    ind: usize,
    ret_unit: bool,
    checked: bool,
    /// Added to line numbers: the current function's file index << 20.
    fofs: u32,
    /// `ensures` clauses of the current function, checked at every return.
    ensures: &'a [Expr],
    fname: &'a str,
    /// Return type of the current function (for `?`).
    ret_ty: Ty,
    /// User enums by qualified name.
    enums: &'a HashMap<String, Variants>,
    /// Sum-type instances used by the program (user enums, Option[T],
    /// Result[T, E]), by C type name; the index is their print descriptor.
    einst: Vec<Ty>,
    eidx: HashMap<String, usize>,
    /// Open temporary scopes: id and temporaries (C name, type).
    temps: Vec<(usize, Vec<(String, Ty)>)>,
    /// Drop code of every closed temporary scope, by id (substituted for
    /// the placeholders that early exits emit).
    temp_drops: HashMap<usize, String>,
    next_tid: usize,
    /// Open loops: (scope depth, temp-scope depth) at their body.
    loops: Vec<(usize, usize)>,
    /// C names of locals whose current use may move the value out.
    movable: HashSet<String>,
    /// The `let` being emitted may borrow its value (see `let_borrows`).
    let_borrow: bool,
    /// Unique-path cache: place paths (`bs`, `bs[i]`, keyed by C names)
    /// already made unique in the current run of straight-line statements
    /// that cannot share a value (see `cache_safe`), with the C local
    /// holding each. Later writes and reads through them skip the
    /// copy-on-write check (and bounds check).
    upc: Vec<(String, String)>,
    /// C names of variables made unique before the loops being emitted,
    /// which those loops cannot share (see `loop_unique`).
    unique_roots: Vec<String>,
    /// Emitting the body of the program's `main` without `--leak-check`:
    /// what its top-level block still owns at the end is left to the OS
    /// (the process exits right after), not freed block by block.
    main_exit: bool,
    /// Stores writing the current fn's inout parameters back to the
    /// caller (`*p_x = v_x; `), run at every return.
    writeback: String,
    /// Heap types needing drop/free (and for structs clone) functions.
    rc_types: Vec<Ty>,
    rc_seen: HashSet<String>,
}

/// Is a value of this type a reference-counted heap value?
fn heap(t: &Ty) -> bool {
    matches!(
        t,
        Ty::Str | Ty::List(_) | Ty::Struct(_) | Ty::Enum(_) | Ty::Option(_) | Ty::Result(..)
    )
}

/// Prefix-free C name fragment of a type (for monomorphized sum types).
fn tname(t: &Ty) -> String {
    match t {
        Ty::Int => "i".into(),
        Ty::Float => "f".into(),
        Ty::Bool => "b".into(),
        Ty::Str => "s".into(),
        Ty::List(x) => format!("L{}", tname(x)),
        Ty::Struct(n) | Ty::Enum(n) => {
            let m = mangle(n);
            let k = if matches!(t, Ty::Struct(_)) { 'S' } else { 'E' };
            format!("{}{}{}", k, m.len(), m)
        }
        Ty::Option(x) => format!("O{}", tname(x)),
        Ty::Result(a, b) => format!("R{}{}", tname(a), tname(b)),
        Ty::Cap(c) => format!("C{}", c.name()),
        Ty::Unit | Ty::Err => unreachable!("not a value type"),
    }
}

fn cty(t: &Ty) -> String {
    match t {
        // Capabilities are handles: an Fs is its root's index, the others 0.
        Ty::Int | Ty::Cap(_) => "int64_t".into(),
        Ty::Float => "double".into(),
        Ty::Bool => "bool".into(),
        Ty::Str => "lm_str".into(),
        Ty::Unit => "void".into(),
        Ty::List(_) => "lm_list *".into(),
        Ty::Struct(n) => format!("S_{} *", mangle(n)),
        Ty::Enum(_) | Ty::Option(_) | Ty::Result(..) => format!("E_{} *", tname(t)),
        Ty::Err => unreachable!("type error reached codegen"),
    }
}

fn to_val(code: &str, t: &Ty) -> String {
    match t {
        Ty::Int | Ty::Bool => format!("lm_vi({})", code),
        Ty::Float => format!("lm_vf({})", code),
        _ => format!("lm_vp({})", code),
    }
}

/// Accessor suffix and C storage type of a list element (see LM_LIST).
fn elem(list: &Ty) -> (&'static str, String) {
    match list {
        Ty::List(t) => match **t {
            Ty::Bool => ("b", "bool".into()),
            Ty::Int => ("i", "int64_t".into()),
            Ty::Float => ("f", "double".into()),
            _ => ("p", "void *".into()),
        },
        _ => unreachable!(),
    }
}

fn elem_ty(list: &Ty) -> Ty {
    match list {
        Ty::List(t) => (**t).clone(),
        _ => unreachable!(),
    }
}

/// Cast a value read from a list back to its C type (pointer elements are `void *`).
fn from_slot(code: String, t: &Ty) -> String {
    match t {
        Ty::Bool | Ty::Int | Ty::Float => code,
        _ => format!("(({}){})", cty(t), code),
    }
}

fn c_escape(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        match b {
            b'"' => o.push_str("\\\""),
            b'\\' => o.push_str("\\\\"),
            32..=126 if b != b'?' => o.push(b as char),
            _ => {
                let _ = write!(o, "\\{:03o}", b);
            }
        }
    }
    o
}

// ---- syntax-tree walks used for ownership decisions ----

/// Call `f` on `e` and every expression nested in it (including the
/// statements of match arms). `inner` is true below a comprehension or a
/// match, where an expression may run several times or not at all.
fn walk(e: &Expr, inner: bool, f: &mut dyn FnMut(&Expr, bool)) {
    f(e, inner);
    match &e.kind {
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Bool(_)
        | ExprKind::Var(_) => {}
        ExprKind::List(xs) | ExprKind::Call(_, xs) | ExprKind::Variant(_, _, xs) => {
            for x in xs {
                walk(x, inner, f);
            }
        }
        ExprKind::StructLit(_, fs) => {
            for (_, x) in fs {
                walk(x, inner, f);
            }
        }
        ExprKind::Method(..) => unreachable!("lowered by the checker"),
        ExprKind::Unary(_, x) | ExprKind::Field(x, _) | ExprKind::Try(x) | ExprKind::Ref(x) => {
            walk(x, inner, f)
        }
        ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) | ExprKind::Range(a, b) => {
            walk(a, inner, f);
            walk(b, inner, f);
        }
        ExprKind::Comp(x, cls) => {
            for cl in cls {
                match cl {
                    Clause::For(_, s) | Clause::If(s) => walk(s, true, f),
                }
            }
            walk(x, true, f);
        }
        ExprKind::Match(x, arms) => {
            walk(x, inner, f);
            for a in arms {
                for s in &a.body {
                    walk_stmt(s, f);
                }
            }
        }
    }
}

fn walk_stmt(s: &Stmt, f: &mut dyn FnMut(&Expr, bool)) {
    let block = |b: &[Stmt], f: &mut dyn FnMut(&Expr, bool)| {
        for s in b {
            walk_stmt(s, f);
        }
    };
    match s {
        Stmt::Let(_, _, e, ..) | Stmt::Expr(e) | Stmt::Return(Some(e), ..) => walk(e, true, f),
        Stmt::Assign(t, _, v) => {
            walk(t, true, f);
            walk(v, true, f);
        }
        Stmt::If(arms, els) => {
            for (c, b) in arms {
                walk(c, true, f);
                block(b, f);
            }
            if let Some(b) = els {
                block(b, f);
            }
        }
        Stmt::While(c, b) | Stmt::ForList(_, c, b) => {
            walk(c, true, f);
            block(b, f);
        }
        Stmt::ForRange(_, a, b, body) => {
            walk(a, true, f);
            walk(b, true, f);
            block(body, f);
        }
        Stmt::Match(e, arms) => {
            walk(e, true, f);
            for a in arms {
                block(&a.body, f);
            }
        }
        Stmt::Pass | Stmt::Break(..) | Stmt::Continue(..) | Stmt::Return(None, ..) => {}
    }
}

/// Names of the variables a statement mentions (conservatively: shadowing
/// names and clause variables count too).
fn stmt_names(s: &Stmt) -> HashSet<String> {
    let mut out = HashSet::new();
    walk_stmt(s, &mut |e, _| {
        if let ExprKind::Var(n) = &e.kind {
            out.insert(n.clone());
        }
    });
    out
}

fn expr_names(e: &Expr) -> HashSet<String> {
    let mut out = HashSet::new();
    walk(e, false, &mut |e, _| {
        if let ExprKind::Var(n) = &e.kind {
            out.insert(n.clone());
        }
    });
    out
}

/// Roots of assignment targets with a field or index step, at any depth.
fn collect_assign_roots(s: &Stmt, out: &mut Vec<String>) {
    let block = |b: &[Stmt], out: &mut Vec<String>| {
        for s in b {
            collect_assign_roots(s, out);
        }
    };
    match s {
        Stmt::Assign(t, ..) if !matches!(t.kind, ExprKind::Var(_)) => out.extend(place_root(t)),
        Stmt::If(arms, els) => {
            for (_, b) in arms {
                block(b, out);
            }
            if let Some(b) = els {
                block(b, out);
            }
        }
        Stmt::While(_, b) | Stmt::ForList(_, _, b) | Stmt::ForRange(_, _, _, b) => block(b, out),
        Stmt::Match(_, arms) => {
            for a in arms {
                block(&a.body, out);
            }
        }
        _ => {}
    }
}

/// Inside `s`, is variable `r` used only through its parts: as the base of
/// `r[i]` / `r.f` (reads or write targets), in `len(r)`, or as the place of
/// push/pop/sort; never whole, never reassigned, never passed with `&`?
fn stmt_uses_only_parts(s: &Stmt, r: &str) -> bool {
    let ok = |e: &Expr| expr_uses_only_parts(e, r);
    let all = |b: &[Stmt]| b.iter().all(|s| stmt_uses_only_parts(s, r));
    match s {
        Stmt::Let(_, _, e, ..) | Stmt::Expr(e) | Stmt::Return(Some(e), ..) => ok(e),
        Stmt::Assign(t, _, v) => {
            !matches!(&t.kind, ExprKind::Var(n) if n == r) && place_parts_ok(t, r) && ok(v)
        }
        Stmt::If(arms, els) => {
            arms.iter().all(|(c, b)| ok(c) && all(b)) && els.as_deref().is_none_or(all)
        }
        Stmt::While(c, b) | Stmt::ForList(_, c, b) => ok(c) && all(b),
        Stmt::ForRange(_, a, b, body) => ok(a) && ok(b) && all(body),
        Stmt::Match(e, arms) => ok(e) && arms.iter().all(|a| all(&a.body)),
        Stmt::Pass | Stmt::Break(..) | Stmt::Continue(..) | Stmt::Return(None, ..) => true,
    }
}

/// The index operands of a place path use `r` only through its parts.
fn place_parts_ok(t: &Expr, r: &str) -> bool {
    match &t.kind {
        ExprKind::Var(_) => true,
        ExprKind::Field(b, _) => place_parts_ok(b, r),
        ExprKind::Index(b, i) => place_parts_ok(b, r) && expr_uses_only_parts(i, r),
        _ => expr_uses_only_parts(t, r),
    }
}

fn expr_uses_only_parts(e: &Expr, r: &str) -> bool {
    let is_r = |x: &Expr| matches!(&x.kind, ExprKind::Var(n) if n == r);
    let ok = |x: &Expr| expr_uses_only_parts(x, r);
    match &e.kind {
        ExprKind::Var(n) => n != r,
        ExprKind::Index(b, i) => (is_r(b) || ok(b)) && ok(i),
        ExprKind::Field(b, _) => is_r(b) || ok(b),
        ExprKind::Call(f, args) if f == "len" && args.len() == 1 && is_r(&args[0]) => true,
        ExprKind::Call(f, args) if matches!(f.as_str(), "push" | "pop" | "sort") => {
            place_parts_ok(&args[0], r) && args[1..].iter().all(ok)
        }
        ExprKind::Ref(x) => place_root(x).is_none_or(|x| x != r) && ok(x),
        ExprKind::Method(..) => unreachable!("lowered by the checker"),
        ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Str(_) | ExprKind::Bool(_) => true,
        ExprKind::List(xs) | ExprKind::Call(_, xs) | ExprKind::Variant(_, _, xs) => {
            xs.iter().all(ok)
        }
        ExprKind::StructLit(_, fs) => fs.iter().all(|(_, x)| ok(x)),
        ExprKind::Unary(_, x) | ExprKind::Try(x) => ok(x),
        ExprKind::Binary(_, a, b) | ExprKind::Range(a, b) => ok(a) && ok(b),
        ExprKind::Comp(x, cls) => {
            ok(x)
                && cls.iter().all(|c| match c {
                    Clause::For(_, s) | Clause::If(s) => ok(s),
                })
        }
        ExprKind::Match(x, arms) => {
            ok(x)
                && arms
                    .iter()
                    .all(|a| a.body.iter().all(|s| stmt_uses_only_parts(s, r)))
        }
    }
}

/// Number of index steps in a place path.
fn index_count(e: &Expr) -> usize {
    match &e.kind {
        ExprKind::Field(b, _) => index_count(b),
        ExprKind::Index(b, _) => 1 + index_count(b),
        _ => 0,
    }
}

/// Does `s` assign to (a part of) variable `root`, at any depth?
fn stmt_assigns(s: &Stmt, root: &str) -> bool {
    let any = |b: &[Stmt]| b.iter().any(|s| stmt_assigns(s, root));
    match s {
        Stmt::Assign(t, ..) => place_root(t).is_some_and(|r| r == root),
        Stmt::If(arms, els) => arms.iter().any(|(_, b)| any(b)) || els.as_deref().is_some_and(any),
        Stmt::While(_, b) | Stmt::ForList(_, _, b) | Stmt::ForRange(_, _, _, b) => any(b),
        Stmt::Match(_, arms) => arms.iter().any(|a| any(&a.body)),
        Stmt::Let(_, _, e, ..) | Stmt::Expr(e) | Stmt::Return(Some(e), ..) => {
            // A match value's arms are statements too.
            let mut hit = false;
            walk(e, false, &mut |x, _| {
                if let ExprKind::Match(_, arms) = &x.kind {
                    hit |= arms.iter().any(|a| any(&a.body));
                }
            });
            hit
        }
        _ => false,
    }
}

/// Variables changed in place by calls inside `e` (`&x` arguments and the
/// list argument of push/pop/sort).
fn mut_roots(e: &Expr) -> HashSet<String> {
    let mut out = HashSet::new();
    walk(e, false, &mut |e, _| {
        if let ExprKind::Call(n, args) = &e.kind {
            for (i, a) in args.iter().enumerate() {
                let r = match &a.kind {
                    ExprKind::Ref(x) => place_root(x),
                    _ if i == 0 && matches!(n.as_str(), "push" | "pop" | "sort") => place_root(a),
                    _ => None,
                };
                out.extend(r);
            }
        }
    });
    out
}

/// May the single use of `name` in this simple statement move its value?
/// Only if it is used exactly once, and not below a comprehension or match.
fn single_use(s: &Stmt, name: &str) -> bool {
    let (mut n, mut inner) = (0, false);
    let mut f = |e: &Expr, i: bool| {
        if matches!(&e.kind, ExprKind::Var(v) if v == name) {
            n += 1;
            inner |= i;
        }
    };
    match s {
        Stmt::Let(_, _, e, ..) | Stmt::Expr(e) | Stmt::Return(Some(e), ..) => {
            walk(e, false, &mut f)
        }
        Stmt::Assign(t, _, v) => {
            walk(t, false, &mut f);
            walk(v, false, &mut f);
        }
        _ => return false,
    }
    n == 1 && !inner
}

/// `({ pre value; })`: run `pre` (operand locals) first.
fn seq(pre: &str, value: String) -> String {
    if pre.is_empty() {
        value
    } else {
        format!("({{ {}{}; }})", pre, value)
    }
}

/// Flatten a string `+` chain into its operands, left to right.
fn concat_parts(e: &Expr) -> Vec<&Expr> {
    let mut parts = Vec::new();
    let mut x = e;
    while let (ExprKind::Binary("+", l, r), Ty::Str) = (&x.kind, &x.ty) {
        parts.push(&**r);
        x = l;
    }
    parts.push(x);
    parts.reverse();
    parts
}

/// Generate C. Returns one or more complete translation units; the first one
/// holds `main`. Several are returned only for large programs when
/// `o.parts > 1`, so the C compiler can run in parallel.
pub fn generate(prog: &Program, types: &crate::check::Types, o: &Opts, root: &str) -> Vec<String> {
    let structs = &types.structs;
    let mut g = Gen {
        out: String::new(),
        strs: Vec::new(),
        struct_idx: prog
            .structs
            .iter()
            .enumerate()
            .map(|(i, s)| (s.name.as_str(), i))
            .collect(),
        structs: &prog.structs,
        sfields: structs,
        scopes: Vec::new(),
        tmp: 0,
        ind: 1,
        ret_unit: true,
        checked: o.checked,
        fofs: 0,
        ensures: &[],
        fname: "",
        ret_ty: Ty::Unit,
        enums: &types.enums,
        einst: Vec::new(),
        eidx: HashMap::new(),
        temps: Vec::new(),
        temp_drops: HashMap::new(),
        next_tid: 0,
        loops: Vec::new(),
        movable: HashSet::new(),
        let_borrow: false,
        writeback: String::new(),
        main_exit: false,
        upc: Vec::new(),
        unique_roots: Vec::new(),
        rc_types: Vec::new(),
        rc_seen: HashSet::new(),
    };
    for s in &prog.structs {
        for (_, t) in &structs[&s.name] {
            g.reg(t);
        }
    }
    let ty_of = |t: &TyExpr| ty_of(t, &types.enums);
    // Tests are only compiled by `limits test`, which never calls `main`.
    let funcs: Vec<&Func> = prog
        .funcs
        .iter()
        .filter(|f| o.tests || f.test.is_none())
        .collect();
    let mut protos = String::new();
    let sig = |f: &Func| -> (String, Vec<(Ty, bool)>, Ty) {
        let r = f.ret.as_ref().map_or(Ty::Unit, ty_of);
        let ps: Vec<(Ty, bool)> = f
            .params
            .iter()
            .zip(&f.inout)
            .map(|((_, t), io)| (ty_of(t), *io))
            .collect();
        (
            format!("LM_FN LM_U {} fn_{}", cty(&r), mangle(&f.name)),
            ps,
            r,
        )
    };
    let pty = |t: &Ty, io: bool| {
        if io {
            format!("{} *", cty(t))
        } else {
            cty(t)
        }
    };
    for f in &funcs {
        let (head, ps, _) = sig(f);
        let ps: Vec<String> = ps.iter().map(|(t, io)| pty(t, *io)).collect();
        let _ = writeln!(
            protos,
            "{}({});",
            head,
            if ps.is_empty() {
                "void".into()
            } else {
                ps.join(", ")
            }
        );
    }
    let mut chunks = Vec::new();
    for f in &funcs {
        let (head, ps, r) = sig(f);
        for (t, _) in &ps {
            g.reg(t);
        }
        g.reg(&r);
        g.scopes = vec![Scope::default()];
        g.temps.clear();
        g.loops.clear();
        g.ret_unit = r == Ty::Unit;
        g.ret_ty = r.clone();
        g.fofs = (f.file as u32) << 20;
        g.fname = &f.name;
        g.ensures = if o.checked { &f.ensures } else { &[] };
        let mut params = Vec::new();
        // An inout parameter is copied in and out (the call has exclusive
        // access to the caller's variable): the body works on a local, kept
        // in a register, and every return stores it back.
        let mut copy_in = String::new();
        g.writeback.clear();
        for ((n, _), (t, io)) in f.params.iter().zip(&ps) {
            let c = g.fresh(&format!("v_{}", n));
            g.scopes[0].names.insert(n.clone(), c.clone());
            g.scopes[0].types.insert(n.clone(), t.clone());
            if *io {
                let p = g.fresh(&format!("p_{}", n));
                params.push(format!("{} LM_U {}", pty(t, true), p));
                let _ = writeln!(copy_in, "  {} LM_U {} = *{};", cty(t), c, p);
                let _ = write!(g.writeback, "*{} = {}; ", p, c);
            } else {
                params.push(format!("{} LM_U {}", pty(t, false), c));
            }
        }
        let _ = writeln!(
            g.out,
            "{}({}) {{\n{}",
            head,
            if params.is_empty() {
                "void".into()
            } else {
                params.join(", ")
            },
            copy_in.trim_end_matches('\n')
        );
        if o.checked {
            let _ = writeln!(g.out, "  LM_ENTER({}, {});", f.line + g.fofs, f.col);
            for e in &f.requires {
                g.contract(e, "requires");
            }
        }
        g.main_exit = f.name == "main" && f.test.is_none() && !o.tests && !o.leak_check;
        g.block(&f.body, None);
        g.main_exit = false;
        if g.ret_unit {
            g.ensures(None);
            let wb = g.writeback.clone();
            if !wb.is_empty() {
                g.line(wb.trim_end());
            }
            g.line("LM_LEAVE;");
        } else {
            g.line("__builtin_unreachable();");
        }
        g.out.push_str("}\n\n");
        let chunk = std::mem::take(&mut g.out);
        chunks.push(g.subst(chunk));
    }
    // Struct declarations, constructors and printers.
    let mut decls = String::new();
    for s in &prog.structs {
        let _ = writeln!(decls, "typedef struct S_{0} S_{0};", mangle(&s.name));
    }
    for t in &g.einst {
        let _ = writeln!(decls, "typedef struct E_{0} E_{0};", tname(t));
    }
    g.enum_decls(&mut decls);
    for s in &prog.structs {
        let fields = &structs[&s.name];
        let _ = writeln!(decls, "struct S_{} {{\n  int64_t rc;", mangle(&s.name));
        for (f, t) in fields {
            let _ = writeln!(decls, "  {} f_{};", cty(t), f);
        }
        decls.push_str("};\n");
        let params: Vec<String> = fields
            .iter()
            .map(|(f, t)| format!("{} f_{}", cty(t), f))
            .collect();
        let _ =
            writeln!(
            decls,
            "static LM_U S_{0} *mk_{0}({1}int l, int c) {{\n  S_{0} *r = lm_alloc(sizeof(S_{0}), l, c);\n  r->rc = 1;",
            mangle(&s.name),
            params.iter().map(|p| format!("{}, ", p)).collect::<String>()
        );
        for (f, _) in fields {
            let _ = writeln!(decls, "  r->f_{0} = f_{0};", f);
        }
        decls.push_str("  return r;\n}\n");
    }
    g.rc_defs(&mut decls);
    decls.push_str("static void lm_struct_repr(lm_buf *b, int idx, void *p) {\n  const char *d;\n  switch (idx) {\n");
    for (i, s) in prog.structs.iter().enumerate() {
        let _ = writeln!(
            decls,
            "  case {}: {{\n    S_{} *v = p;\n    lm_puts(b, \"{} {{\");",
            i,
            mangle(&s.name),
            s.name
        );
        for (j, (f, t)) in structs[&s.name].iter().enumerate() {
            let sep = if j == 0 { "" } else { ", " };
            let _ = writeln!(
                decls,
                "    lm_puts(b, \"{}{}: \"); d = \"{}\"; lm_repr(b, {}, &d, true);",
                sep,
                f,
                g.desc(t),
                to_val(&format!("v->f_{}", f), t)
            );
        }
        decls.push_str("    lm_puts(b, \"}\");\n    break;\n  }\n");
    }
    decls.push_str("  }\n  (void)b; (void)p; (void)d;\n}\n");
    g.enum_repr(&mut decls);
    decls.push_str("static const char *lm_file_name(int i) {\n  static const char *const n[] = {");
    let _ = write!(decls, "\"{}\"", c_escape(root));
    for f in prog.files.iter().skip(1) {
        let _ = write!(decls, ", \"{}\"", c_escape(f));
    }
    decls.push_str("};\n  return i < (int)(sizeof n / sizeof *n) ? n[i] : \"?\";\n}\n");

    let total: usize = chunks.iter().map(String::len).sum();
    let parts = if total < SPLIT_BYTES {
        1
    } else {
        o.parts.clamp(1, total / (SPLIT_BYTES / 2))
    };
    let mut c = String::new();
    if parts > 1 {
        c.push_str("#define LM_SPLIT 1\n");
    }
    let _ = writeln!(
        c,
        "/* generated by limits */\n#define LM_MAX_STEPS {}LL\n#define LM_MAX_DEPTH {}LL\n#define LM_MAX_MEM {}LL",
        o.max_steps.min(i64::MAX as u64),
        o.max_depth.min(i64::MAX as u64),
        o.max_mem.min(i64::MAX as u64)
    );
    if o.checked {
        c.push_str("#define LM_CHECKED 1\n");
    }
    if o.json {
        c.push_str(
            "#define LM_JSON 1\nstatic const char *const lm_codes[] __attribute__((unused)) = {",
        );
        for (_, frag, code) in crate::diag::CODES.iter().filter(|r| r.0 == "runtime") {
            let _ = write!(c, "\"{}\", \"{}\", ", c_escape(frag), code);
        }
        c.push_str("0};\n");
    }
    for (n, xs) in [
        ("read", &o.allow_read),
        ("write", &o.allow_write),
        ("env", &o.allow_env),
    ] {
        let _ = write!(
            c,
            "static const char *const lm_allow_{}[] __attribute__((unused)) = {{",
            n
        );
        for x in xs {
            let _ = write!(c, "\"{}\", ", c_escape(x));
        }
        c.push_str("0};\n");
    }
    if o.tcc {
        c.push_str(include_str!("tcc_shim.h"));
    }
    c.push_str(include_str!("runtime.h"));
    c.push_str("\n/* ---- program ---- */\n");
    c.push_str(&decls);
    for (i, s) in g.strs.iter().enumerate() {
        let _ = writeln!(
            c,
            "static LmStr LM_U lm_s{} = {{LM_IMM, {}, 0, {{\"{}\"}}}};",
            i,
            s.len(),
            c_escape(s)
        );
    }
    c.push_str(&protos);
    c.push('\n');
    // Distribute functions over `parts` files, balancing by size.
    let mut tus = vec![c.clone(); parts];
    tus[0].push_str(
        "LM_DEF LM_U int64_t lm_steps = LM_MAX_STEPS, lm_depth, lm_mem, lm_blocks;\nLM_DEF char *lm_ap, *lm_aend, *lm_stack_lo;\nLM_DEF int lm_rl, lm_rc, lm_argc;\nLM_DEF char **lm_argv;\nLM_DEF lm_buf lm_line;\nLM_DEF void *lm_fl[LM_SMALL / 16 + 1];\nLM_DEF lm_pend *lm_pq;\nLM_DEF int64_t lm_pn, lm_pcap;\nLM_DEF bool lm_draining;\nLM_DEF char **lm_fs_roots;\nLM_DEF int64_t lm_fs_n;\n",
    );
    let per = total / parts + 1;
    let (mut i, mut acc) = (0, 0);
    for ch in chunks {
        if acc >= per * (i + 1) && i + 1 < parts {
            i += 1;
        }
        acc += ch.len();
        tus[i].push_str(&ch);
    }
    let main_env = funcs
        .iter()
        .any(|f| f.name == "main" && f.test.is_none() && !f.params.is_empty());
    let mut body = String::from(if main_env {
        "fn__main(0);"
    } else {
        "fn__main();"
    });
    if o.tests {
        body = "lm_argc--; lm_argv++;\n  switch (argc > 1 ? atoi(argv[1]) : -1) {\n".into();
        for (i, f) in funcs.iter().filter(|f| f.test.is_some()).enumerate() {
            let _ = writeln!(body, "  case {}: fn_{}(); break;", i, mangle(&f.name));
        }
        body.push_str("  default: return 2;\n  }");
    }
    let leak = if o.leak_check {
        "  if (lm_blocks) {\n    fprintf(stderr, \"leak check: %lld heap blocks still live at exit\\n\", (long long)lm_blocks);\n    return 1;\n  }\n"
    } else {
        ""
    };
    let _ = write!(tus[0], "int main(int argc, char **argv) {{\n  static char obuf[1 << 16];\n  (void)argc; (void)argv;\n  setvbuf(stdout, obuf, _IOFBF, sizeof obuf);\n  lm_argc = argc; lm_argv = argv;\n  lm_stack_init();\n  {}\n  fflush(stdout);\n{}  return 0;\n}}\n", body, leak);
    tus
}

/// C name for a (possibly module-qualified) item. Root items get a `_` prefix,
/// qualified ones `M` plus length-prefixed segments, so `a.b_c`, `a_b.c` and a
/// root item `Ma...` can never collide.
fn mangle(n: &str) -> String {
    if !n.contains('.') {
        return format!("_{}", n);
    }
    let mut o = String::from("M");
    for seg in n.split('.') {
        let _ = write!(o, "{}{}", seg.len(), seg);
    }
    o
}

fn ty_of(t: &TyExpr, enums: &HashMap<String, Variants>) -> Ty {
    match t {
        TyExpr::List(t) => Ty::List(Box::new(ty_of(t, enums))),
        TyExpr::Generic(_, args, ..) => {
            let a = Box::new(ty_of(&args[0], enums));
            match args.get(1) {
                Some(e) => Ty::Result(a, Box::new(ty_of(e, enums))),
                None => Ty::Option(a),
            }
        }
        TyExpr::Name(n, ..) => match n.as_str() {
            "int" => Ty::Int,
            "float" => Ty::Float,
            "bool" => Ty::Bool,
            "str" => Ty::Str,
            _ if Cap::ALL.iter().any(|c| c.name() == n) => {
                Ty::Cap(*Cap::ALL.iter().find(|c| c.name() == n).unwrap())
            }
            _ if enums.contains_key(n) => Ty::Enum(n.clone()),
            _ => Ty::Struct(n.clone()),
        },
    }
}

impl<'a> Gen<'a> {
    fn line(&mut self, s: &str) {
        for _ in 0..self.ind {
            self.out.push_str("  ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn fresh(&mut self, base: &str) -> String {
        self.tmp += 1;
        format!("{}_{}", base, self.tmp)
    }

    // ---- scopes, ownership and temporaries ----

    fn push_scope(&mut self) {
        self.scopes.push(Scope::default());
    }

    /// Close a scope, dropping what it still owns.
    fn pop_scope(&mut self) {
        let s = self.scopes.pop().expect("scope");
        for (_, c, t) in s.owned.iter().rev() {
            let d = self.drop_code(t, c);
            self.line(&d);
        }
    }

    /// Bind a source name to a new C local in the current scope; a heap
    /// value is owned by it.
    fn bind(&mut self, name: &str, t: &Ty, owned: bool) -> String {
        let c = self.fresh(&format!("v_{}", name));
        let s = self.scopes.last_mut().unwrap();
        s.names.insert(name.to_string(), c.clone());
        s.types.insert(name.to_string(), t.clone());
        if owned && heap(t) {
            s.owned.push((name.to_string(), c.clone(), t.clone()));
        }
        c
    }

    /// A hidden owned local (a loop's copy of its list, a comprehension's
    /// result): dropped with its scope, or on an early exit.
    fn hide(&mut self, c: &str, t: &Ty) {
        if heap(t) {
            let s = self.scopes.last_mut().unwrap();
            s.owned.push((String::new(), c.to_string(), t.clone()));
        }
    }

    fn lookup(&self, name: &str) -> String {
        self.scopes
            .iter()
            .rev()
            .find_map(|s| s.names.get(name))
            .cloned()
            .expect("checked")
    }

    /// Type of a variable in scope.
    fn var_type(&self, name: &str) -> Option<Ty> {
        self.scopes
            .iter()
            .rev()
            .find_map(|s| s.types.get(name))
            .cloned()
    }

    /// C name of a variable (an inout parameter's local copy, see
    /// `generate`).
    fn var(&self, name: &str) -> String {
        self.lookup(name)
    }

    /// Is `name` an owned local whose value this use may take?
    fn movable_var(&self, name: &str) -> Option<(String, Ty)> {
        let c = self.lookup(name);
        if !self.movable.contains(&c) {
            return None;
        }
        self.scopes
            .iter()
            .flat_map(|s| &s.owned)
            .find(|o| o.1 == c)
            .map(|o| (c.clone(), o.2.clone()))
    }

    fn drop_code(&mut self, t: &Ty, x: &str) -> String {
        self.need(t);
        format!("lm_drop_{}({});", tname(t), x)
    }

    /// Open a temporary scope; returns its id.
    fn push_temps(&mut self) -> usize {
        self.next_tid += 1;
        self.temps.push((self.next_tid, Vec::new()));
        self.next_tid
    }

    /// Close the innermost temporary scope: (declarations, drops).
    fn pop_temps(&mut self) -> (String, String) {
        let (id, ts) = self.temps.pop().expect("temp scope");
        let (mut decl, mut drops) = (String::new(), String::new());
        for (n, t) in &ts {
            let _ = write!(decl, "{} {} = 0; ", cty(t), n);
            let d = self.drop_code(t, n);
            drops.push_str(&d);
            drops.push(' ');
        }
        self.temp_drops.insert(id, drops.clone());
        (decl, drops)
    }

    /// Keep an owned value in a temporary of the current statement; yields
    /// the value (borrowed from the temporary).
    fn temp(&mut self, t: &Ty, code: String) -> String {
        let n = self.fresh("t");
        self.temps
            .last_mut()
            .expect("temp scope")
            .1
            .push((n.clone(), t.clone()));
        format!("({} = {})", n, code)
    }

    /// Evaluate `f` (code of type `t`, owned or scalar) with its own
    /// temporaries, dropped once the value is computed.
    fn scoped(&mut self, t: &Ty, f: impl FnOnce(&mut Self) -> String) -> String {
        self.push_temps();
        let code = f(self);
        let (decl, drops) = self.pop_temps();
        if decl.is_empty() {
            return code;
        }
        if *t == Ty::Unit {
            return format!("({{ {}(void)({}); {}}})", decl, code, drops);
        }
        let r = self.fresh("r");
        format!(
            "({{ {}{} {} = {}; {}{}; }})",
            decl,
            cty(t),
            r,
            code,
            drops,
            r
        )
    }

    /// Emit the lines of `f` with their own temporaries.
    fn scoped_stmt(&mut self, f: impl FnOnce(&mut Self)) {
        self.push_temps();
        let body = self.capture(f);
        let (decl, drops) = self.pop_temps();
        if decl.is_empty() {
            self.out.push_str(&body);
            return;
        }
        self.line(&format!("{{ {}", decl));
        self.out.push_str(&body);
        self.line(&format!("{}}}", drops));
    }

    /// Run `f`, returning the C lines it emits as a string (at the current
    /// indentation).
    fn capture(&mut self, f: impl FnOnce(&mut Self)) -> String {
        let saved = std::mem::take(&mut self.out);
        f(self);
        std::mem::replace(&mut self.out, saved)
    }

    /// Code dropping everything owned by scopes from `scope` up and by the
    /// temporary scopes from `temp` up (an early exit leaving them).
    fn exit_drops(&mut self, scope: usize, temp: usize) -> String {
        let mut o = String::new();
        let owned: Vec<(String, Ty)> = self.scopes[scope..]
            .iter()
            .rev()
            .flat_map(|s| s.owned.iter().rev().map(|x| (x.1.clone(), x.2.clone())))
            .collect();
        for (c, t) in owned {
            let d = self.drop_code(&t, &c);
            o.push_str(&d);
            o.push(' ');
        }
        // Temporaries are listed once their scope closes: placeholder.
        for (id, _) in self.temps[temp..].iter().rev() {
            let _ = write!(o, "\u{1}{}\u{1}", id);
        }
        o
    }

    /// Replace the early-exit placeholders with the temporaries' drops.
    fn subst(&self, chunk: String) -> String {
        if !chunk.contains('\u{1}') {
            return chunk;
        }
        let mut o = String::with_capacity(chunk.len());
        for (i, part) in chunk.split('\u{1}').enumerate() {
            if i % 2 == 0 {
                o.push_str(part);
            } else {
                let id: usize = part.parse().expect("placeholder");
                o.push_str(&self.temp_drops[&id]);
            }
        }
        o
    }

    /// Register the drop (and copy) functions `t` needs, recursively.
    fn need(&mut self, t: &Ty) {
        if !heap(t) || !self.rc_seen.insert(tname(t)) {
            return;
        }
        self.reg(t);
        self.rc_types.push(t.clone());
        for c in self.children(t) {
            self.need(&c);
        }
    }

    /// Types of the values a value of type `t` holds.
    fn children(&self, t: &Ty) -> Vec<Ty> {
        match t {
            Ty::List(e) => vec![(**e).clone()],
            Ty::Struct(n) => self.sfields[n].iter().map(|f| f.1.clone()).collect(),
            Ty::Enum(_) | Ty::Option(_) | Ty::Result(..) => self
                .variants(t)
                .into_iter()
                .flat_map(|v| v.1.into_iter().map(|f| f.1))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Drop, free and (structs) copy functions of every registered type.
    fn rc_defs(&mut self, d: &mut String) {
        let types = self.rc_types.clone();
        let mut defs = String::new();
        for t in types.iter().filter(|t| **t != Ty::Str) {
            let k = tname(t);
            let ct = cty(t);
            let kids = self.children(t);
            let leaf = kids.iter().all(|c| !heap(c));
            let _ = writeln!(d, "static LM_U void lm_fr_{}(void *p);", k);
            let rel = if leaf {
                format!("lm_fr_{}(x)", k)
            } else {
                format!("lm_release(x, lm_fr_{})", k)
            };
            let _ = writeln!(
                d,
                "static inline LM_U void lm_drop_{}({} x) {{ if (x && --x->rc == 0) {}; }}",
                k, ct, rel
            );
            let _ = writeln!(
                defs,
                "static LM_U void lm_fr_{}(void *p) {{\n  {} x = p;",
                k, ct
            );
            match t {
                Ty::List(e) => {
                    let (_, st) = elem(t);
                    if heap(e) {
                        let _ = writeln!(
                            defs,
                            "  for (int64_t i = 0; i < x->len; i++) lm_drop_{}(({})((void **)x->d)[i]);",
                            tname(e),
                            cty(e)
                        );
                    }
                    let _ = writeln!(defs, "  lm_list_free(x, sizeof({}));", st);
                }
                Ty::Struct(n) => {
                    for (f, ft) in &self.sfields[n] {
                        if heap(ft) {
                            let _ = writeln!(defs, "  lm_drop_{}(x->f_{});", tname(ft), f);
                        }
                    }
                    let _ = writeln!(defs, "  lm_free(x, sizeof(*x));");
                    // Copy-on-write support: a field write needs a unique struct.
                    let m = mangle(n);
                    let _ = writeln!(
                        d,
                        "static LM_U S_{0} *lm_clone_{1}(S_{0} *x, int l, int c);",
                        m, k
                    );
                    let mut dups = String::new();
                    for (f, ft) in &self.sfields[n] {
                        if heap(ft) {
                            let _ = write!(dups, " LM_RC(y->f_{})++;", f);
                        }
                    }
                    let _ = writeln!(
                        defs,
                        "}}\n/* copy of a shared struct (the caller stores it in its place) */\nstatic LM_U __attribute__((noinline)) S_{0} *lm_clone_{1}(S_{0} *x, int l, int c) {{\n  S_{0} *y = lm_alloc(sizeof(S_{0}), l, c);\n  *y = *x; y->rc = 1; x->rc--;{2}\n  return y;",
                        m, k, dups
                    );
                }
                _ => {
                    let vs = self.variants(t);
                    let mut cases = String::new();
                    for (i, (_, fs)) in vs.iter().enumerate() {
                        let mut ds = String::new();
                        for (j, (_, ft)) in fs.iter().enumerate() {
                            if heap(ft) {
                                let _ = write!(ds, " lm_drop_{}(x->u.v{}.f{});", tname(ft), i, j);
                            }
                        }
                        if !ds.is_empty() {
                            let _ = writeln!(cases, "  case {}:{} break;", i, ds);
                        }
                    }
                    if !cases.is_empty() {
                        let _ = writeln!(defs, "  switch (x->tag) {{\n{}  }}", cases);
                    }
                    let _ = writeln!(defs, "  lm_free(x, sizeof(*x));");
                }
            }
            defs.push_str("}\n");
        }
        d.push_str(&defs);
    }

    /// Register the sum-type instances `t` uses (recursively).
    fn reg(&mut self, t: &Ty) {
        match t {
            Ty::List(x) => self.reg(x),
            Ty::Enum(_) | Ty::Option(_) | Ty::Result(..) => {
                let k = tname(t);
                if self.eidx.contains_key(&k) {
                    return;
                }
                self.eidx.insert(k, self.einst.len());
                self.einst.push(t.clone());
                for (_, fs) in self.variants(t) {
                    for (_, ft) in fs {
                        self.reg(&ft);
                    }
                }
            }
            _ => {}
        }
    }

    fn variants(&self, t: &Ty) -> Variants {
        variants_of(t, self.enums).expect("sum type")
    }

    /// C struct, constructors and unit-variant constants of every instance.
    fn enum_decls(&self, d: &mut String) {
        for t in &self.einst {
            let n = tname(t);
            let vs = self.variants(t);
            let _ = writeln!(d, "struct E_{} {{\n  int64_t rc, tag;", n);
            if vs.iter().any(|v| !v.1.is_empty()) {
                d.push_str("  union {\n");
                for (i, (_, fs)) in vs.iter().enumerate().filter(|v| !v.1 .1.is_empty()) {
                    d.push_str("    struct {");
                    for (k, (_, ft)) in fs.iter().enumerate() {
                        let _ = write!(d, " {} f{};", cty(ft), k);
                    }
                    let _ = writeln!(d, " }} v{};", i);
                }
                d.push_str("  } u;\n");
            }
            d.push_str("};\n");
            for (i, (_, fs)) in vs.iter().enumerate() {
                if fs.is_empty() {
                    let _ = writeln!(
                        d,
                        "static LM_U E_{0} ev_{0}_{1} = {{ .rc = LM_IMM, .tag = {1} }};",
                        n, i
                    );
                    continue;
                }
                let ps: String = fs
                    .iter()
                    .enumerate()
                    .map(|(k, (_, ft))| format!("{} a{}, ", cty(ft), k))
                    .collect();
                let _ = writeln!(
                    d,
                    "static LM_U E_{0} *mk_{0}_{1}({2}int l, int c) {{\n  E_{0} *r = lm_alloc(sizeof(E_{0}), l, c);\n  r->rc = 1; r->tag = {1};",
                    n, i, ps
                );
                for k in 0..fs.len() {
                    let _ = writeln!(d, "  r->u.v{}.f{} = a{};", i, k, k);
                }
                d.push_str("  return r;\n}\n");
            }
        }
    }

    /// `lm_enum_repr`: prints `Circle(1.0)`, `Some("x")`, `None`.
    fn enum_repr(&self, d: &mut String) {
        d.push_str("static void lm_enum_repr(lm_buf *b, int idx, void *p) {\n  const char *d;\n  switch (idx) {\n");
        for (j, t) in self.einst.iter().enumerate() {
            let n = tname(t);
            let prefix = match t {
                Ty::Enum(q) => q
                    .rsplit_once('.')
                    .map_or(String::new(), |(m, _)| format!("{}.", m)),
                _ => String::new(),
            };
            let _ = writeln!(
                d,
                "  case {}: {{\n    E_{} *v = p;\n    switch (v->tag) {{",
                j, n
            );
            for (i, (vn, fs)) in self.variants(t).iter().enumerate() {
                let _ = write!(
                    d,
                    "    case {}: lm_puts(b, \"{}{}",
                    i,
                    c_escape(&prefix),
                    vn
                );
                if fs.is_empty() {
                    d.push_str("\"); break;\n");
                    continue;
                }
                d.push_str("(\");");
                for (k, (_, ft)) in fs.iter().enumerate() {
                    if k > 0 {
                        d.push_str(" lm_puts(b, \", \");");
                    }
                    let _ = write!(
                        d,
                        " d = \"{}\"; lm_repr(b, {}, &d, true);",
                        self.desc(ft),
                        to_val(&format!("v->u.v{}.f{}", i, k), ft)
                    );
                }
                d.push_str(" lm_puts(b, \")\"); break;\n");
            }
            d.push_str("    }\n    break;\n  }\n");
        }
        d.push_str("  }\n  (void)b; (void)p; (void)d;\n}\n");
    }

    /// C value of variant `i` of sum type `t` with (owned) payload `args`.
    fn mk_variant(&self, t: &Ty, i: usize, args: &[String], l: u32, c: u32) -> String {
        let n = tname(t);
        if args.is_empty() {
            format!("(&ev_{}_{})", n, i)
        } else {
            format!("mk_{}_{}({}, {}, {})", n, i, args.join(", "), l, c)
        }
    }

    fn desc(&self, t: &Ty) -> String {
        match t {
            Ty::Int => "i".into(),
            Ty::Float => "f".into(),
            Ty::Bool => "b".into(),
            Ty::Str => "s".into(),
            Ty::List(t) => format!("L{}", self.desc(t)),
            Ty::Struct(n) => format!("S{};", self.struct_idx[n.as_str()]),
            Ty::Enum(_) | Ty::Option(_) | Ty::Result(..) => format!("E{};", self.eidx[&tname(t)]),
            _ => unreachable!(),
        }
    }

    /// Return the owned C value `c` of type `t`: drop every local and
    /// temporary, check `ensures`, leave.
    fn emit_return(&mut self, c: String, t: &Ty) {
        let leave = if self.checked { "LM_LEAVE; " } else { "" };
        let r = self.fresh("r");
        let drops = self.exit_drops(0, 0);
        self.line(&format!("{{ {} {} = {}; {}", cty(t), r, c, drops));
        self.ensures(Some(&r));
        let wb = self.writeback.clone();
        self.line(&format!("  {}{}return {}; }}", wb, leave, r));
    }

    // ---- statements ----

    /// A block; with `target`, a final expression statement is its value,
    /// stored (owned) into the C variable `target`. Each local is dropped
    /// right after the statement that uses it last; that use may move it.
    fn block(&mut self, b: &[Stmt], target: Option<&str>) {
        // The top-level block of `main` (only the parameter scope is open).
        let top = self.main_exit && self.scopes.len() == 1;
        self.push_scope();
        let depth = self.scopes.len() - 1;
        let mut last: HashMap<String, usize> = HashMap::new();
        for (i, s) in b.iter().enumerate() {
            for n in stmt_names(s) {
                last.insert(n, i);
            }
        }
        self.upc.clear();
        for (i, s) in b.iter().enumerate() {
            let safe = self.cache_safe(s);
            if !safe {
                self.upc.clear();
            } else if let Stmt::Assign(t, ..) = s {
                // Make the written value's container unique once, here, and
                // remember it for the following statements.
                if let ExprKind::Field(base, _) | ExprKind::Index(base, _) = &t.kind {
                    if let Some(k) = self.path_key(base) {
                        if self.cached(base).is_none() {
                            let ix = self.place_indices(base);
                            let mut it = ix.into_iter();
                            let v = self.uniq_val(base, &mut it);
                            let u = self.fresh("pc");
                            self.line(&format!("{} LM_U {} = {};", cty(&base.ty), u, v));
                            self.upc.push((k, u));
                        }
                    }
                }
            }
            let simple = matches!(
                s,
                Stmt::Let(..) | Stmt::Assign(..) | Stmt::Expr(_) | Stmt::Return(Some(_), ..)
            );
            let mut movable = HashSet::new();
            if simple {
                for (n, c, _) in &self.scopes[depth].owned {
                    if !n.is_empty() && last.get(n) == Some(&i) && single_use(s, n) {
                        movable.insert(c.clone());
                    }
                }
            }
            if let Stmt::Return(Some(_), ..) = s {
                // Every local dies at a return: any single use may move.
                for sc in &self.scopes {
                    for (n, c, _) in &sc.owned {
                        if !n.is_empty() && single_use(s, n) {
                            movable.insert(c.clone());
                        }
                    }
                }
            }
            let saved = std::mem::replace(&mut self.movable, movable);
            self.let_borrow = match s {
                Stmt::Let(_, _, e, _, _, false) => self.let_borrows(e, &b[i + 1..], depth),
                _ => false,
            };
            match (target, s) {
                (Some(r), Stmt::Expr(v)) if i + 1 == b.len() => {
                    let t = v.ty.clone();
                    let c = self.scoped(&t, |g| g.own(v));
                    self.line(&format!("{} = {};", r, c));
                }
                _ => self.stmt(s),
            }
            self.movable = saved;
            if !safe {
                self.upc.clear();
            } else if let Stmt::Assign(t, ..) = s {
                // A changed index variable changes which element a path names.
                if let ExprKind::Var(n) = &t.kind {
                    let c = self.lookup(n);
                    self.upc.retain(|(k, _)| !k.contains(&c));
                }
            }
            // Drop the locals of this block that are not used after `s`.
            let dead: Vec<(String, Ty)> = self.scopes[depth]
                .owned
                .iter()
                .filter(|(n, _, _)| !n.is_empty() && last.get(n).is_none_or(|&l| l <= i))
                .map(|(_, c, t)| (c.clone(), t.clone()))
                .collect();
            if !dead.is_empty() && i + 1 < b.len() {
                for (c, t) in &dead {
                    let d = self.drop_code(t, c);
                    self.line(&d);
                }
                self.scopes[depth]
                    .owned
                    .retain(|(_, c, _)| !dead.iter().any(|d| d.0 == *c));
            }
        }
        self.upc.clear();
        if top {
            // The end of the program: the OS reclaims the memory.
            self.scopes.pop();
            return;
        }
        self.pop_scope();
    }

    /// May `let x = e` borrow `e` instead of holding its own reference?
    /// Yes when `e` is a path (`v`, `v.f`, `v[i].g`) into a variable that
    /// outlives the block (a parameter or a variable of an enclosing scope)
    /// and that the rest of the block never changes: `x` then cannot be told
    /// apart from a copy, and costs no reference count.
    fn let_borrows(&self, e: &Expr, rest: &[Stmt], depth: usize) -> bool {
        if !heap(&e.ty) {
            return false;
        }
        let mut x = e;
        let root = loop {
            match &x.kind {
                ExprKind::Field(b, _) => x = b,
                ExprKind::Index(b, i) if mut_roots(i).is_empty() => x = b,
                ExprKind::Var(r) => break r,
                _ => return false,
            }
        };
        if self.scopes[depth].names.contains_key(root) {
            return false;
        }
        !rest.iter().any(|s| {
            let mut hit = false;
            walk_stmt(s, &mut |e, _| {
                hit |= mut_roots(e).contains(root);
            });
            hit || stmt_assigns(s, root)
        })
    }

    fn sub_block(&mut self, b: &[Stmt]) {
        self.ind += 1;
        self.block(b, None);
        self.ind -= 1;
    }

    fn step(&mut self, l: u32, c: u32) {
        if self.checked {
            self.line(&format!("LM_STEP({}, {});", l + self.fofs, c));
        }
    }

    /// Enter a loop body: early exits by break/continue drop from here.
    fn loop_enter(&mut self) {
        self.loops.push((self.scopes.len(), self.temps.len()));
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Pass => {}
            Stmt::Let(name, _, e, ..) => {
                let t = e.ty.clone();
                let borrowed = std::mem::take(&mut self.let_borrow);
                let v = if borrowed {
                    self.scoped(&t, |g| g.ex(e).0)
                } else {
                    self.scoped(&t, |g| g.own(e))
                };
                let c = self.bind(name, &t, !borrowed);
                self.line(&format!("{} LM_U {} = {};", cty(&t), c, v));
            }
            Stmt::Assign(t, op, v) => self.scoped_stmt(|g| g.assign(t, *op, v)),
            Stmt::Expr(e) => {
                let c = self.scoped(&Ty::Unit, |g| g.borrow(e));
                self.line(&format!("(void)({});", c));
            }
            Stmt::If(arms, els) => {
                for (i, (cond, b)) in arms.iter().enumerate() {
                    let c = self.scoped(&Ty::Bool, |g| g.ex(cond).0);
                    let kw = if i == 0 { "if" } else { "} else if" };
                    self.line(&format!("{} ((bool){}) {{", kw, c));
                    self.sub_block(b);
                }
                if let Some(b) = els {
                    self.line("} else {");
                    self.sub_block(b);
                }
                self.line("}");
            }
            Stmt::While(cond, b) => {
                let n = self.hoist_unique(s);
                let c = self.scoped(&Ty::Bool, |g| g.ex(cond).0);
                self.line(&format!("while ((bool){}) {{", c));
                self.ind += 1;
                self.step(cond.line, cond.col);
                self.loop_enter();
                self.block(b, None);
                self.loops.pop();
                self.ind -= 1;
                self.line("}");
                self.unique_roots.truncate(n);
            }
            Stmt::ForRange(v, a, b, body) => {
                let n = self.hoist_unique(s);
                let ca = self.scoped(&Ty::Int, |g| g.ex(a).0);
                let cb = self.scoped(&Ty::Int, |g| g.ex(b).0);
                let (k, e) = (self.fresh("k"), self.fresh("e"));
                self.line(&format!(
                    "for (int64_t {k} = {ca}, {e} = {cb}; {k} < {e}; {k}++) {{"
                ));
                self.ind += 1;
                self.step(a.line, a.col);
                self.push_scope();
                let cv = self.bind(v, &Ty::Int, false);
                self.line(&format!("int64_t LM_U {} = {};", cv, k));
                self.loop_enter();
                self.block(body, None);
                self.loops.pop();
                self.scopes.pop();
                self.ind -= 1;
                self.line("}");
                self.unique_roots.truncate(n);
            }
            Stmt::ForList(v, e, body) => {
                // The loop iterates over its own reference to the list, so
                // changes to the variable inside the body do not affect it.
                let n = self.hoist_unique(s);
                let lt = e.ty.clone();
                let ce = self.scoped(&lt, |g| g.own(e));
                let (k, l) = (self.fresh("k"), self.fresh("l"));
                let et = elem_ty(&lt);
                let st = elem(&lt).1;
                self.line(&format!("{{ lm_list *{l} = {ce};"));
                self.push_scope();
                self.hide(&l, &lt);
                self.line(&format!("for (int64_t {k} = 0; {k} < {l}->len; {k}++) {{"));
                self.ind += 1;
                self.step(e.line, e.col);
                self.push_scope();
                let cv = self.bind(v, &et, false);
                self.line(&format!(
                    "{} LM_U {} = {};",
                    cty(&et),
                    cv,
                    from_slot(format!("(({st} *){l}->d)[{k}]"), &et)
                ));
                self.loop_enter();
                self.block(body, None);
                self.loops.pop();
                self.scopes.pop();
                self.ind -= 1;
                self.line("}");
                self.pop_scope();
                self.line("}");
                self.unique_roots.truncate(n);
            }
            Stmt::Break(..) | Stmt::Continue(..) => {
                let (sd, td) = *self.loops.last().expect("checked: inside a loop");
                let drops = self.exit_drops(sd, td);
                let kw = if matches!(s, Stmt::Break(..)) {
                    "break"
                } else {
                    "continue"
                };
                self.line(&format!("{{ {}{}; }}", drops, kw));
            }
            Stmt::Return(e, ..) => match e {
                None => {
                    let leave = if self.checked { "LM_LEAVE; " } else { "" };
                    let drops = self.exit_drops(0, 0);
                    self.line(&format!("{{ {}", drops));
                    self.ensures(None);
                    let wb = self.writeback.clone();
                    self.line(&format!("  {}{}return;\n}}", wb, leave));
                }
                Some(e) => {
                    let t = e.ty.clone();
                    self.scoped_stmt(|g| {
                        let c = g.own(e);
                        g.emit_return(c, &t);
                    });
                }
            },
            Stmt::Match(e, arms) => self.match_code(e, arms, None),
        }
    }

    /// `t = v` / `t op= v` (inside a statement temp scope).
    fn assign(&mut self, t: &Expr, op: Option<(&'static str, u32, u32)>, v: &Expr) {
        let ty = t.ty.clone();
        let ct = cty(&ty);
        let (tl, tc) = (t.line + self.fofs, t.col);
        // String appends grow a unique string in place: `s += x`,
        // `s = s + x + y` (when the other parts do not read `s` itself).
        if ty == Ty::Str {
            let root = place_root(t).unwrap_or_default();
            let parts: Option<Vec<&Expr>> = match (op, &v.kind) {
                (Some(("+", ..)), _) => Some(concat_parts(v)),
                (None, ExprKind::Binary("+", ..)) => {
                    let ps = concat_parts(v);
                    let same = |e: &Expr, t: &Expr| match (&e.kind, &t.kind) {
                        (ExprKind::Var(a), ExprKind::Var(b)) => a == b,
                        _ => false,
                    };
                    if same(ps[0], t) {
                        Some(ps[1..].to_vec())
                    } else {
                        None
                    }
                }
                _ => None,
            };
            if let Some(ps) = parts.filter(|ps| {
                ps.iter().all(|p| !expr_names(p).contains(&root)) && mut_roots(v).is_empty()
            }) {
                let (ol, oc) = op.map_or((tl, tc), |(_, l, c)| (l + self.fofs, c));
                let idx = self.place_indices(t);
                let (pre, cs) = self.borrow_all(&ps);
                if !pre.is_empty() {
                    self.line(&pre);
                }
                let app = |cur: &str| {
                    format!(
                        "lm_str_append({}, {}, (lm_str[]){{{}}}, {}, {})",
                        cur,
                        cs.len(),
                        cs.join(", "),
                        ol,
                        oc
                    )
                };
                let code = if let ExprKind::Var(n) = &t.kind {
                    let lv = self.var(n);
                    format!("{} = {};", lv, self.costly("lm_str", app(&lv)))
                } else {
                    let mut it = idx.into_iter();
                    let (sl, st) = self.slot(t, &mut it);
                    let call = self.costly("lm_str", app("(lm_str)*lm_p_"));
                    format!("{{ {st} *lm_p_ = {sl}; *lm_p_ = {call}; }}")
                };
                self.line(&code);
                return;
            }
        }
        if let ExprKind::Var(n) = &t.kind {
            let lv = self.var(n);
            match op {
                None => {
                    let nv = self.own(v);
                    if heap(&ty) {
                        let d = self.drop_code(&ty, &lv);
                        self.line(&format!(
                            "{{ {} lm_n_ = {}; {} {} = lm_n_; }}",
                            ct, nv, d, lv
                        ));
                    } else {
                        self.line(&format!("{} = {};", lv, nv));
                    }
                }
                Some((op, ol, oc)) => {
                    let y = self.borrow(v);
                    let r = self.binop(op, &ty, lv.clone(), y, ol + self.fofs, oc);
                    if heap(&ty) {
                        // `s += s` and the like: a new string replaces s.
                        let d = self.drop_code(&ty, "lm_o_");
                        self.line(&format!("{{ {ct} lm_o_ = {lv}; {lv} = {r}; {d} }}"));
                    } else {
                        self.line(&format!("{} = {};", lv, r));
                    }
                }
            }
            return;
        }
        // Index operands first, then the value, then the (unique) slot.
        let idx = self.place_indices(t);
        let y = self.fresh("y");
        let val = match op {
            None => self.own(v),
            Some(_) => self.borrow(v),
        };
        self.line(&format!("{} {} = {};", ct, y, val));
        let mut it = idx.into_iter();
        let (sl, st) = self.slot(t, &mut it);
        let cur = if st == ct {
            "*lm_p_".to_string()
        } else {
            format!("(({})*lm_p_)", ct)
        };
        match op {
            None if heap(&ty) => {
                let d = self.drop_code(&ty, "lm_o_");
                self.line(&format!(
                    "{{ {st} *lm_p_ = {sl}; {ct} lm_o_ = {cur}; *lm_p_ = {y}; {d} }}"
                ));
            }
            None => self.line(&format!("*{} = {};", sl, y)),
            Some((op, ol, oc)) => {
                let (ol, oc) = (ol + self.fofs, oc);
                let r = self.binop(op, &ty, cur.clone(), y, ol, oc);
                if heap(&ty) {
                    // `p.s += p.s`-like appends that read the target: concat.
                    let d = self.drop_code(&ty, "lm_o_");
                    self.line(&format!(
                        "{{ {st} *lm_p_ = {sl}; {ct} lm_o_ = {cur}; *lm_p_ = {r}; {d} }}"
                    ));
                } else {
                    self.line(&format!("{{ {st} *lm_p_ = {sl}; *lm_p_ = {r}; }}"));
                }
            }
        }
    }

    /// Evaluate the index operands of a place (root first) into locals;
    /// returns their names.
    fn place_indices(&mut self, e: &Expr) -> Vec<String> {
        let mut out = Vec::new();
        self.place_idx(e, &mut out);
        out
    }

    fn place_idx(&mut self, e: &Expr, out: &mut Vec<String>) {
        match &e.kind {
            ExprKind::Field(x, _) => self.place_idx(x, out),
            ExprKind::Index(x, i) => {
                self.place_idx(x, out);
                let k = self.fresh("i");
                let ci = self.ex(i).0;
                self.line(&format!("int64_t LM_U {} = {};", k, ci));
                out.push(k);
            }
            _ => {}
        }
    }

    /// Cache key of a simple place path (`v_bs_3[v_i_7].f_vel`): a
    /// variable with field and index steps whose indexes are variables or
    /// literals.
    fn path_key(&self, e: &Expr) -> Option<String> {
        match &e.kind {
            ExprKind::Var(n) => Some(self.lookup(n)),
            ExprKind::Field(b, f) => Some(format!("{}.{}", self.path_key(b)?, f)),
            ExprKind::Index(b, i) => {
                let k = match &i.kind {
                    ExprKind::Var(n) => self.lookup(n),
                    ExprKind::Int(n) => n.to_string(),
                    _ => return None,
                };
                Some(format!("{}[{}]", self.path_key(b)?, k))
            }
            _ => None,
        }
    }

    /// The C local holding place `e`'s value, if it is cached as unique.
    fn cached(&self, e: &Expr) -> Option<String> {
        if self.upc.is_empty() {
            return None;
        }
        let k = self.path_key(e)?;
        self.upc.iter().find(|x| x.0 == k).map(|x| x.1.clone())
    }

    /// Can statement `s` keep the unique-path cache valid? It must not
    /// create a new reference to any value that outlives it: only scalar
    /// `let`s and scalar assignments, with no `&` arguments, no
    /// push/pop/sort and no match (whose arms are statements).
    fn cache_safe(&self, s: &Stmt) -> bool {
        let pure = |e: &Expr| {
            let mut ok = true;
            walk(e, false, &mut |x, _| {
                ok &= !matches!(x.kind, ExprKind::Match(..) | ExprKind::Ref(_))
            });
            ok && mut_roots(e).is_empty()
        };
        match s {
            Stmt::Pass => true,
            Stmt::Let(_, _, e, ..) => !heap(&e.ty) && pure(e),
            Stmt::Assign(t, _, v) => {
                !heap(&t.ty)
                    && pure(t)
                    && pure(v)
                    && (matches!(t.kind, ExprKind::Var(_)) || self.path_key(t).is_some())
            }
            _ => false,
        }
    }

    /// Before loop `s`: make unique, once, every variable the loop writes
    /// through (`xs[i] = ..`, `p.f += ..`, `push(xs, ..)`) but cannot
    /// share: inside the loop it is used only as the base of `[i]` or `.f`,
    /// in `len()`, or as the list of push/pop/sort, never whole (which
    /// could copy its reference), never reassigned and never passed with
    /// `&`. Its writes in the loop then skip the copy-on-write check.
    /// Returns the previous length of `unique_roots` (restore after).
    fn hoist_unique(&mut self, s: &Stmt) -> usize {
        let n = self.unique_roots.len();
        let mut roots: Vec<String> = Vec::new();
        walk_stmt(s, &mut |e, _| {
            if let ExprKind::Call(f, args) = &e.kind {
                if matches!(f.as_str(), "push" | "pop" | "sort") {
                    roots.extend(args.first().and_then(place_root));
                }
            }
        });
        collect_assign_roots(s, &mut roots);
        roots.sort();
        roots.dedup();
        for r in roots {
            let Some(c) = self
                .scopes
                .iter()
                .rev()
                .find_map(|sc| sc.names.get(&r))
                .cloned()
            else {
                continue; // declared inside the loop
            };
            let t = self.var_type(&r);
            if self.unique_roots.contains(&c) || !matches!(t, Some(Ty::List(_) | Ty::Struct(_))) {
                continue;
            }
            if !stmt_uses_only_parts(s, &r) {
                continue;
            }
            // Errors (memory limit) in the copy point at the loop head.
            let (line, col) = match s {
                Stmt::While(e, _) | Stmt::ForRange(_, e, ..) | Stmt::ForList(_, e, _) => {
                    (e.line, e.col)
                }
                _ => (0, 0),
            };
            let v = Expr {
                kind: ExprKind::Var(r.clone()),
                line,
                col,
                ty: t.unwrap(),
            };
            let code = self.uniq_val(&v, &mut Vec::new().into_iter());
            self.line(&format!("(void)({});", code));
            self.unique_roots.push(c);
        }
        n
    }

    /// Call copying a shared list or struct `x` (it gives up one reference).
    fn copy_call(&mut self, t: &Ty, x: &str, l: u32, c: u32) -> String {
        match t {
            Ty::List(e) => format!(
                "lm_list_copy({}, sizeof({}), {}, {}, {})",
                x,
                elem(t).1,
                heap(e),
                l,
                c
            ),
            _ => {
                self.need(t);
                format!("lm_clone_{}({}, {}, {})", tname(t), x, l, c)
            }
        }
    }

    /// Code yielding the (list or struct) value of place `e`, made unique
    /// first: a shared value is copied and the copy stored back in the
    /// place, so later writes through it change only this place. Nothing
    /// is stored when the value is already unique. Index operands come from
    /// `idx` (see `place_indices`).
    fn uniq_val(&mut self, e: &Expr, idx: &mut std::vec::IntoIter<String>) -> String {
        if let ExprKind::Var(n) = &e.kind {
            let c = self.lookup(n);
            if self.unique_roots.contains(&c) {
                return c;
            }
        }
        if let Some(u) = self.cached(e) {
            for _ in 0..index_count(e) {
                idx.next();
            }
            return u;
        }
        let (l, c) = (e.line + self.fofs, e.col);
        let t = cty(&e.ty);
        let u = self.fresh("u");
        let cp = self.copy_call(&e.ty, &u, l, c);
        // `place` is a C lvalue holding the value (as `ct`).
        let fix = |place: &str, ct: &str| {
            let cast = if ct == t {
                String::new()
            } else {
                format!("({})", t)
            };
            format!("{t} {u} = {cast}{place}; if (__builtin_expect({u}->rc != 1, 0)) {place} = {u} = {cp}; {u};")
        };
        match &e.kind {
            ExprKind::Var(n) => {
                let v = self.var(n);
                format!("({{ {} }})", fix(&v, &t))
            }
            ExprKind::Field(b, f) => {
                let bv = self.uniq_val(b, idx);
                let x = self.fresh("u");
                let place = format!("{x}->f_{f}");
                format!("({{ {} {x} = {bv}; {} }})", cty(&b.ty), fix(&place, &t))
            }
            ExprKind::Index(b, _) => {
                let bv = self.uniq_val(b, idx);
                let k = idx.next().expect("index operand");
                let x = self.fresh("u");
                let place = format!("*{x}");
                format!(
                    "({{ void **{x} = lm_slot_p({bv}, {k}, {l}, {c}); {} }})",
                    fix(&place, "void *")
                )
            }
            _ => unreachable!("checked: not a place"),
        }
    }

    /// C pointer to the slot of a place (its containers made unique) and
    /// the slot's C type: the value's type, or `void *` for a list element
    /// holding a heap value. Index operands come from `idx`.
    fn slot(&mut self, e: &Expr, idx: &mut std::vec::IntoIter<String>) -> (String, String) {
        let (l, c) = (e.line + self.fofs, e.col);
        match &e.kind {
            ExprKind::Var(n) => (format!("&{}", self.lookup(n)), cty(&e.ty)),
            ExprKind::Field(x, f) => {
                let s = self.uniq_val(x, idx);
                (format!("(&{}->f_{})", s, f), cty(&e.ty))
            }
            ExprKind::Index(x, _) => {
                let s = self.uniq_val(x, idx);
                let k = idx.next().expect("index operand");
                let (sx, st) = elem(&x.ty);
                (format!("lm_slot_{}({}, {}, {}, {})", sx, s, k, l, c), st)
            }
            _ => unreachable!("checked: not a place"),
        }
    }

    /// Emit a runtime check of one contract clause.
    fn contract(&mut self, e: &Expr, what: &str) {
        let c = self.scoped(&Ty::Bool, |g| g.ex(e).0);
        let m = c_escape(&format!("{} failed in fn {}", what, self.fname));
        let (l, col) = (e.line + self.fofs, e.col);
        self.line(&format!(
            "if (!({})) lm_panic({}, {}, \"{}\");",
            c, l, col, m
        ));
    }

    /// Check the `ensures` clauses, with `result` bound to the C variable `r`.
    fn ensures(&mut self, r: Option<&str>) {
        if self.ensures.is_empty() {
            return;
        }
        // Clauses see the parameters and `result`, never body locals.
        let mut scope = Scope::default();
        if let Some(r) = r {
            scope.names.insert("result".to_string(), r.to_string());
        }
        let params = Scope {
            names: self.scopes[0].names.clone(),
            types: self.scopes[0].types.clone(),
            owned: Vec::new(),
        };
        let saved = std::mem::replace(&mut self.scopes, vec![params, scope]);
        for e in self.ensures {
            self.contract(e, "ensures");
        }
        self.scopes = saved;
    }

    /// A match; with `target`, each arm's final expression is stored in it.
    fn match_code(&mut self, scrut: &Expr, arms: &[Arm], target: Option<&str>) {
        let st = scrut.ty.clone();
        // The match keeps its own reference to the value (bindings borrow
        // from it), so the arms may change the matched variable.
        let cs = self.scoped(&st, |g| g.own(scrut));
        let m = self.fresh("m");
        self.line(&format!("{{ {} LM_U {} = {};", cty(&st), m, cs));
        self.push_scope();
        self.hide(&m, &st);
        let (l, c) = (scrut.line + self.fofs, scrut.col);
        let vs = variants_of(&st, self.enums);
        for (k, arm) in arms.iter().enumerate() {
            let cond = match &arm.pat {
                Pat::Wild => "1".to_string(),
                Pat::Variant(_, _, i) => format!("{}->tag == {}", m, i),
                Pat::Int(n) if *n == i64::MIN => format!("{} == INT64_MIN", m),
                Pat::Int(n) => format!("{} == INT64_C({})", m, n),
                Pat::Bool(b) => format!("{} == {}", m, b),
                Pat::Str(x) => {
                    let lit = self.str_lit(x);
                    let call = format!("lm_str_eq({}, {}, {}, {})", m, lit, l, c);
                    self.costly("bool", call)
                }
            };
            let kw = if k == 0 { "if" } else { "} else if" };
            self.line(&format!("{} ({}) {{", kw, cond));
            self.ind += 1;
            self.push_scope();
            if let (Pat::Variant(_, bs, i), Some(vs)) = (&arm.pat, &vs) {
                for (j, b) in bs.iter().enumerate() {
                    if b == "_" {
                        continue;
                    }
                    let ft = vs[*i].1[j].1.clone();
                    let cv = self.bind(b, &ft, false);
                    self.line(&format!(
                        "{} LM_U {} = {}->u.v{}.f{};",
                        cty(&ft),
                        cv,
                        m,
                        i,
                        j
                    ));
                }
            }
            self.block(&arm.body, target);
            self.scopes.pop();
            self.ind -= 1;
        }
        self.line("}");
        self.pop_scope();
        self.line("}");
    }

    /// Wrap a call to a builtin that charges steps for O(n) work, so the
    /// function-local step counter is synced around it.
    fn costly(&self, t: &str, call: String) -> String {
        match (self.checked, t) {
            (false, _) => call,
            (_, "void") => format!("LM_CALLV({})", call),
            _ => format!("LM_CALL({}, {})", t, call),
        }
    }

    fn str_lit(&mut self, s: &str) -> String {
        let i = match self.strs.iter().position(|x| x == s) {
            Some(i) => i,
            None => {
                self.strs.push(s.to_string());
                self.strs.len() - 1
            }
        };
        format!("(&lm_s{})", i)
    }
}

impl<'a> Gen<'a> {
    /// An owned (+1) value of `e` (or a scalar / immortal).
    fn own(&mut self, e: &Expr) -> String {
        if !heap(&e.ty) {
            return self.ex(e).0;
        }
        if let ExprKind::Var(n) = &e.kind {
            if let Some((c, t)) = self.movable_var(n) {
                // Last use: move the value out (the variable is dead).
                return format!("({{ {} lm_m_ = {}; {} = 0; lm_m_; }})", cty(&t), c, c);
            }
        }
        match self.ex(e) {
            (c, O::Bor) => format!("LM_DUP({})", c),
            (c, _) => c,
        }
    }

    /// A value of `e` valid until the end of the current statement (an
    /// owned result is kept in a temporary).
    fn borrow(&mut self, e: &Expr) -> String {
        match self.ex(e) {
            (c, O::Own) if heap(&e.ty) => self.temp(&e.ty, c),
            (c, _) => c,
        }
    }

    /// Borrow several operands evaluated together; returns code to run
    /// first (see `seq`) and the operands. When an operand changes a
    /// variable in place (`f(xs, g(&xs))`), the operands are evaluated left
    /// to right into locals, and one that reads such a variable gets its own
    /// reference, so the change can neither free nor alter what it holds.
    fn borrow_all(&mut self, es: &[&Expr]) -> (String, Vec<String>) {
        let muts: Vec<HashSet<String>> = es.iter().map(|e| mut_roots(e)).collect();
        if muts.iter().all(|m| m.is_empty()) {
            return (String::new(), es.iter().map(|e| self.borrow(e)).collect());
        }
        let (mut pre, mut out) = (String::new(), Vec::new());
        for (i, e) in es.iter().enumerate() {
            let names = expr_names(e);
            let clash = heap(&e.ty)
                && muts
                    .iter()
                    .enumerate()
                    .any(|(j, m)| j != i && m.iter().any(|r| names.contains(r)));
            let code = if clash {
                let c = self.own(e);
                self.temp(&e.ty, c)
            } else {
                self.borrow(e)
            };
            let v = self.fresh("a");
            let _ = write!(pre, "{} {} = {}; ", cty(&e.ty), v, code);
            out.push(v);
        }
        (pre, out)
    }

    /// Owned values of several operands (constructor fields). If one of them
    /// may return early (`?`), they are evaluated left to right and kept in
    /// temporaries until the value is built, so an early return releases
    /// the ones already computed.
    fn own_all(&mut self, es: &[&Expr]) -> (String, Vec<String>) {
        let early = es.iter().any(|e| {
            let mut t = false;
            walk(e, false, &mut |x, _| {
                t |= matches!(x.kind, ExprKind::Try(_))
            });
            t
        });
        if !early {
            return (String::new(), es.iter().map(|e| self.own(e)).collect());
        }
        let (mut pre, mut out) = (String::new(), Vec::new());
        for e in es {
            let c = self.own(e);
            let v = self.fresh("a");
            if heap(&e.ty) {
                let t = self.temp(&e.ty, c);
                let _ = write!(pre, "{} {} = {}; ", cty(&e.ty), v, t);
                out.push(format!("LM_DUP({})", v));
            } else {
                let _ = write!(pre, "{} {} = {}; ", cty(&e.ty), v, c);
                out.push(v);
            }
        }
        (pre, out)
    }

    /// Emit `e`: C code and the ownership of its value.
    fn ex(&mut self, e: &Expr) -> (String, O) {
        let (l, c) = (e.line + self.fofs, e.col);
        self.reg(&e.ty);
        let own = |s: String| (s, O::Own);
        let imm = |s: String| (s, O::Imm);
        match &e.kind {
            ExprKind::Variant(_, i, args) => {
                if args.is_empty() {
                    return imm(self.mk_variant(&e.ty, *i, &[], l, c));
                }
                let refs: Vec<&Expr> = args.iter().collect();
                let (pre, ca) = self.own_all(&refs);
                own(seq(&pre, self.mk_variant(&e.ty, *i, &ca, l, c)))
            }
            ExprKind::Match(x, arms) => {
                let r = self.fresh("mv");
                let t = e.ty.clone();
                let body = self.capture(|g| {
                    g.line(&format!("{} {} = 0;", cty(&t), r));
                    g.match_code(x, arms, Some(&r));
                });
                own(format!("({{ {}{}; }})", body, r))
            }
            ExprKind::Try(x) => {
                let cx = self.borrow(x);
                let t = self.fresh("t");
                let rt = self.ret_ty.clone();
                let early = match (&rt, &x.ty) {
                    (Ty::Option(_), _) => self.mk_variant(&rt, 1, &[], l, c),
                    (_, Ty::Result(_, et)) => {
                        let p = format!("{}->u.v1.f0", t);
                        let p = if heap(et) {
                            format!("LM_DUP({})", p)
                        } else {
                            p
                        };
                        self.mk_variant(&rt, 1, &[p], l, c)
                    }
                    _ => unreachable!(),
                };
                let ret = self.capture(|g| g.emit_return(early, &rt));
                (
                    format!(
                        "({{ {} {t} = {cx}; if ({t}->tag != 0) {{ {ret}}} {t}->u.v0.f0; }})",
                        cty(&x.ty)
                    ),
                    O::Bor,
                )
            }
            ExprKind::Int(n) if *n == i64::MIN => imm("INT64_MIN".into()),
            ExprKind::Int(n) => imm(format!("INT64_C({})", n)),
            ExprKind::Float(f) => imm(format!("{:?}", f)),
            ExprKind::Bool(b) => imm(b.to_string()),
            ExprKind::Str(s) => imm(self.str_lit(s)),
            ExprKind::Var(n) => (self.var(n), O::Bor),
            ExprKind::Ref(_) => unreachable!("checked: '&' only in call arguments"),
            ExprKind::Method(..) => unreachable!("lowered by the checker"),
            ExprKind::List(items) => {
                let (sx, st) = elem(&e.ty);
                if items.is_empty() {
                    return own(format!("lm_lit_{}(0, 0, {}, {})", sx, l, c));
                }
                let refs: Vec<&Expr> = items.iter().collect();
                let (pre, vs) = self.own_all(&refs);
                own(seq(
                    &pre,
                    format!(
                        "lm_lit_{}({}, ({}[]){{{}}}, {}, {})",
                        sx,
                        items.len(),
                        st,
                        vs.join(", "),
                        l,
                        c
                    ),
                ))
            }
            ExprKind::StructLit(name, fields) => {
                let decl = &self.structs[self.struct_idx[name.as_str()]];
                let vals: Vec<&Expr> = decl
                    .fields
                    .iter()
                    .map(|(f, _)| &fields.iter().find(|(n, _)| n == f).unwrap().1)
                    .collect();
                let (pre, mut args) = self.own_all(&vals);
                args.push(format!("{}, {}", l, c));
                own(seq(
                    &pre,
                    format!("mk_{}({})", mangle(name), args.join(", ")),
                ))
            }
            ExprKind::Unary(op, x) => {
                let cx = self.ex(x).0;
                imm(match (*op, &x.ty) {
                    ("not", _) => format!("(!{})", cx),
                    (_, Ty::Int) => format!("lm_neg({}, {}, {})", cx, l, c),
                    _ => format!("(-{})", cx),
                })
            }
            ExprKind::Binary("+", a, _) if a.ty == Ty::Str => {
                // Flatten `a + b + c` into one allocation instead of copying
                // the growing prefix once per `+`.
                let parts = concat_parts(e);
                let (pre, cs) = self.borrow_all(&parts);
                let call = format!(
                    "lm_concat({}, (lm_str[]){{{}}}, {}, {})",
                    cs.len(),
                    cs.join(", "),
                    l,
                    c
                );
                own(seq(&pre, self.costly("lm_str", call)))
            }
            ExprKind::Binary(op, a, b) if matches!(*op, "and" | "or") => {
                // Short-circuit: the right operand may not run at all.
                let (ca, cb) = (self.ex(a).0, self.ex(b).0);
                imm(self.binop(op, &a.ty, ca, cb, l, c))
            }
            ExprKind::Binary(op, a, b) => {
                let (pre, cs) = self.borrow_all(&[a, b]);
                let (ca, cb) = (cs[0].clone(), cs[1].clone());
                imm(seq(&pre, self.binop(op, &a.ty, ca, cb, l, c)))
            }
            ExprKind::Range(a, b) => {
                let (ca, cb) = (self.ex(a).0, self.ex(b).0);
                let (r, k, n) = (self.fresh("r"), self.fresh("k"), self.fresh("e"));
                let step = self.step_code(l, c);
                own(format!(
                    "({{ lm_list *{r} = lm_lit_i(0, 0, {l}, {c}); for (int64_t {k} = {ca}, {n} = {cb}; {k} < {n}; {k}++) {{ {step}lm_push_i({r}, {k}, {l}, {c}); }} {r}; }})"
                ))
            }
            ExprKind::Comp(item, clauses) => own(self.comp(e, item, clauses)),
            ExprKind::Index(..) | ExprKind::Field(..) if self.cached(e).is_some() => {
                (self.cached(e).unwrap(), O::Bor)
            }
            ExprKind::Index(x, i) => {
                let (pre, cs) = self.borrow_all(&[x, i]);
                let g = format!(
                    "lm_get_{}({}, {}, {}, {})",
                    elem(&x.ty).0,
                    cs[0],
                    cs[1],
                    l,
                    c
                );
                (seq(&pre, from_slot(g, &e.ty)), O::Bor)
            }
            ExprKind::Field(x, f) => (format!("{}->f_{}", self.borrow(x), f), O::Bor),
            ExprKind::Call(name, args) => self.call(name, args, e),
        }
    }

    /// `ca op cb` where `a` (the left operand) has type `t`.
    fn binop(&mut self, op: &str, t: &Ty, ca: String, cb: String, l: u32, c: u32) -> String {
        match (t, op) {
            (Ty::Str, "+") => {
                let call = format!("lm_concat(2, (lm_str[]){{{}, {}}}, {}, {})", ca, cb, l, c);
                self.costly("lm_str", call)
            }
            (_, "and") => format!("({} && {})", ca, cb),
            (_, "or") => format!("({} || {})", ca, cb),
            (Ty::Int, "+" | "-" | "*" | "/" | "%") => {
                let f = match op {
                    "+" => "add",
                    "-" => "sub",
                    "*" => "mul",
                    "/" => "div",
                    _ => "mod",
                };
                format!("lm_{}({}, {}, {}, {})", f, ca, cb, l, c)
            }
            (Ty::Float, "%") => format!("fmod({}, {})", ca, cb),
            (Ty::Str, "==" | "!=") => {
                let call = format!("lm_str_eq({}, {}, {}, {})", ca, cb, l, c);
                let neg = if op == "!=" { "!" } else { "" };
                format!("({}{})", neg, self.costly("bool", call))
            }
            (Ty::Str, _) => {
                let call = format!("lm_str_cmp({}, {}, {}, {})", ca, cb, l, c);
                format!("({} {} 0)", self.costly("int", call), op)
            }
            _ => format!("({} {} {})", ca, op, cb),
        }
    }

    /// `LM_STEP(l, c); ` in checked mode (positions already offset).
    fn step_code(&self, l: u32, c: u32) -> String {
        if self.checked {
            format!("LM_STEP({}, {}); ", l, c)
        } else {
            String::new()
        }
    }

    /// Open a C loop over an iterable (range or list), binding `v` in the
    /// current scope. Returns the loop head and its closing code. A list
    /// source is held by its own reference for the whole loop.
    fn iter_open(&mut self, v: &str, src: &Expr) -> (String, String) {
        let k = self.fresh("k");
        if let ExprKind::Range(a, b) = &src.kind {
            let ca = self.scoped(&Ty::Int, |g| g.ex(a).0);
            let cb = self.scoped(&Ty::Int, |g| g.ex(b).0);
            let n = self.fresh("e");
            let step = self.step_code(a.line + self.fofs, a.col);
            let cv = self.bind(v, &Ty::Int, false);
            return (
                format!("for (int64_t {k} = {ca}, {n} = {cb}; {k} < {n}; {k}++) {{ {step}int64_t LM_U {cv} = {k}; "),
                "} ".into(),
            );
        }
        let lt = src.ty.clone();
        let ce = self.scoped(&lt, |g| g.own(src));
        let et = elem_ty(&lt);
        let st = elem(&lt).1;
        let x = self.fresh("l");
        self.hide(&x, &lt);
        let step = self.step_code(src.line + self.fofs, src.col);
        let cv = self.bind(v, &et, false);
        let val = from_slot(format!("(({st} *){x}->d)[{k}]"), &et);
        let d = self.drop_code(&lt, &x);
        (
            format!(
                "{{ lm_list *{x} = {ce}; for (int64_t {k} = 0; {k} < {x}->len; {k}++) {{ {step}{} LM_U {cv} = {val}; ",
                cty(&et)
            ),
            format!("}} {} }} ", d),
        )
    }

    /// Loops of a comprehension: opening code, the closing code, and `body`
    /// run inside (with each clause's scope open) to produce the code that
    /// uses each element.
    fn comp_loops(&mut self, clauses: &[Clause], body: impl FnOnce(&mut Self) -> String) -> String {
        let mut open = String::new();
        let mut closes = Vec::new();
        let depth = self.scopes.len();
        for cl in clauses {
            match cl {
                Clause::For(v, src) => {
                    self.push_scope();
                    let (head, close) = self.iter_open(v, src);
                    open.push_str(&head);
                    closes.push(close);
                }
                Clause::If(cond) => {
                    let cc = self.scoped(&Ty::Bool, |g| g.ex(cond).0);
                    open.push_str(&format!("if ((bool){}) {{ ", cc));
                    closes.push("} ".into());
                }
            }
        }
        let inner = body(self);
        // The clause scopes' holds are dropped by the closing code.
        self.scopes.truncate(depth);
        closes.reverse();
        format!("{}{}{}", open, inner, closes.concat())
    }

    /// A list comprehension as a GNU statement expression (owned result).
    fn comp(&mut self, e: &Expr, item: &Expr, clauses: &[Clause]) -> String {
        let (l, c) = (e.line + self.fofs, e.col);
        let (sx, _) = elem(&e.ty);
        let r = self.fresh("c");
        self.push_scope();
        self.hide(&r, &e.ty);
        let loops = self.comp_loops(clauses, |g| {
            let it = item.ty.clone();
            let v = g.scoped(&it, |g| g.own(item));
            format!("lm_push_{sx}({r}, {v}, {l}, {c}); ")
        });
        // The result is handed out, not dropped.
        self.scopes.pop();
        format!("({{ lm_list *{r} = lm_lit_{sx}(0, 0, {l}, {c}); {loops}{r}; }})")
    }

    /// sum / min / max / any / all over a list. A comprehension argument is
    /// fused into the loop, so `sum([f(x) for x in xs])` allocates no list.
    /// (min/max of strings build the list: the result borrows from it.)
    fn reduce(&mut self, name: &str, arg: &Expr, e: &Expr) -> (String, O) {
        let (l, c) = (e.line + self.fofs, e.col);
        let et = elem_ty(&arg.ty);
        let (a, v, t) = (self.fresh("a"), self.fresh("v"), cty(&et));
        let fold = |g: &mut Self, val: String| -> String {
            match name {
                "sum" => {
                    let add = g.binop("+", &et, a.clone(), v.clone(), l, c);
                    format!("{{ {t} {v} = {val}; {a} = {add}; }} ")
                }
                "any" => format!("if ({val}) {a} = true; "),
                "all" => format!("if (!({val})) {a} = false; "),
                _ => {
                    let op = if name == "min" { "<" } else { ">" };
                    let better = g.binop(op, &et, v.clone(), a.clone(), l, c);
                    let h = format!("{}_h", a);
                    format!("{{ {t} {v} = {val}; if (!{h} || {better}) {{ {a} = {v}; {h} = true; }} }} ")
                }
            }
        };
        let loops = match &arg.kind {
            ExprKind::Comp(item, clauses) if !heap(&et) => {
                self.push_scope();
                let s = self.comp_loops(clauses, |g| {
                    let it = item.ty.clone();
                    let val = g.scoped(&it, |g| g.ex(item).0);
                    fold(g, val)
                });
                self.scopes.pop();
                s
            }
            _ => {
                let ca = self.borrow(arg);
                let st = elem(&arg.ty).1;
                let (x, k) = (self.fresh("l"), self.fresh("k"));
                let step = self.step_code(l, c);
                let val = from_slot(format!("(({st} *){x}->d)[{k}]"), &et);
                let body = fold(self, val);
                format!("{{ lm_list *{x} = {ca}; for (int64_t {k} = 0; {k} < {x}->len; {k}++) {{ {step}{body}}} }} ")
            }
        };
        let code = match name {
            "sum" => {
                let zero = if et == Ty::Float { "0.0" } else { "0" };
                format!("({{ {t} {a} = {zero}; {loops}{a}; }})")
            }
            "any" => format!("({{ bool {a} = false; {loops}{a}; }})"),
            "all" => format!("({{ bool {a} = true; {loops}{a}; }})"),
            _ => {
                let msg = c_escape(&format!("{} of empty list", name));
                format!("({{ {t} {a} = 0; bool {a}_h = false; {loops}if (!{a}_h) lm_panic({l}, {c}, \"{msg}\"); {a}; }})")
            }
        };
        (code, if heap(&et) { O::Bor } else { O::Imm })
    }

    /// A call of a user fn: borrowed arguments, `&` arguments as pointers to
    /// their (unique) slots, computed after every other argument.
    fn user_call(&mut self, name: &str, args: &[Expr], e: &Expr) -> (String, O) {
        let wrap = |g: &Self, call: String| match (&e.ty, g.checked) {
            (_, false) => call,
            (Ty::Unit, _) => format!("LM_CALLV({})", call),
            (t, _) => format!("LM_CALL({}, {})", cty(t), call),
        };
        let o = if heap(&e.ty) { O::Own } else { O::Imm };
        if !args.iter().any(|a| matches!(a.kind, ExprKind::Ref(_))) {
            let refs: Vec<&Expr> = args.iter().collect();
            let (pre, ca) = self.borrow_all(&refs);
            let call = format!("fn_{}({})", mangle(name), ca.join(", "));
            return (seq(&pre, wrap(self, call)), o);
        }
        // A `&x` argument changes x itself (as well as anything changed
        // by calls inside its index operands).
        let muts: Vec<HashSet<String>> = args
            .iter()
            .map(|a| {
                let mut m = mut_roots(a);
                if let ExprKind::Ref(x) = &a.kind {
                    m.extend(place_root(x));
                }
                m
            })
            .collect();
        let mut pre = String::new();
        let mut vals = vec![String::new(); args.len()];
        for (i, a) in args.iter().enumerate() {
            if matches!(a.kind, ExprKind::Ref(_)) {
                continue;
            }
            let names = expr_names(a);
            let clash = heap(&a.ty)
                && muts
                    .iter()
                    .enumerate()
                    .any(|(j, m)| j != i && m.iter().any(|r| names.contains(r)));
            let code = if clash {
                let c = self.own(a);
                self.temp(&a.ty, c)
            } else {
                self.borrow(a)
            };
            let v = self.fresh("a");
            let _ = write!(pre, "{} {} = {}; ", cty(&a.ty), v, code);
            vals[i] = v;
        }
        let mut idxs = Vec::new();
        for a in args {
            if let ExprKind::Ref(x) = &a.kind {
                let lines = self.capture(|g| idxs.push(g.place_indices(x)));
                pre.push_str(&lines.replace('\n', " "));
            }
        }
        let mut idxs = idxs.into_iter();
        // A list element holding a heap value is passed through a local
        // (copied in and back out): its slot has C type `void *`.
        let mut back = String::new();
        for (i, a) in args.iter().enumerate() {
            if let ExprKind::Ref(x) = &a.kind {
                let mut it = idxs.next().unwrap().into_iter();
                let (sl, st) = self.slot(x, &mut it);
                let (v, ct) = (self.fresh("s"), cty(&x.ty));
                let _ = write!(pre, "{} *{} = {}; ", st, v, sl);
                if st == ct {
                    vals[i] = v;
                } else {
                    let w = self.fresh("w");
                    let _ = write!(pre, "{ct} {w} = ({ct})*{v}; ");
                    let _ = write!(back, "*{v} = {w}; ");
                    vals[i] = format!("&{}", w);
                }
            }
        }
        let call = wrap(self, format!("fn_{}({})", mangle(name), vals.join(", ")));
        if back.is_empty() {
            return (format!("({{ {}{}; }})", pre, call), o);
        }
        if e.ty == Ty::Unit {
            return (format!("({{ {}{}; {}}})", pre, call, back), o);
        }
        let r = self.fresh("r");
        (
            format!(
                "({{ {}{} {} = {}; {}{}; }})",
                pre,
                cty(&e.ty),
                r,
                call,
                back,
                r
            ),
            o,
        )
    }

    /// push/pop/sort: the list argument is a place, made unique; push's
    /// value is computed first.
    fn mutate(&mut self, name: &str, args: &[Expr], e: &Expr) -> (String, O) {
        let (l, c) = (e.line + self.fofs, e.col);
        let lt = args[0].ty.clone();
        let sx = elem(&lt).0;
        let mut pre = String::new();
        if name == "push" {
            let v = self.own(&args[1]);
            let _ = write!(pre, "{} lm_v_ = {}; ", cty(&elem_ty(&lt)), v);
        }
        let idx = self.capture(|g| {
            let ix = g.place_indices(&args[0]);
            let mut it = ix.into_iter();
            let s = g.uniq_val(&args[0], &mut it);
            g.line(&format!("lm_list *lm_s_ = {};", s));
        });
        pre.push_str(&idx.replace('\n', " "));
        match name {
            "push" => (
                format!("({{ {}lm_push_{}(lm_s_, lm_v_, {}, {}); }})", pre, sx, l, c),
                O::Imm,
            ),
            "pop" => {
                let call = from_slot(format!("lm_pop_{}(lm_s_, {}, {})", sx, l, c), &e.ty);
                let o = if heap(&e.ty) { O::Own } else { O::Imm };
                (format!("({{ {}{}; }})", pre, call), o)
            }
            _ => {
                let call = self.costly("void", format!("lm_sort_{}(lm_s_, {}, {})", sx, l, c));
                (format!("({{ {}{}; }})", pre, call), O::Imm)
            }
        }
    }

    fn call(&mut self, name: &str, args: &[Expr], e: &Expr) -> (String, O) {
        if matches!(name, "sum" | "min" | "max" | "any" | "all") && args.len() == 1 {
            return self.reduce(name, &args[0], e);
        }
        if matches!(name, "push" | "pop" | "sort") {
            return self.mutate(name, args, e);
        }
        if !crate::check::BUILTINS.contains(&name) && !name.starts_with('$') {
            return self.user_call(name, args, e);
        }
        let own = |s: String| (s, O::Own);
        let imm = |s: String| (s, O::Imm);
        let t0 = args.first().map(|a| a.ty.clone()).unwrap_or(Ty::Unit);
        if name == "unwrap_or" {
            let ca = self.borrow(&args[0]);
            let d = self.own(&args[1]);
            let (t, dv) = (self.fresh("t"), self.fresh("d"));
            let rt = e.ty.clone();
            if !heap(&rt) {
                return imm(format!(
                    "({{ {} {t} = {ca}; {} {dv} = {d}; {t}->tag == 0 ? {t}->u.v0.f0 : {dv}; }})",
                    cty(&t0),
                    cty(&rt)
                ));
            }
            let drop = self.drop_code(&rt, &dv);
            return own(format!(
                "({{ {} {t} = {ca}; {} {dv} = {d}; {t}->tag == 0 ? ({{ {drop} LM_DUP({t}->u.v0.f0); }}) : {dv}; }})",
                cty(&t0),
                cty(&rt)
            ));
        }
        let refs: Vec<&Expr> = args.iter().collect();
        let (pre, ca) = self.borrow_all(&refs);
        let (code, o) = self.builtin(name, args, e, &ca);
        (seq(&pre, code), o)
    }

    /// A builtin call on already evaluated operands `ca`.
    fn builtin(&mut self, name: &str, args: &[Expr], e: &Expr, ca: &[String]) -> (String, O) {
        let (l, c) = (e.line + self.fofs, e.col);
        let own = |s: String| (s, O::Own);
        let imm = |s: String| (s, O::Imm);
        let t0 = args.first().map(|a| a.ty.clone()).unwrap_or(Ty::Unit);
        match name {
            "print" => {
                let mut parts = Vec::new();
                for (i, (a, code)) in args.iter().zip(ca).enumerate() {
                    if i > 0 {
                        parts.push(format!("lm_print_sep(' ', {}, {})", l, c));
                    }
                    parts.push(match &a.ty {
                        Ty::Int => format!("lm_print_int({}, {}, {})", code, l, c),
                        Ty::Str => {
                            self.costly("void", format!("lm_print_str({}, {}, {})", code, l, c))
                        }
                        t => {
                            let call = format!(
                                "lm_print_val({}, \"{}\", {}, {})",
                                to_val(code, t),
                                self.desc(t),
                                l,
                                c
                            );
                            self.costly("void", call)
                        }
                    });
                }
                parts.push(format!("lm_print_sep('\\n', {}, {})", l, c));
                imm(format!(
                    "({{ int64_t lm_ps = lm_line.len; {}; lm_print_end(lm_ps); }})",
                    parts.join(", ")
                ))
            }
            "len" if t0 == Ty::Str => {
                imm(self.costly("int64_t", format!("lm_str_len({}, {}, {})", ca[0], l, c)))
            }
            "len" => imm(format!("({})->len", ca[0])),
            "str" => match &t0 {
                Ty::Str => (ca[0].clone(), O::Bor),
                Ty::Int => own(format!("lm_str_int({}, {}, {})", ca[0], l, c)),
                Ty::Float => own(format!("lm_str_float({}, {}, {})", ca[0], l, c)),
                Ty::Bool => own(format!("lm_str_bool({}, {}, {})", ca[0], l, c)),
                t => {
                    let call = format!(
                        "lm_to_str({}, \"{}\", {}, {})",
                        to_val(&ca[0], t),
                        self.desc(t),
                        l,
                        c
                    );
                    own(self.costly("lm_str", call))
                }
            },
            "int" => imm(match &t0 {
                Ty::Int => ca[0].clone(),
                Ty::Float => format!("lm_int_of_float({}, {}, {})", ca[0], l, c),
                _ => format!("lm_int_of_str({}, {}, {})", ca[0], l, c),
            }),
            "float" => imm(match &t0 {
                Ty::Float => ca[0].clone(),
                Ty::Int => format!("((double){})", ca[0]),
                _ => format!("lm_float_of_str({}, {}, {})", ca[0], l, c),
            }),
            "sqrt" => imm(format!("sqrt({})", ca[0])),
            "parse_int" | "parse_float" => {
                let (ct, f) = if name == "parse_int" {
                    ("int64_t", "lm_parse_int")
                } else {
                    ("double", "lm_parse_float")
                };
                let v = self.fresh("v");
                let some = self.mk_variant(&e.ty, 0, std::slice::from_ref(&v), l, c);
                let none = self.mk_variant(&e.ty, 1, &[], l, c);
                own(format!(
                    "({{ {ct} {v} = 0; {f}({}, &{v}) ? {some} : {none}; }})",
                    ca[0]
                ))
            }
            "find" => {
                let call = match &t0 {
                    Ty::Str => format!("lm_str_find({}, {}, {}, {})", ca[0], ca[1], l, c),
                    _ => {
                        let sx = match &t0 {
                            Ty::List(t) if **t == Ty::Str => "s",
                            _ => elem(&t0).0,
                        };
                        format!("lm_find_{}({}, {}, {}, {})", sx, ca[0], ca[1], l, c)
                    }
                };
                let k = self.fresh("k");
                let call = self.costly("int64_t", call);
                let some = self.mk_variant(&e.ty, 0, std::slice::from_ref(&k), l, c);
                let none = self.mk_variant(&e.ty, 1, &[], l, c);
                own(format!(
                    "({{ int64_t {k} = {call}; {k} >= 0 ? {some} : {none}; }})"
                ))
            }
            "get" => {
                let (x, k) = (self.fresh("l"), self.fresh("k"));
                let et = elem_ty(&t0);
                let st = elem(&t0).1;
                let val = from_slot(format!("(({st} *){x}->d)[{k}]"), &et);
                let val = if heap(&et) {
                    format!("LM_DUP({})", val)
                } else {
                    val
                };
                let some = self.mk_variant(&e.ty, 0, &[val], l, c);
                let none = self.mk_variant(&e.ty, 1, &[], l, c);
                own(format!(
                    "({{ lm_list *{x} = {}; int64_t {k} = {}; ({k} >= 0 && {k} < {x}->len) ? {some} : {none}; }})",
                    ca[0], ca[1]
                ))
            }
            "unwrap" => {
                let t = self.fresh("t");
                let fail = match &t0 {
                    Ty::Result(_, et) => format!(
                        "lm_unwrap_err({}, \"{}\", {}, {})",
                        to_val(&format!("{}->u.v1.f0", t), et),
                        self.desc(et),
                        l,
                        c
                    ),
                    _ => format!("lm_panic({}, {}, \"unwrap of None\")", l, c),
                };
                (
                    format!(
                        "({{ {} {t} = {}; if ({t}->tag != 0) {fail}; {t}->u.v0.f0; }})",
                        cty(&t0),
                        ca[0]
                    ),
                    O::Bor,
                )
            }
            "is_some" | "is_ok" => imm(format!("(({})->tag == 0)", ca[0])),
            "is_none" | "is_err" => imm(format!("(({})->tag != 0)", ca[0])),
            "min" | "max" => {
                let (a, b) = (self.fresh("a"), self.fresh("b"));
                let op = if name == "min" { "<" } else { ">" };
                let better = self.binop(op, &t0, b.clone(), a.clone(), l, c);
                let t = cty(&t0);
                (
                    format!(
                        "({{ {t} {a} = {}; {t} {b} = {}; {better} ? {b} : {a}; }})",
                        ca[0], ca[1]
                    ),
                    if heap(&t0) { O::Bor } else { O::Imm },
                )
            }
            "split" => own(self.costly(
                "lm_list *",
                format!("lm_split({}, {}, {}, {})", ca[0], ca[1], l, c),
            )),
            "join" => own(self.costly(
                "lm_str",
                format!("lm_join({}, {}, {}, {})", ca[0], ca[1], l, c),
            )),
            // Capability accesses (lowered methods and `env.<field>`); the
            // receiver is evaluated for its effects, then ignored except for
            // an Fs, whose handle selects its root directory.
            "$Env.fs" | "$Env.vars" | "$Env.clock" | "$Env.args" | "$Env.stdin" => {
                imm(format!("((void)({}), (int64_t)0)", ca[0]))
            }
            "$Fs.read" => own(self.costly(
                "lm_str",
                format!("lm_fs_read({}, {}, {}, {})", ca[0], ca[1], l, c),
            )),
            "$Fs.write" => imm(self.costly(
                "void",
                format!("lm_fs_write({}, {}, {}, {}, {})", ca[0], ca[1], ca[2], l, c),
            )),
            "$Fs.sub" => imm(format!("lm_fs_sub({}, {}, {}, {})", ca[0], ca[1], l, c)),
            "$Stdin.read_line" => own(self.costly(
                "lm_str",
                format!("((void)({}), lm_input_line({}, {}))", ca[0], l, c),
            )),
            "$Vars.get" => own(format!(
                "((void)({}), lm_env({}, {}, {}))",
                ca[0], ca[1], l, c
            )),
            "$Clock.now_ms" => imm(format!("((void)({}), lm_now_ms())", ca[0])),
            "$Args.list" => own(format!("((void)({}), lm_args({}, {}))", ca[0], l, c)),
            "assert" => {
                let msg = ca.get(1).map_or("NULL", |s| s.as_str());
                imm(format!(
                    "((void)(({}) || (lm_assert_fail({}, {}, {}), 0)))",
                    ca[0], l, c, msg
                ))
            }
            _ => unreachable!("builtin {}", name),
        }
    }
}
