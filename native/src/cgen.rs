//! C11 code generation from a checked program.

use crate::ast::*;
use std::collections::HashMap;
use std::fmt::Write;

pub struct Opts {
    pub checked: bool,
    pub max_steps: u64,
    pub max_depth: u64,
    /// Heap budget in bytes (all modes).
    pub max_mem: u64,
    /// Upper bound on the number of C translation units (for parallel cc).
    pub parts: usize,
    /// Runtime errors as JSON lines (`--json`).
    pub json: bool,
    /// Build the inline tests (`limits test`) instead of `main`.
    pub tests: bool,
    /// `--allow` grants: canonical path prefixes for fs.read / fs.write
    /// (empty = current directory) and env var names (empty = all).
    pub allow_read: Vec<String>,
    pub allow_write: Vec<String>,
    pub allow_env: Vec<String>,
}

/// Generated code below this many bytes always stays in one C file.
const SPLIT_BYTES: usize = 64 * 1024;

struct Gen<'a> {
    out: String,
    strs: Vec<String>,
    struct_idx: HashMap<&'a str, usize>,
    structs: &'a [Struct],
    scopes: Vec<HashMap<String, String>>,
    tmp: usize,
    ind: usize,
    ret_unit: bool,
    checked: bool,
    /// Added to line numbers: the current function's file index << 20.
    fofs: u32,
    /// `ensures` clauses of the current function, checked at every return.
    ensures: &'a [Expr],
    fname: &'a str,
}

