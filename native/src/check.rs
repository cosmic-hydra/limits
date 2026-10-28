//! Name resolution and type checking. Collects every error instead of stopping
//! at the first, and annotates `Expr::ty` for codegen.

use crate::ast::*;
use crate::diag::{ranked, Edit, Fix};
use crate::module::Module;
use crate::Error;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

/// Name of the placeholder identifier `limits check --expect` puts at the
/// cursor; the checker records what is expected there instead of failing.
pub const HOLE: &str = "__limits_hole__";

/// What the checker saw at the hole: the expected type (if the context gives
/// one), the receiver type after `.`, and every name in scope.
#[derive(Debug, Clone, Default)]
pub struct Hole {
    pub expected: Option<Ty>,
    pub recv: Option<Ty>,
    /// `X.` where X is an enum name (variant completion).
    pub enum_recv: Option<String>,
    pub vars: Vec<(String, Ty)>,
    pub funcs: Vec<(String, Vec<Ty>, Ty)>,
    pub enums: Vec<(String, Variants)>,
    pub structs: Vec<(String, Vec<(String, Ty)>)>,
    pub seen: bool,
    /// No value fits: the hole is the right operand of an operator that
    /// does not apply to the left operand's type (`xs + ` for a list).
    pub none_fits: bool,
}

pub const BUILTINS: &[&str] = &[
    "print",
    "len",
    "push",
    "pop",
    "str",
    "int",
    "float",
    "assert",
    "sqrt",
    "split",
    "join",
    "sort",
    "sum",
    "min",
    "max",
    "any",
    "all",
    "parse_int",
    "parse_float",
    "find",
    "get",
    "unwrap",
    "unwrap_or",
    "is_some",
    "is_none",
    "is_ok",
    "is_err",
];

/// Constructors of the built-in sum types: name, type, variant index.
const CTORS: &[(&str, &str, usize)] = &[
    ("Some", "Option", 0),
    ("None", "Option", 1),
    ("Ok", "Result", 0),
    ("Err", "Result", 1),
];

/// Type names that cannot be declared.
const TYPE_NAMES: &[&str] = &[
    "int", "float", "bool", "str", "Option", "Result", "Env", "Fs", "Vars", "Clock", "Args",
    "Stdin",
];

/// Capabilities a file may declare with `uses ...`.
pub const CAPS: &[&str] = &["fs.read", "fs.write", "env", "time", "args", "stdin"];

/// Methods of capability values: receiver, name -> the capabilities that
/// allow it (any one of them), parameter types, return type.
pub fn cap_method(cap: Cap, name: &str) -> Option<(&'static [&'static str], Vec<Ty>, Ty)> {
    Some(match (cap, name) {
        (Cap::Fs, "read") => (&["fs.read"], vec![Ty::Str], Ty::Str),
        (Cap::Fs, "write") => (&["fs.write"], vec![Ty::Str, Ty::Str], Ty::Unit),
        (Cap::Fs, "sub") => (&["fs.read", "fs.write"], vec![Ty::Str], Ty::Cap(Cap::Fs)),
        (Cap::Vars, "get") => (&["env"], vec![Ty::Str], Ty::Str),
        (Cap::Clock, "now_ms") => (&["time"], vec![], Ty::Int),
        (Cap::Args, "list") => (&["args"], vec![], Ty::List(Box::new(Ty::Str))),
        (Cap::Stdin, "read_line") => (&["stdin"], vec![], Ty::Str),
        _ => return None,
    })
}

/// Method names of a capability type, for messages.
pub fn cap_methods(cap: Cap) -> &'static [&'static str] {
    match cap {
        Cap::Env => &[],
        Cap::Fs => &["read", "write", "sub"],
        Cap::Vars => &["get"],
        Cap::Clock => &["now_ms"],
        Cap::Args => &["list"],
        Cap::Stdin => &["read_line"],
    }
}

/// I/O builtins of v0.4, replaced by capability methods: name -> the new form.
fn removed_io(name: &str) -> Option<&'static str> {
    Some(match name {
        "read_file" => "env.fs.read(path) (in a fn: take 'fs: Fs' and call fs.read(path))",
        "write_file" => "env.fs.write(path, data) (in a fn: take 'fs: Fs')",
        "env" => "env.vars.get(name) (in a fn: take 'vars: Vars')",
        "now_ms" => "env.clock.now_ms() (in a fn: take 'clock: Clock')",
        "args" => "env.args.list() (in a fn: take 'args: Args')",
        "input_line" => "env.stdin.read_line() (in a fn: take 'stdin: Stdin')",
        _ => return None,
    })
}

/// Struct name -> fields in declaration order.
pub type StructMap = HashMap<String, Vec<(String, Ty)>>;

/// A sum type's variants: name and payload fields, in declaration order.
pub type Variants = Vec<(String, Vec<(String, Ty)>)>;

/// Resolved declarations handed to codegen.
pub struct Types {
    pub structs: StructMap,
    /// User enums by qualified name.
    pub enums: HashMap<String, Variants>,
}

/// Variants of a sum type (`None` if `t` is not one).
pub fn variants_of(t: &Ty, enums: &HashMap<String, Variants>) -> Option<Variants> {
    Some(match t {
        Ty::Option(x) => vec![
            ("Some".into(), vec![("value".into(), (**x).clone())]),
            ("None".into(), vec![]),
        ],
        Ty::Result(x, e) => vec![
            ("Ok".into(), vec![("value".into(), (**x).clone())]),
            ("Err".into(), vec![("error".into(), (**e).clone())]),
        ],
        Ty::Enum(n) => enums.get(n)?.clone(),
        _ => return None,
    })
}

/// `Rect(w, h)`: a pattern for a variant, binding its field names.
fn pat_hint(name: &str, fields: &[(String, Ty)]) -> String {
    if fields.is_empty() {
        return name.to_string();
    }
    let bs: Vec<String> = fields
        .iter()
        .enumerate()
        .map(|(i, (f, _))| {
            if f.starts_with('_') {
                format!("x{}", i)
            } else {
                f.clone()
            }
        })
        .collect();
    format!("{}({})", name, bs.join(", "))
}

/// `Circle(r: float)`: a variant's declaration for messages.
fn variant_sig(name: &str, fields: &[(String, Ty)]) -> String {
    if fields.is_empty() {
        return name.to_string();
    }
    let fs: Vec<String> = fields
        .iter()
        .map(|(f, t)| {
            if f.starts_with('_') {
                t.to_string()
            } else {
                format!("{}: {}", f, t)
            }
        })
        .collect();
    format!("{}({})", name, fs.join(", "))
}

/// Does the block leave by `return`, `break` or `continue` (so it yields no value)?
/// `noun` with its indefinite article: "an int", "a Result".
fn a_an(noun: &str) -> String {
    let vowel = noun.starts_with(|c: char| "aeiouAEIOU".contains(c));
    format!("{} {}", if vowel { "an" } else { "a" }, noun)
}

/// Source position of a statement (`None` for `pass`).
fn stmt_pos(s: &Stmt) -> Option<(u32, u32)> {
    let e = match s {
        Stmt::Let(_, _, _, l, c, _)
        | Stmt::Break(l, c)
        | Stmt::Continue(l, c)
        | Stmt::Return(_, l, c) => return Some((*l, *c)),
        Stmt::Pass => return None,
        Stmt::Assign(e, ..)
        | Stmt::Expr(e)
        | Stmt::While(e, _)
        | Stmt::ForRange(_, e, ..)
        | Stmt::ForList(_, e, _)
        | Stmt::Match(e, _) => e,
        Stmt::If(arms, _) => &arms.first()?.0,
    };
    Some((e.line, e.col))
}

fn diverges(b: &[Stmt]) -> bool {
    returns(b) || matches!(b.last(), Some(Stmt::Break(..) | Stmt::Continue(..)))
}

enum VarRef {
    /// Not a variant reference.
    No,
    /// A variant: enum key and index.
    Yes(String, usize),
    /// Looked like one but was wrong (already reported).
    Bad,
}

