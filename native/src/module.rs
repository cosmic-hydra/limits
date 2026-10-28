//! Module loader: resolves `import a.b;` to `a/b.lim` relative to the importing
//! file, lexes and parses each file exactly once (cached by canonical path) and
//! detects import cycles.

use crate::ast::Program;
use crate::{lexer, parser, Error};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct Module {
    /// Qualifier for this module's items ("" for the root, else e.g. "util.strings").
    pub name: String,
    /// Path shown in errors ("" for the root file).
    pub file: String,
    pub prog: Program,
    /// Import alias (last path segment) -> module index.
    pub aliases: HashMap<String, usize>,
}

struct Loader {
    mods: Vec<Module>,
    cache: HashMap<PathBuf, usize>,
    stack: Vec<PathBuf>,
    base: PathBuf,
    errs: Vec<Error>,
}

/// Load the root file and everything it imports. Module 0 is the root.
pub fn load(root: &Path) -> Result<Vec<Module>, Vec<Error>> {
    let canon = std::fs::canonicalize(root).unwrap_or_else(|e| {
        eprintln!("limits: cannot read '{}': {}", root.display(), e);
        std::process::exit(2)
    });
    let mut l = Loader {
        mods: Vec::new(),
        cache: HashMap::new(),
        stack: Vec::new(),
        base: canon.parent().map(Path::to_path_buf).unwrap_or_default(),
        errs: Vec::new(),
    };
    l.file(canon, String::new());
    if l.errs.is_empty() {
        Ok(l.mods)
    } else {
        Err(l.errs)
    }
}

impl Loader {
    fn show(&self, p: &Path) -> String {
        p.strip_prefix(&self.base)
            .unwrap_or(p)
            .display()
            .to_string()
    }

    fn err(&mut self, mut e: Error, file: &str) {
        e.file = file.to_string();
        self.errs.push(e);
    }

    fn file(&mut self, path: PathBuf, name: String) -> Option<usize> {
        let file = if name.is_empty() {
            String::new()
        } else {
            self.show(&path)
        };
        let src = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("limits: cannot read '{}': {}", path.display(), e);
                std::process::exit(2)
            }
        };
        let prog = match lexer::lex(&src).and_then(parser::parse) {
            Ok(p) => p,
            Err(e) => {
                self.err(e, &file);
                return None;
            }
        };
        let imports = prog.imports.clone();
        let idx = self.mods.len();
        self.mods.push(Module {
            name,
            file: file.clone(),
            prog,
            aliases: HashMap::new(),
        });
        self.cache.insert(path.clone(), idx);
        self.stack.push(path.clone());
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        for (imp, line, col) in imports {
            let target = dir.join(imp.replace('.', "/") + ".lim");
            let alias = imp.rsplit('.').next().unwrap().to_string();
            let cerr = |m: String| Error::new("check", line, col, m);
            let Ok(canon) = std::fs::canonicalize(&target) else {
                let m = format!(
                    "cannot find module '{}' (no file {})",
                    imp,
                    self.show(&target)
                );
                self.err(cerr(m), &file);
                continue;
            };
            if let Some(pos) = self.stack.iter().position(|p| *p == canon) {
                let mut cyc: Vec<String> = self.stack[pos..].iter().map(|p| self.show(p)).collect();
                cyc.push(self.show(&canon));
                let m = format!("import cycle: {}", cyc.join(" -> "));
                self.err(cerr(m), &file);
                continue;
            }
            let m = match self.cache.get(&canon) {
                Some(&m) => Some(m),
                None => {
                    // Qualifier: the import path, made unique if another file
                    // was already imported under the same path text.
                    let mut q = imp.clone();
                    let mut n = 2;
                    while self.mods.iter().any(|m| m.name == q) {
                        q = format!("{}{}", imp, n);
                        n += 1;
                    }
                    self.file(canon, q)
                }
            };
            let Some(m) = m else { continue };
            if self.mods[idx].aliases.insert(alias.clone(), m).is_some() {
                let m = format!("module name '{}' imported twice", alias);
                self.err(cerr(m), &file);
            }
        }
        self.stack.pop();
        Some(idx)
    }
}