fn cty(t: &Ty) -> String {
    match t {
        Ty::Int => "int64_t".into(),
        Ty::Float => "double".into(),
        Ty::Bool => "bool".into(),
        Ty::Str => "lm_str".into(),
        Ty::Unit => "void".into(),
        Ty::List(_) => "lm_list *".into(),
        Ty::Struct(n) => format!("S_{} *", mangle(n)),
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

/// Generate C. Returns one or more complete translation units; the first one
/// holds `main`. Several are returned only for large programs when
/// `o.parts > 1`, so the C compiler can run in parallel.
pub fn generate(
    prog: &Program,
    structs: &crate::check::StructMap,
    o: &Opts,
    root: &str,
) -> Vec<String> {
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
        scopes: Vec::new(),
        tmp: 0,
        ind: 1,
        ret_unit: true,
        checked: o.checked,
        fofs: 0,
        ensures: &[],
        fname: "",
    };
    // Tests are only compiled by `limits test`, which never calls `main`.
    let funcs: Vec<&Func> = prog
        .funcs
        .iter()
        .filter(|f| o.tests || f.test.is_none())
        .collect();
    // Struct declarations, constructors and printers.
    let mut decls = String::new();
    for s in &prog.structs {
        let _ = writeln!(decls, "typedef struct S_{0} S_{0};", mangle(&s.name));
    }
    for s in &prog.structs {
        let fields = &structs[&s.name];
        let _ = writeln!(decls, "struct S_{} {{", mangle(&s.name));
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
            "static LM_U S_{0} *mk_{0}({1}int l, int c) {{\n  S_{0} *r = lm_alloc(sizeof(S_{0}), l, c);",
            mangle(&s.name),
            params.iter().map(|p| format!("{}, ", p)).collect::<String>()
        );
        for (f, _) in fields {
            let _ = writeln!(decls, "  r->f_{0} = f_{0};", f);
        }
        decls.push_str("  return r;\n}\n");
    }
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
    decls.push_str("static const char *lm_file_name(int i) {\n  static const char *const n[] = {");
    let _ = write!(decls, "\"{}\"", c_escape(root));
    for f in prog.files.iter().skip(1) {
        let _ = write!(decls, ", \"{}\"", c_escape(f));
    }
    decls.push_str("};\n  return i < (int)(sizeof n / sizeof *n) ? n[i] : \"?\";\n}\n");

    let mut protos = String::new();
    let sig = |f: &Func| -> (String, Vec<Ty>, Ty) {
        let r = f.ret.as_ref().map_or(Ty::Unit, ty_of);
        let ps: Vec<Ty> = f.params.iter().map(|(_, t)| ty_of(t)).collect();
        (
            format!("LM_FN LM_U {} fn_{}", cty(&r), mangle(&f.name)),
            ps,
            r,
        )
    };
    for f in &funcs {
        let (head, ps, _) = sig(f);
        let ps: Vec<String> = ps.iter().map(cty).collect();
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
        g.scopes = vec![HashMap::new()];
        g.ret_unit = r == Ty::Unit;
        g.fofs = (f.file as u32) << 20;
        g.fname = &f.name;
        g.ensures = if o.checked { &f.ensures } else { &[] };
        let mut params = Vec::new();
        for ((n, _), t) in f.params.iter().zip(&ps) {
            let c = g.bind(n);
            params.push(format!("{} LM_U {}", cty(t), c));
        }
        let _ = writeln!(
            g.out,
            "{}({}) {{",
            head,
            if params.is_empty() {
                "void".into()
            } else {
                params.join(", ")
            }
        );
        if o.checked {
            let _ = writeln!(g.out, "  LM_ENTER({}, {});", f.line + g.fofs, f.col);
        }
        if o.checked {
            for e in &f.requires {
                g.contract(e, "requires");
            }
        }
        g.block(&f.body);
        if g.ret_unit {
            g.ensures(None);
            g.line("LM_LEAVE;");
        } else {
            g.line("__builtin_unreachable();");
        }
        g.out.push_str("}\n\n");
        chunks.push(std::mem::take(&mut g.out));
    }
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
    c.push_str(include_str!("runtime.h"));
    c.push_str("\n/* ---- program ---- */\n");
    c.push_str(&decls);
    for (i, s) in g.strs.iter().enumerate() {
        let _ = writeln!(
            c,
            "static LmStr LM_U lm_s{} = {{{}, \"{}\"}};",
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
        "LM_DEF LM_U int64_t lm_steps = LM_MAX_STEPS, lm_depth, lm_mem;\nLM_DEF char *lm_ap, *lm_aend, *lm_stack_lo;\nLM_DEF int lm_rl, lm_rc, lm_argc;\nLM_DEF char **lm_argv;\nLM_DEF lm_buf lm_line;\n",
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
    let mut body = String::from("fn__main();");
    if o.tests {
        body = "lm_argc--; lm_argv++;\n  switch (argc > 1 ? atoi(argv[1]) : -1) {\n".into();
        for (i, f) in funcs.iter().filter(|f| f.test.is_some()).enumerate() {
            let _ = writeln!(body, "  case {}: fn_{}(); break;", i, mangle(&f.name));
        }
        body.push_str("  default: return 2;\n  }");
    }
    let _ = write!(tus[0], "int main(int argc, char **argv) {{\n  static char obuf[1 << 16];\n  (void)argc; (void)argv;\n  setvbuf(stdout, obuf, _IOFBF, sizeof obuf);\n  lm_argc = argc; lm_argv = argv;\n  lm_stack_init();\n  {}\n  fflush(stdout);\n  return 0;\n}}\n", body);
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

fn ty_of(t: &TyExpr) -> Ty {
    match t {
        TyExpr::List(t) => Ty::List(Box::new(ty_of(t))),
        TyExpr::Name(n, ..) => match n.as_str() {
            "int" => Ty::Int,
            "float" => Ty::Float,
            "bool" => Ty::Bool,
            "str" => Ty::Str,
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

    fn bind(&mut self, name: &str) -> String {
        let c = self.fresh(&format!("v_{}", name));
        self.scopes
            .last_mut()
            .unwrap()
            .insert(name.to_string(), c.clone());
        c
    }

    fn var(&self, name: &str) -> String {
        self.scopes
            .iter()
            .rev()
            .find_map(|s| s.get(name))
            .cloned()
            .expect("checked")
    }

    fn desc(&self, t: &Ty) -> String {
        match t {
            Ty::Int => "i".into(),
            Ty::Float => "f".into(),
            Ty::Bool => "b".into(),
            Ty::Str => "s".into(),
            Ty::List(t) => format!("L{}", self.desc(t)),
            Ty::Struct(n) => format!("S{};", self.struct_idx[n.as_str()]),
            _ => unreachable!(),
        }
    }

    fn block(&mut self, b: &[Stmt]) {
        self.scopes.push(HashMap::new());
        for s in b {
            self.stmt(s);
        }
        self.scopes.pop();
    }

    fn sub_block(&mut self, b: &[Stmt]) {
        self.ind += 1;
        self.block(b);
        self.ind -= 1;
    }

    fn step(&mut self, l: u32, c: u32) {
        if self.checked {
            self.line(&format!("LM_STEP({}, {});", l + self.fofs, c));
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let(name, _, e, ..) => {
                let v = self.expr(e);
                let c = self.bind(name);
                self.line(&format!("{} LM_U {} = {};", cty(&e.ty), c, v));
            }
            Stmt::Assign(t, v) => {
                let val = self.expr(v);
                let code = match &t.kind {
                    ExprKind::Var(n) => format!("{} = {};", self.var(n), val),
                    ExprKind::Field(o, f) => format!("{}->f_{} = {};", self.expr(o), f, val),
                    ExprKind::Index(l, i) => {
                        format!(
                            "lm_set_{}({}, {}, {}, {}, {});",
                            elem(&l.ty).0,
                            self.expr(l),
                            self.expr(i),
                            val,
                            t.line + self.fofs,
                            t.col
                        )
                    }
                    _ => unreachable!(),
                };
                self.line(&code);
            }
            Stmt::Expr(e) => {
                let c = self.expr(e);
                self.line(&format!("(void)({});", c));
            }
            Stmt::If(arms, els) => {
                for (i, (cond, b)) in arms.iter().enumerate() {
                    let c = self.expr(cond);
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
                let c = self.expr(cond);
                self.line(&format!("while ((bool){}) {{", c));
                self.ind += 1;
                self.step(cond.line, cond.col);
                self.ind -= 1;
                self.sub_block(b);
                self.line("}");
            }
            Stmt::ForRange(v, a, b, body) => {
                let (ca, cb) = (self.expr(a), self.expr(b));
                let (k, e) = (self.fresh("k"), self.fresh("e"));
                self.line(&format!(
                    "for (int64_t {k} = {ca}, {e} = {cb}; {k} < {e}; {k}++) {{"
                ));
                self.ind += 1;
                self.step(a.line, a.col);
                self.scopes.push(HashMap::new());
                let cv = self.bind(v);
                self.line(&format!("int64_t LM_U {} = {};", cv, k));
                self.block(body);
                self.scopes.pop();
                self.ind -= 1;
                self.line("}");
            }
            Stmt::ForList(v, e, body) => {
                let ce = self.expr(e);
                let (k, l) = (self.fresh("k"), self.fresh("l"));
                let et = match &e.ty {
                    Ty::List(t) => (**t).clone(),
                    _ => unreachable!(),
                };
                let st = elem(&e.ty).1;
                self.line(&format!("for (lm_list *{l} = {ce}, *{l}_ = 0; !{l}_; {l}_ = {l}) for (int64_t {k} = 0; {k} < {l}->len; {k}++) {{"));
                self.ind += 1;
                self.step(e.line, e.col);
                self.scopes.push(HashMap::new());
                let cv = self.bind(v);
                self.line(&format!(
                    "{} LM_U {} = {};",
                    cty(&et),
                    cv,
                    from_slot(format!("(({st} *){l}->d)[{k}]"), &et)
                ));
                self.block(body);
                self.scopes.pop();
                self.ind -= 1;
                self.line("}");
            }
            Stmt::Break(..) => self.line("break;"),
            Stmt::Continue(..) => self.line("continue;"),
            Stmt::Return(e, ..) => {
                let leave = if self.checked { "LM_LEAVE; " } else { "" };
                match e {
                    None => {
                        self.line("{");
                        self.ensures(None);
                        self.line(&format!("  {}return;\n}}", leave));
                    }
                    Some(e) => {
                        let c = self.expr(e);
                        let r = self.fresh("r");
                        self.line(&format!("{{ {} {} = {};", cty(&e.ty), r, c));
                        self.ensures(Some(&r));
                        self.line(&format!("  {}return {}; }}", leave, r));
                    }
                }
            }
        }
    }

    /// Emit a runtime check of one contract clause.
    fn contract(&mut self, e: &Expr, what: &str) {
        let c = self.expr(e);
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
        let mut scope = HashMap::new();
        if let Some(r) = r {
            scope.insert("result".to_string(), r.to_string());
        }
        let params = self.scopes[0].clone();
        let saved = std::mem::replace(&mut self.scopes, vec![params, scope]);
        for e in self.ensures {
            self.contract(e, "ensures");
        }
        self.scopes = saved;
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

    fn expr(&mut self, e: &Expr) -> String {
        let (l, c) = (e.line + self.fofs, e.col);
        match &e.kind {
            ExprKind::Int(n) if *n == i64::MIN => "INT64_MIN".into(),
            ExprKind::Int(n) => format!("INT64_C({})", n),
            ExprKind::Float(f) => format!("{:?}", f),
            ExprKind::Bool(b) => b.to_string(),
            ExprKind::Str(s) => {
                let i = match self.strs.iter().position(|x| x == s) {
                    Some(i) => i,
                    None => {
                        self.strs.push(s.clone());
                        self.strs.len() - 1
                    }
                };
                format!("(&lm_s{})", i)
            }
            ExprKind::Var(n) => self.var(n),
            ExprKind::List(items) => {
                let (sx, st) = elem(&e.ty);
                if items.is_empty() {
                    return format!("lm_lit_{}(0, 0, {}, {})", sx, l, c);
                }
                let vs: Vec<String> = items.iter().map(|it| self.expr(it)).collect();
                format!(
                    "lm_lit_{}({}, ({}[]){{{}}}, {}, {})",
                    sx,
                    items.len(),
                    st,
                    vs.join(", "),
                    l,
                    c
                )
            }
            ExprKind::StructLit(name, fields) => {
                let decl = &self.structs[self.struct_idx[name.as_str()]];
                let mut args = Vec::new();
                for (f, _) in &decl.fields {
                    let v = &fields.iter().find(|(n, _)| n == f).unwrap().1;
                    args.push(self.expr(v));
                }
                args.push(format!("{}, {}", l, c));
                format!("mk_{}({})", mangle(name), args.join(", "))
            }
            ExprKind::Unary(op, x) => {
                let cx = self.expr(x);
                match (*op, &x.ty) {
                    ("not", _) => format!("(!{})", cx),
                    (_, Ty::Int) => format!("lm_neg({}, {}, {})", cx, l, c),
                    _ => format!("(-{})", cx),
                }
            }
            ExprKind::Binary("+", a, _) if a.ty == Ty::Str => {
                // Flatten `a + b + c` into one allocation instead of copying
                // the growing prefix once per `+`.
                let mut parts = Vec::new();
                let mut x = e;
                while let (ExprKind::Binary("+", l, r), Ty::Str) = (&x.kind, &x.ty) {
                    parts.push(self.expr(r));
                    x = l;
                }
                parts.push(self.expr(x));
                parts.reverse();
                let call = format!(
                    "lm_concat({}, (lm_str[]){{{}}}, {}, {})",
                    parts.len(),
                    parts.join(", "),
                    l,
                    c
                );
                self.costly("lm_str", call)
            }
            ExprKind::Binary(op, a, b) => {
                let (ca, cb) = (self.expr(a), self.expr(b));
                match (&a.ty, *op) {
                    (_, "and") => format!("({} && {})", ca, cb),
                    (_, "or") => format!("({} || {})", ca, cb),
                    (Ty::Int, "+" | "-" | "*" | "/" | "%") => {
                        let f = match *op {
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
                        let neg = if *op == "!=" { "!" } else { "" };
                        format!("({}{})", neg, self.costly("bool", call))
                    }
                    (Ty::Str, _) => {
                        let call = format!("lm_str_cmp({}, {}, {}, {})", ca, cb, l, c);
                        format!("({} {} 0)", self.costly("int", call), op)
                    }
                    _ => format!("({} {} {})", ca, op, cb),
                }
            }
            ExprKind::Index(x, i) => {
                let g = format!(
                    "lm_get_{}({}, {}, {}, {})",
                    elem(&x.ty).0,
                    self.expr(x),
                    self.expr(i),
                    l,
                    c
                );
                from_slot(g, &e.ty)
            }
            ExprKind::Field(x, f) => format!("{}->f_{}", self.expr(x), f),
            ExprKind::Call(name, args) => self.call(name, args, e),
        }
    }

    fn call(&mut self, name: &str, args: &[Expr], e: &Expr) -> String {
        let (l, c) = (e.line + self.fofs, e.col);
        let ca: Vec<String> = args.iter().map(|a| self.expr(a)).collect();
        let t0 = args.first().map(|a| a.ty.clone()).unwrap_or(Ty::Unit);
        match name {
            "print" => {
                let mut parts = Vec::new();
                for (i, (a, code)) in args.iter().zip(&ca).enumerate() {
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
                format!(
                    "({{ int64_t lm_ps = lm_line.len; {}; lm_print_end(lm_ps); }})",
                    parts.join(", ")
                )
            }
            "len" if t0 == Ty::Str => {
                self.costly("int64_t", format!("lm_str_len({}, {}, {})", ca[0], l, c))
            }
            "len" => format!("({})->len", ca[0]),
            "push" => format!(
                "lm_push_{}({}, {}, {}, {})",
                elem(&t0).0,
                ca[0],
                ca[1],
                l,
                c
            ),
            "pop" => from_slot(
                format!("lm_pop_{}({}, {}, {})", elem(&t0).0, ca[0], l, c),
                &e.ty,
            ),
            "str" => match &t0 {
                Ty::Str => ca[0].clone(),
                Ty::Int => format!("lm_str_int({}, {}, {})", ca[0], l, c),
                Ty::Float => format!("lm_str_float({}, {}, {})", ca[0], l, c),
                Ty::Bool => format!("lm_str_bool({}, {}, {})", ca[0], l, c),
                t => {
                    let call = format!(
                        "lm_to_str({}, \"{}\", {}, {})",
                        to_val(&ca[0], t),
                        self.desc(t),
                        l,
                        c
                    );
                    self.costly("lm_str", call)
                }
            },
            "int" => match &t0 {
                Ty::Int => ca[0].clone(),
                Ty::Float => format!("lm_int_of_float({}, {}, {})", ca[0], l, c),
                _ => format!("lm_int_of_str({}, {}, {})", ca[0], l, c),
            },
            "float" => match &t0 {
                Ty::Float => ca[0].clone(),
                Ty::Int => format!("((double){})", ca[0]),
                _ => format!("lm_float_of_str({}, {}, {})", ca[0], l, c),
            },
            "sqrt" => format!("sqrt({})", ca[0]),
            "split" => self.costly(
                "lm_list *",
                format!("lm_split({}, {}, {}, {})", ca[0], ca[1], l, c),
            ),
            "join" => self.costly(
                "lm_str",
                format!("lm_join({}, {}, {}, {})", ca[0], ca[1], l, c),
            ),
            "sort" => self.costly(
                "void",
                format!("lm_sort_{}({}, {}, {})", elem(&t0).0, ca[0], l, c),
            ),
            "read_file" => self.costly("lm_str", format!("lm_read_file({}, {}, {})", ca[0], l, c)),
            "write_file" => self.costly(
                "void",
                format!("lm_write_file({}, {}, {}, {})", ca[0], ca[1], l, c),
            ),
            "input_line" => self.costly("lm_str", format!("lm_input_line({}, {})", l, c)),
            "env" => format!("lm_env({}, {}, {})", ca[0], l, c),
            "now_ms" => "lm_now_ms()".into(),
            "args" => format!("lm_args({}, {})", l, c),
            "assert" => {
                let msg = ca.get(1).map_or("NULL", |s| s.as_str());
                format!(
                    "((void)(({}) || lm_assert_fail({}, {}, {})))",
                    ca[0], l, c, msg
                )
            }
            _ => {
                let call = format!("fn_{}({})", mangle(name), ca.join(", "));
                match (&e.ty, self.checked) {
                    (_, false) => call,
                    (Ty::Unit, _) => format!("LM_CALLV({})", call),
                    (t, _) => format!("LM_CALL({}, {})", cty(t), call),
                }
            }
        }
    }
}