pub struct Checker {
    pub structs: HashMap<String, Vec<(String, Ty)>>,
    pub enums: HashMap<String, Variants>,
    pub_enums: HashSet<String>,
    /// Qualified variant name -> (qualified enum, index).
    variants: HashMap<String, (String, usize)>,
    funcs: HashMap<String, (Vec<Ty>, Ty)>,
    /// Parameter names and `inout` flags of every function.
    params: HashMap<String, Vec<(String, bool)>>,
    /// Qualified names of `pub` functions and structs.
    pub_fns: HashSet<String>,
    pub_structs: HashSet<String>,
    /// Per module: qualifier and import aliases.
    names: Vec<String>,
    aliases: Vec<HashMap<String, usize>>,
    files: Vec<String>,
    /// Per module: declared capabilities.
    caps: Vec<HashSet<String>>,
    cur: usize,
    scopes: Vec<HashMap<String, (Ty, Bind)>>,
    ret: Ty,
    loops: usize,
    errs: Vec<Error>,
    /// Alternatives and fixes for the next reported error (taken by err_h).
    alts: RefCell<Vec<String>>,
    fix: Vec<Fix>,
    /// Position of the `let` declaring (name, scope depth), for let -> var.
    let_decl: HashMap<(String, usize), (u32, u32)>,
    /// Per module: where to append to the `uses` line (end of its last
    /// name), if it has one.
    uses_end: Vec<Option<(u32, u32)>>,
    /// Recording for `--expect`, when the program contains a hole.
    pub hole: Option<Hole>,
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
/// `need_main` is false in `limits test` mode, where no entry point is used.
pub fn check(mods: &mut [Module], need_main: bool) -> Result<(Program, Types), Vec<Error>> {
    check_with(mods, need_main, None).0
}

pub type Checked = Result<(Program, Types), Vec<Error>>;

/// `check`, optionally recording what the checker sees at a hole.
pub fn check_with(
    mods: &mut [Module],
    need_main: bool,
    hole: Option<Hole>,
) -> (Checked, Option<Hole>) {
    let mut c = Checker {
        structs: HashMap::new(),
        enums: HashMap::new(),
        pub_enums: HashSet::new(),
        variants: HashMap::new(),
        funcs: HashMap::new(),
        params: HashMap::new(),
        pub_fns: HashSet::new(),
        pub_structs: HashSet::new(),
        names: mods.iter().map(|m| m.name.clone()).collect(),
        aliases: mods.iter().map(|m| m.aliases.clone()).collect(),
        files: mods.iter().map(|m| m.file.clone()).collect(),
        caps: mods
            .iter()
            .map(|m| m.prog.uses.iter().map(|u| u.0.clone()).collect())
            .collect(),
        cur: 0,
        scopes: Vec::new(),
        ret: Ty::Unit,
        loops: 0,
        errs: Vec::new(),
        alts: RefCell::new(Vec::new()),
        fix: Vec::new(),
        let_decl: HashMap::new(),
        uses_end: mods
            .iter()
            .map(|m| {
                m.prog
                    .uses
                    .iter()
                    .max_by_key(|u| (u.1, u.2))
                    .map(|(n, l, c)| (*l, c + n.chars().count() as u32))
            })
            .collect(),
        hole,
    };
    // Script mode: the root file's top-level statements are the body of an
    // implicit `fn main()`; modules may not have any.
    for (i, m) in mods.iter_mut().enumerate() {
        c.cur = i;
        if m.prog.top.is_empty() {
            continue;
        }
        let (l, col) = m.prog.top_pos;
        if i > 0 {
            let h = "move them into a function of this module".into();
            c.err_h(
                l,
                col,
                "a module cannot have top-level statements".into(),
                h,
            );
        } else if let Some(f) = m
            .prog
            .funcs
            .iter()
            .find(|f| f.name == "main" && f.test.is_none())
        {
            let h = "move the top-level statements into main, or delete 'fn main'".into();
            let msg = "'fn main' cannot be combined with top-level statements".into();
            c.err_h(f.line, f.col, msg, h);
        } else {
            // The entry receives the program's capabilities as `env`.
            let body = std::mem::take(&mut m.prog.top);
            m.prog.funcs.push(Func {
                name: "main".into(),
                public: false,
                params: vec![("env".into(), TyExpr::Name("Env".into(), l, col))],
                inout: vec![false],
                ret: None,
                body,
                requires: Vec::new(),
                ensures: Vec::new(),
                test: None,
                file: 0,
                line: l,
                col,
            });
        }
    }
    c.cur = 0;
    // Capabilities: known names only, and a module may use only what the
    // root file declared (the host grants capabilities to the whole program).
    for (i, m) in mods.iter().enumerate() {
        c.cur = i;
        for (u, l, col) in &m.prog.uses {
            if !CAPS.contains(&u.as_str()) {
                let h = format!("capabilities are: {}", CAPS.join(", "));
                *c.alts.borrow_mut() = ranked(u, CAPS.iter().copied());
                c.err_h(*l, *col, format!("unknown capability '{}'", u), h);
            } else if i > 0 && !c.caps[0].contains(u) {
                let m = format!(
                    "capability '{}' is not declared by the root file (import chain: {})",
                    u, m.chain
                );
                let h = format!("add '{}' to the 'uses' line of the root file", u);
                c.err_h(*l, *col, m, h);
            }
        }
    }
    // Pass 1a: declare every struct name in every module.
    for (i, m) in mods.iter().enumerate() {
        c.cur = i;
        for s in &m.prog.structs {
            let q = qualify(&m.name, &s.name);
            if TYPE_NAMES.contains(&s.name.as_str()) {
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
        for en in &m.prog.enums {
            let q = qualify(&m.name, &en.name);
            if TYPE_NAMES.contains(&en.name.as_str()) {
                let m = format!("'{}' is a builtin type name", en.name);
                c.err(en.line, en.col, m);
            }
            if en.public {
                c.pub_enums.insert(q.clone());
            }
            if c.structs.contains_key(&q) || c.enums.insert(q.clone(), Vec::new()).is_some() {
                c.err(en.line, en.col, format!("type '{}' defined twice", en.name));
            }
            for (i, v) in en.variants.iter().enumerate() {
                let vq = qualify(&m.name, &v.name);
                if CTORS.iter().any(|x| x.0 == v.name) || BUILTINS.contains(&v.name.as_str()) {
                    let msg = format!("'{}' is a builtin and cannot be redefined", v.name);
                    c.err(v.line, v.col, msg);
                } else if let Some((other, _)) = c.variants.get(&vq) {
                    let msg = format!(
                        "variant '{}' defined twice (also in enum '{}')",
                        v.name, other
                    );
                    let h = "variant names share one namespace per module; rename one".into();
                    c.err_h(v.line, v.col, msg, h);
                } else if c.structs.contains_key(&vq) || c.enums.contains_key(&vq) {
                    let msg = format!("variant '{}' defined twice (also a type)", v.name);
                    c.err(v.line, v.col, msg);
                } else {
                    c.variants.insert(vq, (q.clone(), i));
                }
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
                if has_cap(&t) {
                    c.cap_stored(&t, "a struct field", s.line, s.col);
                }
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
        for en in &mut m.prog.enums {
            let mut vs = Vec::new();
            for v in &mut en.variants {
                let mut fields: Vec<(String, Ty)> = Vec::new();
                for (f, t) in &mut v.fields {
                    let t = c.resolve(t);
                    if has_cap(&t) {
                        c.cap_stored(&t, "an enum variant", v.line, v.col);
                    }
                    if fields.iter().any(|(n, _)| n == f) {
                        let msg = format!("duplicate field '{}' in variant '{}'", f, v.name);
                        c.err(v.line, v.col, msg);
                    }
                    fields.push((f.clone(), t));
                }
                vs.push((v.name.clone(), fields));
            }
            c.enums.insert(qualify(&m.name, &en.name), vs);
        }
        for f in m.prog.funcs.iter_mut().filter(|f| f.test.is_none()) {
            let ps: Vec<Ty> = f.params.iter_mut().map(|(_, t)| c.resolve(t)).collect();
            let r = f.ret.as_mut().map_or(Ty::Unit, |t| c.resolve(t));
            let q = qualify(&m.name, &f.name);
            let names = f
                .params
                .iter()
                .map(|p| p.0.clone())
                .zip(f.inout.iter().copied());
            c.params.entry(q.clone()).or_insert_with(|| names.collect());
            if f.public {
                c.pub_fns.insert(q.clone());
            }
            if BUILTINS.contains(&f.name.as_str()) || CTORS.iter().any(|x| x.0 == f.name) {
                c.err(
                    f.line,
                    f.col,
                    format!("'{}' is a builtin and cannot be redefined", f.name),
                );
            } else if c.variants.contains_key(&q) {
                let msg = format!("function '{}' defined twice (also an enum variant)", f.name);
                c.err(f.line, f.col, msg);
            } else if c.funcs.insert(q.clone(), (ps, r)).is_some() {
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
        _ if !need_main => {}
        None => c.err_h(
            1,
            1,
            "missing 'fn main()'".into(),
            "write the program as top-level statements, or add 'fn main():'".into(),
        ),
        Some((p, r)) if !(p.is_empty() || *p == [Ty::Cap(Cap::Env)]) || *r != Ty::Unit => {
            let f = mods[0]
                .prog
                .funcs
                .iter()
                .find(|f| f.name == "main")
                .unwrap();
            c.err(
                f.line,
                f.col,
                "'main' must take no parameters or '(env: Env)' and return nothing".into(),
            );
        }
        _ => {}
    }
    // Pass 2: bodies.
    for (i, m) in mods.iter_mut().enumerate() {
        c.cur = i;
        for f in &mut m.prog.funcs {
            let sig = match f.test {
                Some(_) => Some((Vec::new(), Ty::Unit)),
                None => c.funcs.get(&qualify(&m.name, &f.name)).cloned(),
            };
            let Some((ps, r)) = sig else {
                continue; // builtin redefinition, already reported
            };
            c.ret = r.clone();
            c.scopes = vec![HashMap::new()];
            for (((n, _), t), io) in f.params.iter().zip(ps).zip(&f.inout) {
                let b = if *io { Bind::Inout } else { Bind::Param };
                if c.scopes[0].insert(n.clone(), (t, b)).is_some() {
                    c.err(f.line, f.col, format!("duplicate parameter '{}'", n));
                }
            }
            for e in &mut f.requires {
                c.want(e, &Ty::Bool, "requires clause");
            }
            c.scopes.push(HashMap::new());
            if r != Ty::Unit {
                c.scopes[1].insert("result".into(), (r.clone(), Bind::Let));
            }
            for e in &mut f.ensures {
                c.want(e, &Ty::Bool, "ensures clause");
            }
            c.scopes.pop();
            // The body is its own scope, so `var n = n` can shadow a parameter.
            c.block(&mut f.body, true);
            if r != Ty::Unit && !returns(&f.body) {
                c.err(
                    f.line,
                    f.col,
                    format!(
                        "function '{}' may finish without returning {}",
                        f.name,
                        a_an(&r.to_string())
                    ),
                );
            }
        }
    }
    let hole = c.hole.take();
    if !c.errs.is_empty() {
        c.errs
            .sort_by(|a, b| (&a.file, a.line, a.col).cmp(&(&b.file, b.line, b.col)));
        return (Err(c.errs), hole);
    }
    let mut out = Program {
        files: mods.iter().map(|m| m.file.clone()).collect(),
        ..Program::default()
    };
    for (i, m) in mods.iter_mut().enumerate() {
        for mut f in std::mem::take(&mut m.prog.funcs) {
            f.name = qualify(&m.name, &f.name);
            f.test = f.test.map(|t| qualify(&m.name, &t));
            f.file = i;
            out.funcs.push(f);
        }
        for mut s in std::mem::take(&mut m.prog.structs) {
            s.name = qualify(&m.name, &s.name);
            out.structs.push(s);
        }
        for mut en in std::mem::take(&mut m.prog.enums) {
            en.name = qualify(&m.name, &en.name);
            out.enums.push(en);
        }
    }
    (
        Ok((
            out,
            Types {
                structs: c.structs,
                enums: c.enums,
            },
        )),
        hole,
    )
}

/// Does every path through the block end in `return`?
fn returns(b: &[Stmt]) -> bool {
    b.iter().any(|s| match s {
        Stmt::Return(..) => true,
        Stmt::If(arms, Some(els)) => arms.iter().all(|(_, b)| returns(b)) && returns(els),
        // Matches are exhaustive (checked), so every arm returning suffices.
        Stmt::Match(_, arms) => !arms.is_empty() && arms.iter().all(|a| returns(&a.body)),
        _ => false,
    })
}

/// Whether the arms seen so far already match every value of `t`
/// (all enum variants, or both bools), making a later '_' arm unreachable.
fn covers_all(t: &Ty, seen: &[bool], seen_lit: &[String]) -> bool {
    if *t == Ty::Bool {
        return seen_lit.iter().any(|k| k == "true") && seen_lit.iter().any(|k| k == "false");
    }
    !seen.is_empty() && seen.iter().all(|s| *s)
}

pub fn compat(a: &Ty, b: &Ty) -> bool {
    match (a, b) {
        (Ty::Err, _) | (_, Ty::Err) => true,
        (Ty::List(x), Ty::List(y)) | (Ty::Option(x), Ty::Option(y)) => compat(x, y),
        (Ty::Result(x, e), Ty::Result(y, f)) => compat(x, y) && compat(e, f),
        _ => a == b,
    }
}

impl Checker {
    /// Queue the let -> var fix for `n` (declared with `let` in a live scope).
    fn let_fix(&mut self, n: &str) {
        let Some(d) = (1..=self.scopes.len())
            .rev()
            .find(|d| self.scopes[d - 1].contains_key(n))
        else {
            return;
        };
        if let Some(&(l, c)) = self.let_decl.get(&(n.to_string(), d)) {
            self.fix.push(Fix {
                msg: format!("declare '{}' with 'var'", n),
                edits: vec![Edit::replace(l, c, 3, "var".into())],
                machine: true,
            });
        }
    }

    /// Queue a fix adding skeleton arms (`pats`, each with body `pass`) before
    /// the first arm. Machine-applicable for match statements only: a match
    /// value needs real arm values.
    fn arms_fix(&mut self, arms: &[Arm], pats: &[String], stmt: bool) {
        let Some(a) = arms.first() else { return };
        let ind = " ".repeat(a.col.saturating_sub(1) as usize);
        let text: String = pats
            .iter()
            .map(|p| format!("{}{}: pass\n", ind, p))
            .collect();
        self.fix.push(Fix {
            msg: format!("add the missing arm(s): {}", pats.join(", ")),
            edits: vec![Edit::insert(a.line, 1, text)],
            machine: stmt,
        });
    }

    /// Record the scope and expected type at the `--expect` hole.
    fn record_hole(&mut self, hint: Option<&Ty>, recv: Option<Ty>, en: Option<String>) {
        // Scope by scope (outermost first, names sorted), each name once:
        // in the innermost scope that declares it. Linear in the names.
        let mut deepest: HashMap<&str, usize> = HashMap::new();
        for (d, sc) in self.scopes.iter().enumerate() {
            for n in sc.keys() {
                deepest.insert(n, d);
            }
        }
        let mut vars: Vec<(String, Ty)> = Vec::new();
        for (d, sc) in self.scopes.iter().enumerate() {
            let mut names: Vec<(&String, &(Ty, Bind))> = sc
                .iter()
                .filter(|(n, _)| deepest.get(n.as_str()) == Some(&d))
                .collect();
            names.sort_by(|a, b| a.0.cmp(b.0));
            vars.extend(names.into_iter().map(|(n, (t, _))| (n.clone(), t.clone())));
        }
        let pre = qualify(&self.names[self.cur], "");
        let short = |q: &str| -> Option<String> {
            if let Some(k) = q.strip_prefix(pre.as_str()).filter(|k| !k.contains('.')) {
                return Some(k.to_string());
            }
            // An imported module's public item: `alias.item`.
            let (m, item) = q.rsplit_once('.')?;
            let (alias, _) = self.aliases[self.cur]
                .iter()
                .find(|(_, &i)| self.names[i] == m)?;
            Some(format!("{}.{}", alias, item))
        };
        let mut funcs: Vec<(String, Vec<Ty>, Ty)> = self
            .funcs
            .iter()
            .filter(|(q, _)| !q.starts_with(|c: char| c.is_ascii_digit()) && q.as_str() != "main")
            .filter(|(q, _)| q.starts_with(pre.as_str()) || self.pub_fns.contains(*q))
            .filter_map(|(q, (ps, r))| Some((short(q)?, ps.clone(), r.clone())))
            .collect();
        funcs.sort_by(|a, b| a.0.cmp(&b.0));
        let mut enums: Vec<(String, Variants)> = self
            .enums
            .iter()
            .filter_map(|(q, v)| Some((short(q)?, v.clone())))
            .collect();
        enums.sort_by(|a, b| a.0.cmp(&b.0));
        let mut structs: Vec<(String, Vec<(String, Ty)>)> = self
            .structs
            .iter()
            .filter_map(|(q, v)| Some((short(q)?, v.clone())))
            .collect();
        structs.sort_by(|a, b| a.0.cmp(&b.0));
        let enum_recv = en.and_then(|a| {
            let q = qualify(&self.names[self.cur], &a);
            self.enums.contains_key(&q).then_some(a)
        });
        self.hole = Some(Hole {
            expected: hint.cloned().filter(|t| *t != Ty::Err),
            recv: recv.filter(|t| *t != Ty::Err),
            enum_recv,
            vars,
            funcs,
            enums,
            structs,
            seen: true,
            none_fits: false,
        });
    }

    fn err(&mut self, l: u32, c: u32, m: String) {
        self.err_h(l, c, m, String::new());
    }

    /// Report a capability type used where values are stored.
    fn cap_stored(&mut self, t: &Ty, place: &str, l: u32, c: u32) {
        let m = format!("a capability ({}) cannot be stored in {}", t, place);
        let h = "pass capabilities as fn parameters instead (e.g. 'fn load(fs: Fs)')".into();
        self.err_h(l, c, m, h);
    }

    fn err_h(&mut self, l: u32, c: u32, m: String, hint: String) {
        let mut e = Error::new("check", l, c, m);
        e.file = self.files[self.cur].clone();
        e.hint = hint;
        e.more.alts = self.alts.take();
        e.more.fixes = std::mem::take(&mut self.fix);
        self.errs.push(e);
    }

    /// "did you mean" over the functions (or structs) of module `m`.
    fn suggest_item(&self, n: &str, fns: bool, m: usize) -> String {
        let pre = qualify(&self.names[m], "");
        let keys: Vec<&String> = if fns {
            self.funcs.keys().chain(self.variants.keys()).collect()
        } else {
            self.structs.keys().chain(self.enums.keys()).collect()
        };
        let local = keys
            .into_iter()
            .filter_map(|k| k.strip_prefix(pre.as_str()))
            .filter(|k| !k.contains('.'));
        let extra = if fns && m == self.cur { BUILTINS } else { &[] };
        let all: Vec<&str> = local.chain(extra.iter().copied()).collect();
        *self.alts.borrow_mut() = ranked(n, all.iter().copied());
        crate::diag::suggest(n, all.into_iter())
    }

    /// Resolve a function (`fns`) or struct name as written in the current
    /// module to its qualified name, reporting unknown or private items.
    fn item(&mut self, n: &str, fns: bool, l: u32, c: u32) -> Option<String> {
        let mut what = if fns { "function" } else { "struct" };
        let Some((alias, item)) = n.split_once('.') else {
            let q = qualify(&self.names[self.cur], n);
            let known = if fns {
                self.funcs.contains_key(&q)
            } else {
                self.structs.contains_key(&q) || self.enums.contains_key(&q)
            };
            if known {
                return Some(q);
            }
            if let Some(t) = self.lookup(n).filter(|_| fns).cloned() {
                let m = format!("'{}' is a variable of type {}, not a function", n, t);
                let h = "only functions can be called; use the variable without '(...)', or rename it if a function was meant".into();
                self.err_h(l, c, m, h);
                return None;
            }
            let m = if fns {
                format!("undefined function '{}'", n)
            } else {
                format!("unknown struct '{}'", n)
            };
            let h = match removed_io(n).filter(|_| fns) {
                Some(new) => format!("I/O goes through capability values: use {}", new),
                None => self.suggest_item(n, fns, self.cur),
            };
            self.err_h(l, c, m, h);
            return None;
        };
        let Some(&m) = self.aliases[self.cur].get(alias) else {
            self.err(
                l,
                c,
                format!("unknown module '{}' (missing 'import {}'?)", alias, alias),
            );
            return None;
        };
        let q = qualify(&self.names[m], item);
        let (known, public) = if fns {
            (self.funcs.contains_key(&q), self.pub_fns.contains(&q))
        } else if self.enums.contains_key(&q) {
            (true, self.pub_enums.contains(&q))
        } else {
            (self.structs.contains_key(&q), self.pub_structs.contains(&q))
        };
        if !known {
            let h = self.suggest_item(item, fns, m);
            let msg = format!("module '{}' has no {} '{}'", alias, what, item);
            self.err_h(l, c, msg, h);
            return None;
        }
        if !fns && self.enums.contains_key(&q) {
            what = "enum";
        }
        if !public {
            let msg = format!("{} '{}' in module '{}' is private", what, item, alias);
            let kw = if fns {
                "fn"
            } else if self.enums.contains_key(&q) {
                "enum"
            } else {
                "struct"
            };
            let h = format!("add 'pub' to {} {} in {}", kw, item, self.files[m]);
            self.err_h(l, c, msg, h);
            return None;
        }
        Some(q)
    }

    /// Resolve a written type, rewriting struct names to qualified names.
    fn resolve(&mut self, t: &mut TyExpr) -> Ty {
        match t {
            TyExpr::List(t) => {
                let x = self.resolve(t);
                if let (Ty::Cap(_), TyExpr::Name(_, l, c)) = (&x, &**t) {
                    let (l, c) = (*l, *c);
                    self.cap_stored(&x, "a list", l, c);
                    return Ty::Err;
                }
                Ty::List(Box::new(x))
            }
            TyExpr::Generic(n, args, l, c) => {
                let (l, c) = (*l, *c);
                let want = match n.as_str() {
                    "Option" => 1,
                    "Result" => 2,
                    _ => {
                        for a in args.iter_mut() {
                            self.resolve(a);
                        }
                        let m = format!("'{}' is not generic", n);
                        let h = "only Option[T] and Result[T, E] take type arguments".into();
                        self.err_h(l, c, m, h);
                        return Ty::Err;
                    }
                };
                let ts: Vec<Ty> = args.iter_mut().map(|a| self.resolve(a)).collect();
                if let Some(t) = ts.iter().find(|t| matches!(t, Ty::Cap(_))) {
                    let t = t.clone();
                    self.cap_stored(&t, &a_an(n), l, c);
                    return Ty::Err;
                }
                if ts.len() != want {
                    let m = format!("'{}' takes {} type argument(s), got {}", n, want, ts.len());
                    let h = if want == 1 {
                        "write Option[T], e.g. Option[int]"
                    } else {
                        "write Result[T, E], e.g. Result[int, str]"
                    };
                    self.err_h(l, c, m, h.into());
                    return Ty::Err;
                }
                let mut it = ts.into_iter();
                let a = Box::new(it.next().unwrap());
                match it.next() {
                    Some(e) => Ty::Result(a, Box::new(e)),
                    None => Ty::Option(a),
                }
            }
            TyExpr::Name(n, l, c) => match n.as_str() {
                "int" => Ty::Int,
                "float" => Ty::Float,
                "bool" => Ty::Bool,
                "str" => Ty::Str,
                "Env" => Ty::Cap(Cap::Env),
                "Fs" => Ty::Cap(Cap::Fs),
                "Vars" => Ty::Cap(Cap::Vars),
                "Clock" => Ty::Cap(Cap::Clock),
                "Args" => Ty::Cap(Cap::Args),
                "Stdin" => Ty::Cap(Cap::Stdin),
                "Option" | "Result" => {
                    let m = format!("'{}' needs type arguments", n);
                    let h = if n == "Option" {
                        "write Option[T], e.g. Option[int]"
                    } else {
                        "write Result[T, E], e.g. Result[int, str]"
                    };
                    self.err_h(*l, *c, m, h.into());
                    Ty::Err
                }
                _ if !n.contains('.') && {
                    let q = qualify(&self.names[self.cur], n);
                    !self.structs.contains_key(&q) && !self.enums.contains_key(&q)
                } =>
                {
                    let h = self.suggest_item(n, false, self.cur);
                    self.err_h(*l, *c, format!("unknown type '{}'", n), h);
                    Ty::Err
                }
                _ => match self.item(&n.clone(), false, *l, *c) {
                    Some(q) => {
                        *n = q.clone();
                        if self.enums.contains_key(&q) {
                            Ty::Enum(q)
                        } else {
                            Ty::Struct(q)
                        }
                    }
                    None => Ty::Err,
                },
            },
        }
    }

    fn suggest_var(&self, n: &str) -> String {
        let names = self
            .scopes
            .iter()
            .flat_map(|s| s.keys().map(String::as_str));
        let all: Vec<&str> = names.filter(|x| *x != HOLE).collect();
        *self.alts.borrow_mut() = ranked(n, all.iter().copied());
        crate::diag::suggest(n, all.into_iter())
    }

    fn lookup(&self, n: &str) -> Option<&Ty> {
        self.binding(n).map(|b| &b.0)
    }

    fn binding(&self, n: &str) -> Option<&(Ty, Bind)> {
        self.scopes.iter().rev().find_map(|s| s.get(n))
    }

    /// Type of `a op b` for arithmetic and comparison operators (`what` is
    /// the operator as written, e.g. `+=`), or an error.
    fn binop(&mut self, op: &str, what: &str, ta: Ty, tb: Ty, l: u32, c: u32) -> Ty {
        let arith = matches!(op, "+" | "-" | "*" | "/" | "%");
        if ta == Ty::Err || tb == Ty::Err {
            return if arith { Ty::Err } else { Ty::Bool };
        }
        if !binop_ok(op, &ta, &tb) {
            let sum = |t: &Ty| matches!(t, Ty::Option(_) | Ty::Result(..) | Ty::Enum(_));
            if sum(&ta) || sum(&tb) {
                let m = format!("cannot apply '{}' to {} and {}", what, ta, tb);
                let h = "sum types cannot be compared or combined with operators; use match, or is_some/is_none/is_ok/is_err".into();
                self.err_h(l, c, m, h);
                return Ty::Err;
            }
            let hint = if matches!((&ta, &tb), (Ty::Int, Ty::Float) | (Ty::Float, Ty::Int)) {
                " (use float(x) or int(x) to convert)"
            } else {
                ""
            };
            self.err(
                l,
                c,
                format!("cannot apply '{}' to {} and {}{}", what, ta, tb, hint),
            );
            return Ty::Err;
        }
        if arith {
            ta
        } else {
            Ty::Bool
        }
    }

    /// Element type of an iterable (`for` / comprehension source).
    fn iter_elem(&mut self, e: &mut Expr) -> Ty {
        if let ExprKind::Range(a, b) = &mut e.kind {
            self.want(a, &Ty::Int, "range start");
            self.want(b, &Ty::Int, "range end");
            e.ty = Ty::List(Box::new(Ty::Int));
            return Ty::Int;
        }
        match self.expr(e, None) {
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
        }
    }

    fn block(&mut self, b: &mut [Stmt], scope: bool) {
        if scope {
            self.scopes.push(HashMap::new());
        }
        let n = b.len();
        for (k, s) in b.iter_mut().enumerate() {
            self.stmt(s);
            let one = std::slice::from_ref(s);
            // A break/continue outside a loop is already an error; keep checking.
            if k + 1 < n && (returns(one) || self.loops > 0 && diverges(one)) {
                // Later statements never run; checking them would only give
                // misleading errors (e.g. a trailing expression read as the
                // implicit return value).
                if let Some((l, c)) = stmt_pos(&b[k + 1]) {
                    let m = "unreachable code: the statement before it always leaves (return, break or continue)".into();
                    self.err_h(l, c, m, "remove the unreachable statements".into());
                }
                break;
            }
        }
        if scope {
            self.scopes.pop();
        }
    }

    fn scoped_loop(&mut self, var: Option<(&str, Ty)>, b: &mut [Stmt]) {
        self.scopes.push(HashMap::new());
        if let Some((v, t)) = var {
            self.scopes
                .last_mut()
                .unwrap()
                .insert(v.to_string(), (t, Bind::Loop));
        }
        self.loops += 1;
        self.block(b, true);
        self.loops -= 1;
        self.scopes.pop();
    }

    fn want(&mut self, e: &mut Expr, t: &Ty, what: &str) {
        let got = self.expr(e, Some(t));
        if !compat(&got, t) {
            let (l, c) = expr_start(e);
            self.err(l, c, format!("{} must be {}, got {}", what, t, got));
        }
    }

    fn stmt(&mut self, s: &mut Stmt) {
        let is_break = matches!(s, Stmt::Break(..));
        match s {
            Stmt::Pass => {}
            Stmt::Let(name, ann, e, l, c, mutable) => {
                if !*mutable {
                    let d = self.scopes.len();
                    self.let_decl.insert((name.clone(), d), (*l, *c));
                }
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
                    .insert(
                        name.clone(),
                        (t, if *mutable { Bind::Var } else { Bind::Let }),
                    )
                    .is_some()
                {
                    self.err(
                        *l,
                        *c,
                        format!("'{}' is already declared in this block", name),
                    );
                }
            }
            Stmt::Assign(target, op, v) => {
                if let ExprKind::Var(n) = &target.kind {
                    match self.binding(n).map(|b| b.1) {
                        None => {
                            let h = match self.suggest_var(n) {
                                h if h.is_empty() => format!("declare it first: var {} = ...", n),
                                h => h,
                            };
                            // Renaming to a name declared with `let` would only
                            // lead to a second fix (let -> var) that silently
                            // makes this assign a different variable: offer the
                            // rename for review, never as a machine fix.
                            let good = h
                                .split("did you mean '")
                                .nth(1)
                                .and_then(|r| r.split('\'').next())
                                .map(str::to_string);
                            if let Some(g) = good {
                                let mutable =
                                    matches!(self.binding(&g), Some((_, Bind::Var | Bind::Inout)));
                                if !mutable {
                                    self.fix.push(Fix {
                                        msg: format!("rename '{}' to '{}'", n, g),
                                        edits: vec![Edit::replace(
                                            target.line,
                                            target.col,
                                            n.chars().count() as u32,
                                            g,
                                        )],
                                        machine: false,
                                    });
                                }
                            }
                            let m = format!("assignment to undeclared variable '{}'", n);
                            self.err_h(target.line, target.col, m, h);
                            self.expr(v, None);
                            return;
                        }
                        Some(Bind::Var | Bind::Inout) => {}
                        Some(b) => {
                            let (m, h) = match b {
                                Bind::Param => (
                                    format!("cannot assign to parameter '{}'", n),
                                    format!(
                                        "parameters are read-only; copy it first: var {0} = {0} (or declare '{0}: inout T' and pass '&{0}' to change the caller's variable)",
                                        n
                                    ),
                                ),
                                Bind::Loop => (
                                    format!("cannot assign to loop variable '{}'", n),
                                    format!("copy it first: var {0}2 = {0}", n),
                                ),
                                _ => (
                                    {
                                        let n = n.clone();
                                        self.let_fix(&n);
                                        format!("cannot assign to '{}': it is declared with 'let'", n)
                                    },
                                    format!(
                                        "declare it with 'var {} = ...' to allow reassignment",
                                        n
                                    ),
                                ),
                            };
                            self.err_h(target.line, target.col, m, h);
                        }
                    }
                }
                let t = self.expr(target, None);
                if !matches!(target.kind, ExprKind::Var(_)) {
                    self.check_mut(target);
                }
                match op {
                    None => self.want(v, &t, "assigned value"),
                    Some((op, l, c)) => {
                        let tv = self.expr(v, Some(&t));
                        let (op, l, c) = (*op, *l, *c);
                        let r = self.binop(op, &format!("{}=", op), t.clone(), tv, l, c);
                        if !compat(&r, &t) {
                            let m = format!("result of '{}=' must be {}, got {}", op, t, r);
                            self.err(l, c, m);
                        }
                    }
                }
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
                let t = self.iter_elem(e);
                self.scoped_loop(Some((v, t)), body);
            }
            Stmt::Break(l, c) | Stmt::Continue(l, c) => {
                if self.loops == 0 {
                    let w = if is_break { "break" } else { "continue" };
                    self.err(*l, *c, format!("'{}' outside of a loop", w));
                }
            }
            Stmt::Match(e, arms) => {
                let (l, c) = (e.line, e.col);
                self.check_match(e, arms, None, l, c);
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
        if self.hole.as_ref().is_some_and(|h| !h.seen) {
            let rec = match &mut e.kind {
                ExprKind::Var(n) if n == HOLE => Some((None, None)),
                ExprKind::Field(x, f) if f == HOLE => match &x.kind {
                    ExprKind::Var(a) if self.lookup(a).is_none() => Some((None, Some(a.clone()))),
                    _ => Some((Some(self.expr(x, None)), None)),
                },
                _ => None,
            };
            if let Some((recv, en)) = rec {
                self.record_hole(hint, recv, en);
                e.ty = Ty::Err;
                return Ty::Err;
            }
        }
        if let ExprKind::Call(n, args) = &mut e.kind {
            // `x.m(...)` where `x` is a local: a method call on its value.
            if let Some((v, m)) = n.split_once('.') {
                if self.lookup(v).is_some() {
                    let recv = Expr {
                        kind: ExprKind::Var(v.to_string()),
                        line: e.line,
                        col: e.col,
                        ty: Ty::Err,
                    };
                    e.kind = ExprKind::Method(Box::new(recv), m.to_string(), std::mem::take(args));
                }
            }
        }
        let mut t = if matches!(e.kind, ExprKind::Method(..)) {
            self.method(e)
        } else {
            self.expr_inner(e, hint)
        };
        match &mut e.kind {
            // `env.fs`: lowered to an internal builtin on the receiver.
            ExprKind::Field(x, f) if matches!(x.ty, Ty::Cap(_)) && t != Ty::Err => {
                let x = std::mem::replace(
                    &mut **x,
                    Expr {
                        kind: ExprKind::Bool(false),
                        line: 0,
                        col: 0,
                        ty: Ty::Err,
                    },
                );
                let n = format!("${}.{}", x.ty, f);
                e.kind = ExprKind::Call(n, vec![x]);
            }
            ExprKind::List(_) | ExprKind::Comp(..) | ExprKind::Variant(..) if has_cap(&t) => {
                let what = match &t {
                    Ty::List(_) => "a list".to_string(),
                    t => a_an(t.to_string().split('[').next().unwrap_or("")),
                };
                let x = match &t {
                    Ty::List(x) | Ty::Option(x) => (**x).clone(),
                    Ty::Result(a, _) if has_cap(a) => (**a).clone(),
                    Ty::Result(_, b) => (**b).clone(),
                    t => t.clone(),
                };
                self.cap_stored(&x, &what, e.line, e.col);
                t = Ty::Err;
            }
            _ => {}
        }
        e.ty = t.clone();
        t
    }

    /// `recv.m(args)` on a capability: checks it and lowers it to a call of
    /// the internal builtin `$Cap.m` with the receiver as first argument.
    fn method(&mut self, e: &mut Expr) -> Ty {
        let (l, c) = (e.line, e.col);
        let ExprKind::Method(recv, m, args) = &mut e.kind else {
            unreachable!()
        };
        let t = self.expr(recv, None);
        let cap = match t {
            Ty::Cap(cap) => cap,
            t => {
                if t != Ty::Err {
                    let msg = format!("{} has no method '{}'", t, m);
                    let h = "only capabilities (Fs, Vars, Clock, Args, Stdin) have methods; call functions as f(x)".into();
                    self.err_h(l, c, msg, h);
                }
                for a in args.iter_mut() {
                    self.expr(a, None);
                }
                return Ty::Err;
            }
        };
        let Some((need, ps, r)) = cap_method(cap, m) else {
            let msg = format!("{} has no method '{}'", cap.name(), m);
            let h = if cap == Cap::Env {
                "Env has fields, not methods: env.fs, env.vars, env.clock, env.args, env.stdin"
                    .to_string()
            } else {
                *self.alts.borrow_mut() = ranked(m, cap_methods(cap).iter().copied());
                match crate::diag::suggest(m, cap_methods(cap).iter().copied()) {
                    h if h.is_empty() => format!(
                        "{} methods are: {}",
                        cap.name(),
                        cap_methods(cap).join(", ")
                    ),
                    h => h,
                }
            };
            self.err_h(l, c, msg, h);
            for a in args.iter_mut() {
                self.expr(a, None);
            }
            return Ty::Err;
        };
        let name = format!("{}.{}", cap.name(), m);
        if !need.iter().any(|n| self.caps[self.cur].contains(*n)) {
            let msg = format!("'{}' needs capability '{}'", name, need.join("' or '"));
            let h = format!("add 'uses {}' at the top of this file", need[0]);
            let edit = match self.uses_end[self.cur] {
                Some((ul, uc)) => Edit::insert(ul, uc, format!(", {}", need[0])),
                None => Edit::insert(1, 1, format!("uses {}\n", need[0])),
            };
            self.fix.push(Fix {
                msg: format!("add 'uses {}'", need[0]),
                edits: vec![edit],
                machine: true,
            });
            *self.alts.borrow_mut() = need.iter().map(|x| x.to_string()).collect();
            self.err_h(l, c, msg, h);
        }
        if args.len() != ps.len() {
            let msg = format!(
                "'{}' takes {} argument(s), got {}",
                name,
                ps.len(),
                args.len()
            );
            self.err(l, c, msg);
        }
        for (i, a) in args.iter_mut().enumerate() {
            match ps.get(i) {
                Some(p) => self.want(a, p, &format!("argument {} of '{}'", i + 1, name)),
                None => {
                    self.expr(a, None);
                }
            }
        }
        let recv = std::mem::replace(
            &mut **recv,
            Expr {
                kind: ExprKind::Bool(false),
                line: 0,
                col: 0,
                ty: Ty::Err,
            },
        );
        let mut all = vec![recv];
        all.append(args);
        e.kind = ExprKind::Call(format!("${}", name), all);
        r
    }

    /// Is `n` (as written: `Circle`, `Shape.Circle`, `m.Circle`, `Some`) a
    /// variant? Reports a wrong variant of a known enum, or a private one.
    fn variant_ref(&mut self, n: &str, l: u32, c: u32) -> VarRef {
        if let Some(&(_, ty, i)) = CTORS.iter().find(|x| x.0 == n) {
            return VarRef::Yes(ty.into(), i);
        }
        let Some((a, v)) = n.split_once('.') else {
            return match self.variants.get(&qualify(&self.names[self.cur], n)) {
                Some((e, i)) => VarRef::Yes(e.clone(), *i),
                None => VarRef::No,
            };
        };
        let eq = qualify(&self.names[self.cur], a);
        if let Some(vs) = self.enums.get(&eq) {
            if let Some(i) = vs.iter().position(|x| x.0 == v) {
                return VarRef::Yes(eq, i);
            }
            let h = crate::diag::suggest(v, vs.iter().map(|x| x.0.as_str()));
            *self.alts.borrow_mut() = ranked(v, vs.iter().map(|x| x.0.as_str()));
            self.err_h(l, c, format!("enum '{}' has no variant '{}'", a, v), h);
            return VarRef::Bad;
        }
        let Some(&m) = self.aliases[self.cur].get(a) else {
            return VarRef::No;
        };
        let Some((e, i)) = self.variants.get(&qualify(&self.names[m], v)).cloned() else {
            return VarRef::No;
        };
        if !self.pub_enums.contains(&e) {
            let en = e.rsplit('.').next().unwrap_or(&e).to_string();
            let msg = format!("enum '{}' in module '{}' is private", en, a);
            let h = format!("add 'pub' to enum {} in {}", en, self.files[m]);
            self.err_h(l, c, msg, h);
            return VarRef::Bad;
        }
        VarRef::Yes(e, i)
    }

    /// Type of a variant value `en.variants[i](args)`. `Option`/`Result`
    /// take their type arguments from the expected type when there is one.
    fn variant(
        &mut self,
        en: &str,
        i: usize,
        args: &mut [Expr],
        hint: Option<&Ty>,
        l: u32,
        c: u32,
    ) -> Ty {
        let fields: Vec<(String, Ty)> = match (en, hint) {
            ("Option", Some(Ty::Option(t))) => vec![("value".into(), (**t).clone())],
            ("Result", Some(Ty::Result(t, _))) if i == 0 => vec![("value".into(), (**t).clone())],
            ("Result", Some(Ty::Result(_, e))) => vec![("error".into(), (**e).clone())],
            ("Option" | "Result", _) => vec![("value".into(), Ty::Err)],
            _ => self.enums[en][i].1.clone(),
        };
        let vname = match en {
            "Option" | "Result" => CTORS.iter().find(|x| x.1 == en && x.2 == i).unwrap().0,
            _ => self.enums[en][i].0.as_str(),
        }
        .to_string();
        let nf = if en == "Option" && i == 1 {
            0
        } else {
            fields.len()
        };
        if args.len() != nf {
            let m = format!("'{}' takes {} argument(s), got {}", vname, nf, args.len());
            let sig = if en == "Option" || en == "Result" {
                if nf == 0 {
                    vname.clone()
                } else {
                    format!("{}(value)", vname)
                }
            } else {
                variant_sig(&vname, &fields)
            };
            self.err_h(l, c, m, format!("declared as {}", sig));
            for a in args.iter_mut() {
                self.expr(a, None);
            }
            return Ty::Err;
        }
        let mut tys = Vec::new();
        for (a, (_, t)) in args.iter_mut().zip(&fields) {
            let t = if *t == Ty::Err {
                self.expr(a, None)
            } else {
                self.want(a, t, &format!("value of '{}'", vname));
                t.clone()
            };
            if t == Ty::Unit {
                self.err(a.line, a.col, "cannot bind a unit value".into());
                return Ty::Err;
            }
            tys.push(t);
        }
        match (en, hint) {
            ("Option", Some(h @ Ty::Option(_))) | ("Result", Some(h @ Ty::Result(..))) => h.clone(),
            ("Option", _) if i == 0 => Ty::Option(Box::new(tys.pop().unwrap())),
            // An annotation that failed to resolve was already reported.
            ("Option" | "Result", Some(Ty::Err)) => Ty::Err,
            ("Option" | "Result", _) => {
                let (m, h) = if en == "Option" {
                    (
                        "cannot infer the type of 'None'".to_string(),
                        "add a type annotation, e.g. 'var x: Option[int] = None'",
                    )
                } else {
                    (
                        format!("cannot infer the type of '{}(...)': Result needs both its value and error types", vname),
                        "add a type annotation, e.g. 'let r: Result[int, str] = ...', or return it from a fn declared '-> Result[int, str]'",
                    )
                };
                self.err_h(l, c, m, h.into());
                Ty::Err
            }
            _ => Ty::Enum(en.to_string()),
        }
    }

    /// Check a match statement (`value` is None) or a match value (`value`
    /// holds the expected type, if any). Reports non-exhaustive matches.
    fn check_match(
        &mut self,
        scrut: &mut Expr,
        arms: &mut [Arm],
        value: Option<Option<&Ty>>,
        l: u32,
        c: u32,
    ) -> Ty {
        let t = self.expr(scrut, None);
        let vs = variants_of(&t, &self.enums);
        let lit = matches!(t, Ty::Int | Ty::Str | Ty::Bool);
        if vs.is_none() && !lit && t != Ty::Err {
            let m = format!("cannot match on {}", t);
            let h = "match works on enums, Option, Result, int, str and bool".into();
            self.err_h(scrut.line, scrut.col, m, h);
        }
        let nv = vs.as_ref().map_or(0, |v| v.len());
        let mut seen = vec![false; nv];
        let mut seen_lit: Vec<String> = Vec::new();
        let mut wild = false;
        let mut out: Option<Ty> = value.flatten().cloned();
        let mut all_leave = !arms.is_empty();
        for arm in arms.iter_mut() {
            let (al, ac) = (arm.line, arm.col);
            if !wild && matches!(arm.pat, Pat::Wild) && covers_all(&t, &seen, &seen_lit) {
                let m = format!(
                    "unreachable match arm: the earlier arms already cover every {}",
                    t
                );
                self.err_h(al, ac, m, "remove the '_' arm".into());
            }
            if wild {
                let m = "unreachable match arm: an earlier '_' arm matches everything".into();
                self.err_h(al, ac, m, "move the '_' arm last".into());
            }
            let mut binds: Vec<(String, Ty)> = Vec::new();
            match &mut arm.pat {
                Pat::Wild => wild = true,
                Pat::Variant(name, bs, idx) => {
                    let short = name.rsplit('.').next().unwrap_or(name).to_string();
                    let mut fields = None;
                    match &vs {
                        None if t == Ty::Err => {}
                        None => {
                            let m = format!("pattern '{}' needs an enum, Option or Result value, but the matched value is {}", short, t);
                            let h = if lit {
                                format!(
                                    "match {} with literal patterns and a final '_' arm",
                                    a_an(&t.to_string())
                                )
                            } else {
                                String::new()
                            };
                            self.err_h(al, ac, m, h);
                        }
                        Some(vs) => match vs.iter().position(|v| v.0 == short) {
                            None => {
                                let names: Vec<&str> = vs.iter().map(|v| v.0.as_str()).collect();
                                let m = format!("{} has no variant '{}'", t, short);
                                *self.alts.borrow_mut() = ranked(&short, names.iter().copied());
                                let h = match crate::diag::suggest(&short, names.iter().copied()) {
                                    h if h.is_empty() => {
                                        format!("its variants are: {}", names.join(", "))
                                    }
                                    h => h,
                                };
                                self.err_h(al, ac, m, h);
                            }
                            Some(i) => {
                                *idx = i;
                                if seen[i] {
                                    let m = format!(
                                        "unreachable match arm: '{}' is already matched",
                                        short
                                    );
                                    self.err(al, ac, m);
                                }
                                seen[i] = true;
                                let fs = &vs[i].1;
                                if !bs.is_empty() && bs.len() != fs.len() {
                                    let m = format!(
                                        "pattern '{}' has {} binding(s) but the variant has {} field(s)",
                                        short,
                                        bs.len(),
                                        fs.len()
                                    );
                                    let h = format!("write {}", pat_hint(&short, fs));
                                    self.err_h(al, ac, m, h);
                                } else {
                                    fields = Some(fs.clone());
                                }
                            }
                        },
                    }
                    for (k, b) in bs.iter().enumerate() {
                        let bt = fields.as_ref().map_or(Ty::Err, |f| f[k].1.clone());
                        binds.push((b.clone(), bt));
                    }
                }
                p => {
                    let (pt, key) = match p {
                        Pat::Int(n) => (Ty::Int, n.to_string()),
                        Pat::Str(x) => (Ty::Str, format!("{:?}", x)),
                        Pat::Bool(b) => (Ty::Bool, b.to_string()),
                        _ => unreachable!(),
                    };
                    if !compat(&pt, &t) {
                        let m = format!(
                            "pattern {} is {} but the matched value is {}",
                            key,
                            a_an(&pt.to_string()),
                            t
                        );
                        self.err(al, ac, m);
                    } else if seen_lit.contains(&key) {
                        let m = format!("unreachable match arm: {} is already matched", key);
                        self.err(al, ac, m);
                    } else {
                        seen_lit.push(key);
                    }
                }
            }
            let mut scope = HashMap::new();
            for (b, bt) in binds {
                if b != "_" && scope.insert(b.clone(), (bt, Bind::Let)).is_some() {
                    self.err(al, ac, format!("'{}' is already declared in this block", b));
                }
            }
            self.scopes.push(scope);
            match value {
                None => self.block(&mut arm.body, true),
                Some(_) => {
                    self.scopes.push(HashMap::new());
                    let n = arm.body.len();
                    let last_val = matches!(arm.body.last(), Some(Stmt::Expr(_)));
                    for (k, st) in arm.body.iter_mut().enumerate() {
                        let Stmt::Expr(v) = st else {
                            self.stmt(st);
                            continue;
                        };
                        if k + 1 < n {
                            self.expr(v, None);
                            continue;
                        }
                        let ty = match &out {
                            Some(h) => {
                                let h = h.clone();
                                self.want(v, &h, "match arm value");
                                h
                            }
                            None => self.expr(v, None),
                        };
                        if ty == Ty::Unit {
                            self.err(v.line, v.col, "cannot bind a unit value".into());
                        } else if out.is_none() && ty != Ty::Err {
                            out = Some(ty);
                        }
                    }
                    self.scopes.pop();
                    all_leave &= diverges(&arm.body);
                    if !last_val && !diverges(&arm.body) {
                        let m =
                            "this match arm must end with a value (or return, break or continue)"
                                .into();
                        self.err(al, ac, m);
                    }
                }
            }
            self.scopes.pop();
        }
        if !wild {
            if let Some(vs) = &vs {
                let missing: Vec<&(String, Vec<(String, Ty)>)> = vs
                    .iter()
                    .zip(&seen)
                    .filter(|(_, s)| !**s)
                    .map(|(v, _)| v)
                    .collect();
                if !missing.is_empty() {
                    let names: Vec<&str> = missing.iter().map(|v| v.0.as_str()).collect();
                    let pats: Vec<String> = missing
                        .iter()
                        .map(|v| format!("'{}: ...'", pat_hint(&v.0, &v.1)))
                        .collect();
                    let m = format!(
                        "match on {} is not exhaustive: missing {}",
                        t,
                        names.join(", ")
                    );
                    let h = format!("add {} (or a final '_: ...' arm)", pats.join(", "));
                    let skel: Vec<String> = missing.iter().map(|v| pat_hint(&v.0, &v.1)).collect();
                    *self.alts.borrow_mut() = skel.clone();
                    self.arms_fix(arms, &skel, value.is_none());
                    self.err_h(l, c, m, h);
                }
            } else if t == Ty::Bool {
                for b in ["true", "false"] {
                    if !seen_lit.iter().any(|k| k == b) {
                        let m = format!("match on bool is not exhaustive: missing {}", b);
                        let h = format!("add '{}: ...' (or a final '_: ...' arm)", b);
                        *self.alts.borrow_mut() = vec![b.to_string()];
                        self.arms_fix(arms, &[b.to_string()], value.is_none());
                        self.err_h(l, c, m, h);
                    }
                }
            } else if lit {
                let m = format!("match on {} is not exhaustive: it needs a final '_' arm", t);
                let h = format!(
                    "literal patterns cannot cover every {}; add '_: ...' as the last arm",
                    t
                );
                self.arms_fix(arms, &["_".to_string()], value.is_none());
                self.err_h(l, c, m, h);
            }
        }
        if value.is_some() && all_leave {
            let m =
                "every arm of this match leaves (return, break or continue), so it has no value"
                    .into();
            let h = "write the match as a statement instead of binding or using its value".into();
            self.err_h(l, c, m, h);
            return Ty::Err;
        }
        match value {
            None => Ty::Unit,
            Some(_) => out.unwrap_or(Ty::Err),
        }
    }

    fn expr_inner(&mut self, e: &mut Expr, hint: Option<&Ty>) -> Ty {
        let (l, c) = (e.line, e.col);
        // Variant references are written as names, fields or calls.
        let vr = match &e.kind {
            ExprKind::Var(n) if self.lookup(n).is_none() => {
                let n = n.clone();
                self.variant_ref(&n, l, c)
            }
            ExprKind::Field(x, f) => match &x.kind {
                ExprKind::Var(a) if self.lookup(a).is_none() => {
                    let n = format!("{}.{}", a, f);
                    let (xl, xc) = (x.line, x.col);
                    self.variant_ref(&n, xl, xc)
                }
                _ => VarRef::No,
            },
            ExprKind::Call(n, _) if !BUILTINS.contains(&n.as_str()) => {
                let n = n.clone();
                self.variant_ref(&n, l, c)
            }
            _ => VarRef::No,
        };
        match vr {
            VarRef::No => {}
            VarRef::Bad => {
                if let ExprKind::Call(_, args) = &mut e.kind {
                    for a in args.iter_mut() {
                        self.expr(a, None);
                    }
                }
                return Ty::Err;
            }
            VarRef::Yes(en, i) => {
                let args = match std::mem::replace(&mut e.kind, ExprKind::Bool(false)) {
                    ExprKind::Call(_, a) => a,
                    _ => Vec::new(),
                };
                e.kind = ExprKind::Variant(en, i, args);
            }
        }
        match &mut e.kind {
            ExprKind::Variant(en, i, args) => {
                let en = en.clone();
                self.variant(&en, *i, args, hint, l, c)
            }
            ExprKind::Match(x, arms) => self.check_match(x, arms, Some(hint), l, c),
            ExprKind::Try(x) => {
                let t = self.expr(x, None);
                let r = self.ret.clone();
                match (&t, &r) {
                    (Ty::Err, _) => Ty::Err,
                    (Ty::Option(v), Ty::Option(_)) => (**v).clone(),
                    (Ty::Result(v, e1), Ty::Result(_, e2)) if compat(e1, e2) => (**v).clone(),
                    (Ty::Option(v) | Ty::Result(v, _), _) => {
                        let want = match &t {
                            Ty::Result(_, e) => format!("Result[_, {}]", e),
                            _ => "Option[_]".into(),
                        };
                        let m = format!(
                            "'?' on {} needs the enclosing fn to return {}, but it returns {}",
                            t, want, r
                        );
                        let h = "handle the value with match or unwrap_or, or change the fn's return type".into();
                        self.err_h(l, c, m, h);
                        (**v).clone()
                    }
                    _ => {
                        let m = format!("'?' needs an Option or Result, got {}", t);
                        self.err(l, c, m);
                        Ty::Err
                    }
                }
            }
            ExprKind::Ref(x) => {
                let t = self.expr(x, hint);
                let m = "'&' can only pass a variable to an 'inout' parameter of a fn".into();
                self.err_h(l, c, m, "remove the '&'".into());
                t
            }
            ExprKind::Int(_) => Ty::Int,
            ExprKind::Float(_) => Ty::Float,
            ExprKind::Str(_) => Ty::Str,
            ExprKind::Bool(_) => Ty::Bool,
            ExprKind::Var(n) => match self.lookup(n) {
                Some(t) => t.clone(),
                None => {
                    let n = n.clone();
                    let h = if n == "result" {
                        "'result' only exists in 'ensures' of a fn with a return type".into()
                    } else if n == "env" {
                        "'env' exists only at the top level of the root file (or as 'fn main(env: Env)'); pass what a fn needs as a parameter, e.g. 'fn load(fs: Fs)' called as load(env.fs)".into()
                    } else {
                        self.suggest_var(&n)
                    };
                    self.err_h(l, c, format!("undefined variable '{}'", n), h);
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
                        self.err(l, c, "cannot infer the type of an empty list; add an annotation like 'var xs: [int] = []'".into());
                        Ty::Err
                    }
                }
            }
            ExprKind::StructLit(name, fields) => {
                let mut q = self.item(&name.clone(), false, l, c);
                if let Some(en) = q.as_ref().filter(|q| self.enums.contains_key(*q)) {
                    let m = format!("'{}' is an enum, not a struct", name);
                    let v = self.enums[en]
                        .first()
                        .map_or(String::new(), |v| v.0.clone());
                    let h = format!("build an enum value with a variant, e.g. {}(...)", v);
                    self.err_h(l, c, m, h);
                    q = None;
                }
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
                            let names = decl.iter().map(|(n, _)| n.as_str());
                            *self.alts.borrow_mut() = ranked(f, names.clone());
                            let h = crate::diag::suggest(f, names);
                            self.err_h(v.line, v.col, m, h);
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
                let op = *op;
                if matches!(&b.kind, ExprKind::Var(n) if n == HOLE) && !binop_ok(op, &ta, &ta) {
                    if let Some(h) = self.hole.as_mut() {
                        h.none_fits = true;
                    }
                }
                self.binop(op, op, ta, tb, l, c)
            }
            ExprKind::Range(a, b) => {
                self.want(a, &Ty::Int, "range start");
                self.want(b, &Ty::Int, "range end");
                Ty::List(Box::new(Ty::Int))
            }
            ExprKind::Comp(elem, clauses) => {
                let depth = self.scopes.len();
                for cl in clauses.iter_mut() {
                    match cl {
                        Clause::For(v, src) => {
                            let t = self.iter_elem(src);
                            let mut s = HashMap::new();
                            s.insert(v.clone(), (t, Bind::Loop));
                            self.scopes.push(s);
                        }
                        Clause::If(cond) => self.want(cond, &Ty::Bool, "condition"),
                    }
                }
                let t = self.expr(elem, None);
                self.scopes.truncate(depth);
                match t {
                    Ty::Unit => {
                        self.err(elem.line, elem.col, "cannot bind a unit value".into());
                        Ty::Err
                    }
                    Ty::Err => Ty::Err,
                    t => Ty::List(Box::new(t)),
                }
            }
            ExprKind::Index(x, i) => {
                let t = self.expr(x, None);
                self.want(i, &Ty::Int, "index");
                match t {
                    Ty::List(t) => *t,
                    Ty::Err => Ty::Err,
                    t => {
                        let (l, c) = expr_start(x);
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
                            let names = self.structs[s].iter().map(|(n, _)| n.as_str());
                            *self.alts.borrow_mut() = ranked(f, names.clone());
                            let h = crate::diag::suggest(f, names);
                            self.err_h(l, c, m, h);
                            Ty::Err
                        }
                    },
                    Ty::Err => Ty::Err,
                    Ty::Cap(Cap::Env) => match Cap::env_field(f) {
                        Some(cap) => Ty::Cap(cap),
                        None => {
                            let m = format!("Env has no field '{}'", f);
                            let fs = ["fs", "vars", "clock", "args", "stdin"];
                            *self.alts.borrow_mut() = ranked(f, fs.iter().copied());
                            let h = match crate::diag::suggest(f, fs.iter().copied()) {
                                h if h.is_empty() => {
                                    "fields are: fs, vars, clock, args, stdin".into()
                                }
                                h => h,
                            };
                            self.err_h(l, c, m, h);
                            Ty::Err
                        }
                    },
                    Ty::Cap(cap) => {
                        let m = format!("{} has no fields", cap.name());
                        let h = format!(
                            "call a method: {}",
                            cap_methods(*cap)
                                .iter()
                                .map(|m| format!("x.{}(...)", m))
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                        self.err_h(l, c, m, h);
                        Ty::Err
                    }
                    t => {
                        let m = format!("{} has no fields", t);
                        let (l, c) = expr_start(x);
                        self.err(l, c, m);
                        Ty::Err
                    }
                }
            }
            ExprKind::Method(..) => unreachable!("handled by expr"),
            ExprKind::Call(name, args) if name.starts_with('$') => {
                // An already lowered capability access (checked again).
                for a in args.iter_mut() {
                    self.expr(a, None);
                }
                let (cap, m) = name[1..].split_once('.').unwrap_or_default();
                let cap = Cap::ALL.into_iter().find(|c| c.name() == cap);
                match cap {
                    Some(Cap::Env) => Cap::env_field(m).map_or(Ty::Err, Ty::Cap),
                    Some(cap) => cap_method(cap, m).map_or(Ty::Err, |x| x.2),
                    None => Ty::Err,
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
        let pos: Vec<(u32, u32)> = args.iter().map(expr_start).collect();
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
            // push/pop/sort change their list argument in place; `&` is optional.
            let mutates = matches!(name, "push" | "pop" | "sort");
            if mutates {
                if let Some(a) = args.first_mut() {
                    if let ExprKind::Ref(x) = &mut a.kind {
                        let x = std::mem::replace(
                            &mut **x,
                            Expr {
                                kind: ExprKind::Bool(false),
                                line: 0,
                                col: 0,
                                ty: Ty::Err,
                            },
                        );
                        *a = x;
                    }
                }
            }
            let ts: Vec<Ty> = match name {
                "push" | "unwrap_or" => Vec::new(),
                _ => args.iter_mut().map(|a| self.expr(a, None)).collect(),
            };
            let bad = |me: &mut Self, i: usize, what: &str, ts: &[Ty]| {
                let m = format!("'{}' expects {}, got {}", name, what, ts[i]);
                me.err(pos[i].0, pos[i].1, m);
            };
            if mutates && !args.is_empty() {
                if name == "push" {
                    // push types its arguments itself (below); check after.
                } else {
                    self.check_mut(&args[0]);
                }
            }
            return match name {
                "print" => {
                    for (i, t) in ts.iter().enumerate() {
                        if matches!(t, Ty::Unit | Ty::Cap(_)) {
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
                    let t0 = self.expr(&mut args[0], None);
                    self.check_mut(&args[0]);
                    match t0 {
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
                    if arity(1..=1, self) && matches!(ts[0], Ty::Unit | Ty::Cap(_)) {
                        bad(self, 0, "a printable value", &ts);
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
                "split" | "join" => {
                    let want = if name == "split" {
                        Ty::Str
                    } else {
                        Ty::List(Box::new(Ty::Str))
                    };
                    if arity(2..=2, self) {
                        if !compat(&ts[0], &want) {
                            bad(self, 0, &format!("a {}", want), &ts);
                        }
                        if !compat(&ts[1], &Ty::Str) {
                            bad(self, 1, "a str separator", &ts);
                        }
                    }
                    if name == "split" {
                        Ty::List(Box::new(Ty::Str))
                    } else {
                        Ty::Str
                    }
                }
                "sort" => {
                    let ok = |t: &Ty| matches!(t, Ty::Int | Ty::Float | Ty::Str | Ty::Err);
                    if arity(1..=1, self)
                        && !matches!(&ts[0], Ty::List(t) if ok(t))
                        && ts[0] != Ty::Err
                    {
                        bad(self, 0, "a list of int, float or str", &ts);
                    }
                    Ty::Unit
                }
                "sum" => {
                    if !arity(1..=1, self) {
                        return Ty::Err;
                    }
                    match &ts[0] {
                        Ty::List(t) if matches!(**t, Ty::Int | Ty::Float | Ty::Err) => {
                            (**t).clone()
                        }
                        Ty::Err => Ty::Err,
                        _ => {
                            bad(self, 0, "a list of int or float", &ts);
                            Ty::Err
                        }
                    }
                }
                "min" | "max" => {
                    let ord = |t: &Ty| matches!(t, Ty::Int | Ty::Float | Ty::Str | Ty::Err);
                    if !arity(1..=2, self) {
                        return Ty::Err;
                    }
                    if nargs == 1 {
                        return match &ts[0] {
                            Ty::List(t) if ord(t) => (**t).clone(),
                            Ty::Err => Ty::Err,
                            _ => {
                                bad(self, 0, "a list of int, float or str (or two values)", &ts);
                                Ty::Err
                            }
                        };
                    }
                    if ts[0] == Ty::Err || ts[1] == Ty::Err {
                        return Ty::Err;
                    }
                    if !ord(&ts[0]) {
                        bad(self, 0, "an int, float or str", &ts);
                        return Ty::Err;
                    }
                    if ts[1] != ts[0] {
                        bad(
                            self,
                            1,
                            &format!("a {} like the first argument", ts[0]),
                            &ts,
                        );
                        return Ty::Err;
                    }
                    ts[0].clone()
                }
                "any" | "all" => {
                    if arity(1..=1, self) && !compat(&ts[0], &Ty::List(Box::new(Ty::Bool))) {
                        bad(self, 0, "a [bool]", &ts);
                    }
                    Ty::Bool
                }
                "parse_int" | "parse_float" => {
                    if arity(1..=1, self) && !compat(&ts[0], &Ty::Str) {
                        bad(self, 0, "a str", &ts);
                    }
                    Ty::Option(Box::new(if name == "parse_int" {
                        Ty::Int
                    } else {
                        Ty::Float
                    }))
                }
                "find" => {
                    if !arity(2..=2, self) {
                        return Ty::Err;
                    }
                    let scalar = |t: &Ty| matches!(t, Ty::Int | Ty::Float | Ty::Bool | Ty::Str);
                    match (&ts[0], &ts[1]) {
                        (Ty::Err, _) | (_, Ty::Err) | (Ty::Str, Ty::Str) => {}
                        (Ty::List(t), v) if scalar(t) && **t == *v => {}
                        (Ty::List(t), _) if scalar(t) => {
                            bad(self, 1, &format!("a {} like the list elements", t), &ts)
                        }
                        _ => bad(
                            self,
                            0,
                            "a list of int, float, bool or str, or a str to search",
                            &ts,
                        ),
                    }
                    Ty::Option(Box::new(Ty::Int))
                }
                "get" => {
                    if !arity(2..=2, self) {
                        return Ty::Err;
                    }
                    if !compat(&ts[1], &Ty::Int) {
                        bad(self, 1, "an int index", &ts);
                    }
                    match &ts[0] {
                        Ty::List(t) => Ty::Option(t.clone()),
                        Ty::Err => Ty::Err,
                        _ => {
                            bad(self, 0, "a list", &ts);
                            Ty::Err
                        }
                    }
                }
                "unwrap" => {
                    if !arity(1..=1, self) {
                        return Ty::Err;
                    }
                    match &ts[0] {
                        Ty::Option(t) | Ty::Result(t, _) => (**t).clone(),
                        Ty::Err => Ty::Err,
                        _ => {
                            bad(self, 0, "an Option or Result", &ts);
                            Ty::Err
                        }
                    }
                }
                "unwrap_or" => {
                    if !arity(2..=2, self) {
                        for a in args.iter_mut() {
                            self.expr(a, None);
                        }
                        return Ty::Err;
                    }
                    match self.expr(&mut args[0], None) {
                        Ty::Option(t) | Ty::Result(t, _) => {
                            self.want(&mut args[1], &t, "default value");
                            *t
                        }
                        Ty::Err => {
                            self.expr(&mut args[1], None);
                            Ty::Err
                        }
                        t => {
                            let m = format!("'unwrap_or' expects an Option or Result, got {}", t);
                            self.err(args[0].line, args[0].col, m);
                            self.expr(&mut args[1], None);
                            Ty::Err
                        }
                    }
                }
                "is_some" | "is_none" | "is_ok" | "is_err" => {
                    let opt = name == "is_some" || name == "is_none";
                    if arity(1..=1, self) {
                        let ok = match &ts[0] {
                            Ty::Err => true,
                            Ty::Option(_) => opt,
                            Ty::Result(..) => !opt,
                            _ => false,
                        };
                        if !ok {
                            bad(self, 0, if opt { "an Option" } else { "a Result" }, &ts);
                        }
                    }
                    Ty::Bool
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
        let pinfo = self.params.get(name).cloned().unwrap_or_default();
        let mut roots: Vec<String> = Vec::new();
        for (i, a) in args.iter_mut().enumerate() {
            let what = format!("argument {} of '{}'", i + 1, name);
            let io = pinfo.get(i).is_some_and(|p| p.1);
            let pname = pinfo.get(i).map_or(String::new(), |p| p.0.clone());
            let (al, ac) = (a.line, a.col);
            match (ps.get(i), &mut a.kind) {
                (Some(t), ExprKind::Ref(x)) if io => {
                    self.want(x, t, &what);
                    self.check_mut(x);
                    if let Some(r) = place_root(x) {
                        if roots.contains(&r) {
                            let m =
                                format!("'{}' is passed to two inout parameters in one call", r);
                            let h = "pass it once and change the parts in separate calls".into();
                            self.err_h(al, ac, m, h);
                        }
                        roots.push(r);
                    }
                }
                (Some(t), ExprKind::Ref(x)) => {
                    self.want(x, t, &what);
                    let m = format!(
                        "'&' passes a variable to an inout parameter, but parameter '{}' of '{}' is not inout",
                        pname, name
                    );
                    let h = format!(
                        "remove the '&' (arguments are passed read-only), or declare the parameter '{}: inout {}'",
                        pname, t
                    );
                    self.fix.push(Fix {
                        msg: "remove the '&'".into(),
                        edits: vec![Edit::replace(al, ac, 1, String::new())],
                        machine: false,
                    });
                    self.err_h(al, ac, m, h);
                }
                (Some(t), _) => {
                    self.want(a, t, &what);
                    if io {
                        let src = place_text(a).unwrap_or_else(|| "x".into());
                        let m = format!(
                            "argument {} of '{}' is inout (parameter '{}'), so it must be passed as '&{}'",
                            i + 1,
                            name,
                            pname,
                            src
                        );
                        let h = if place_text(a).is_some() {
                            let (rl, rc) = place_start(a);
                            self.fix.push(Fix {
                                msg: format!("pass '&{}'", src),
                                edits: vec![Edit::insert(rl, rc, "&".into())],
                                machine: true,
                            });
                            format!("write '&{}' to let '{}' change it", src, name)
                        } else {
                            "store the value in a variable first ('var x = ...') and pass '&x'"
                                .to_string()
                        };
                        self.err_h(al, ac, m, h);
                    }
                }
                (None, _) => {
                    self.expr(a, None);
                }
            }
        }
        r
    }

    /// Report an attempt to change `e` (assignment target, `push`/`pop`/`sort`
    /// argument, `&` argument) unless it is a `var`, an inout parameter, or a
    /// field or element of one.
    fn check_mut(&mut self, e: &Expr) {
        let mut x = e;
        while let ExprKind::Field(b, _) | ExprKind::Index(b, _) = &x.kind {
            x = b;
        }
        let ExprKind::Var(n) = &x.kind else {
            let m = "can only change a variable, or a field or element of one".into();
            let h = "store the value in a variable first: var xs = ...".into();
            let (l, c) = expr_start(e);
            self.err_h(l, c, m, h);
            return;
        };
        let Some((t, b)) = self.binding(n).cloned() else {
            return; // undefined: already reported
        };
        let (m, h) = match b {
            Bind::Var | Bind::Inout => return,
            Bind::Param => (
                format!("cannot change parameter '{}': parameters are read-only", n),
                format!(
                    "to change the caller's value declare '{0}: inout {1}' and call with '&{0}'; to change a local copy start the body with 'var {0} = {0}'",
                    n, t
                ),
            ),
            Bind::Loop => (
                format!("cannot change loop variable '{}': it is a copy of the element", n),
                "change the element itself instead, e.g. 'xs[i] = ...' in 'for i in 0..len(xs)'"
                    .into(),
            ),
            Bind::Let => (
                {
                    let n = n.clone();
                    self.let_fix(&n);
                    format!("cannot change '{}': it is declared with 'let'", n)
                },
                format!("declare it with 'var {} = ...' to allow changes", n),
            ),
        };
        let (l, c) = place_start(e);
        self.err_h(l, c, m, h);
    }
}

/// Do the operand types suit the operator? (`Err` operands always do.)
fn binop_ok(op: &str, ta: &Ty, tb: &Ty) -> bool {
    if *ta == Ty::Err || *tb == Ty::Err {
        return true;
    }
    match op {
        "and" | "or" => *ta == Ty::Bool && *tb == Ty::Bool,
        "+" => ta == tb && matches!(ta, Ty::Int | Ty::Float | Ty::Str),
        "-" | "*" | "/" | "%" => ta == tb && matches!(ta, Ty::Int | Ty::Float),
        "==" | "!=" => ta == tb && matches!(ta, Ty::Int | Ty::Float | Ty::Str | Ty::Bool),
        _ => ta == tb && matches!(ta, Ty::Int | Ty::Float | Ty::Str),
    }
}

/// Position of the first token of an expression. Binary, index and field
/// nodes are located at their operator (runtime errors name it); diagnostics
/// about the whole value point at its start instead.
fn expr_start(e: &Expr) -> (u32, u32) {
    match &e.kind {
        ExprKind::Binary(_, b, _)
        | ExprKind::Field(b, _)
        | ExprKind::Index(b, _)
        | ExprKind::Try(b) => expr_start(b),
        _ => (e.line, e.col),
    }
}

/// Position of the root variable of a place expression (its first token).
fn place_start(e: &Expr) -> (u32, u32) {
    match &e.kind {
        ExprKind::Field(b, _) | ExprKind::Index(b, _) => place_start(b),
        _ => (e.line, e.col),
    }
}

/// Root variable of a place (`xs`, `p.items`, `grid[i][j]`).
pub fn place_root(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Var(n) => Some(n.clone()),
        ExprKind::Field(b, _) | ExprKind::Index(b, _) => place_root(b),
        _ => None,
    }
}

/// Source text of a simple place (`xs`, `p.items`) for hints.
fn place_text(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Var(n) => Some(n.clone()),
        ExprKind::Field(b, f) => Some(format!("{}.{}", place_text(b)?, f)),
        ExprKind::Index(b, _) => Some(format!("{}[...]", place_text(b)?)),
        _ => None,
    }
}
