//! Source-accurate lexical intelligence for agents and editors.
//!
//! Bindings are scope slots, not runtime values. A reference records all lexical
//! candidates, nearest first: before a declaration executes, the interpreter can
//! fall through to an outer binding. This index deliberately does not pretend
//! to know which branch or closure invocation executes first.
use crate::error::{LimError, Span};
use crate::interp::BUILTINS;
use crate::lexer::{self, Tok, Token};
use crate::{parser, resolve};
use std::collections::{BTreeMap, BTreeSet, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceRange {
    pub start: Span,
    /// End-exclusive, in one-based Unicode character columns like LimError.
    pub end: Span,
}
impl SourceRange {
    pub fn contains(&self, line: usize, col: usize) -> bool {
        (line, col) >= (self.start.line, self.start.col)
            && (line, col) < (self.end.line, self.end.col)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub id: usize,
    pub name: String,
    pub kind: String,
    pub scope: usize,
    /// Redeclarations in one scope share a binding ID and keep every location.
    /// Builtins have no source declarations.
    pub declarations: Vec<SourceRange>,
    pub parameters: Vec<String>,
    /// Conservative direct user-binding captures, including fallback candidates.
    pub captures: Vec<usize>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub id: usize,
    pub parent: Option<usize>,
    pub kind: String,
    pub range: Option<SourceRange>,
    pub owner_function: Option<usize>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub name: String,
    pub range: SourceRange,
    pub scope: usize,
    /// read, write, or call (direct callee references only).
    pub role: String,
    /// Nearest lexical binding, not a claim about runtime initialization.
    pub binding: Option<usize>,
    pub candidates: Vec<usize>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticIndex {
    pub symbols: Vec<Symbol>,
    pub scopes: Vec<Scope>,
    pub references: Vec<Reference>,
    pub diagnostics: Vec<LimError>,
}

pub fn analyze(src: &str) -> Result<SemanticIndex, LimError> {
    let tokens = lexer::lex(src)?;
    let program = parser::parse(tokens.clone())?;
    let diagnostics = resolve::check(&program);
    drop(program); // Indexing no longer needs the execution AST.
    let end = tokens.last().unwrap().span;
    let mut builder = Builder {
        tokens,
        pos: 0,
        scope: 1,
        index: SemanticIndex {
            symbols: Vec::new(),
            scopes: vec![
                Scope {
                    id: 0,
                    parent: None,
                    kind: "builtins".into(),
                    range: None,
                    owner_function: None,
                },
                Scope {
                    id: 1,
                    parent: Some(0),
                    kind: "module".into(),
                    range: Some(SourceRange {
                        start: Span { line: 1, col: 1 },
                        end,
                    }),
                    owner_function: None,
                },
            ],
            references: Vec::new(),
            diagnostics,
        },
        bindings: vec![BTreeMap::new(), BTreeMap::new()],
    };
    for name in BUILTINS {
        builder.declare(name, "builtin", None, 0, Vec::new());
    }
    while builder.peek() != &Tok::Eof {
        builder.stmt();
    }
    builder.resolve();
    Ok(builder.index)
}

impl SemanticIndex {
    pub fn definition_at(&self, line: usize, col: usize) -> Option<&Symbol> {
        self.definitions_at(line, col).into_iter().next()
    }
    pub fn definitions_at(&self, line: usize, col: usize) -> Vec<&Symbol> {
        if let Some(symbol) = self
            .symbols
            .iter()
            .find(|s| s.declarations.iter().any(|r| r.contains(line, col)))
        {
            return vec![symbol];
        }
        self.references
            .iter()
            .find(|r| r.range.contains(line, col))
            .map(|r| r.candidates.iter().map(|&id| &self.symbols[id]).collect())
            .unwrap_or_default()
    }
    pub fn references_to(&self, symbol_id: usize) -> Vec<&Reference> {
        self.references
            .iter()
            .filter(|r| r.candidates.contains(&symbol_id))
            .collect()
    }
    pub fn completions(&self, line: usize, col: usize, prefix: &str) -> Vec<&Symbol> {
        let Some(scope) = self
            .scopes
            .iter()
            .filter(|s| {
                s.range.is_some_and(|r| {
                    r.contains(line, col)
                        || (s.kind == "module" && (line, col) == (r.end.line, r.end.col))
                })
            })
            .max_by_key(|s| self.scope_depth(s.id))
        else {
            return Vec::new();
        };
        let mut seen = HashSet::new();
        let mut matches = Vec::new();
        let mut cursor = Some(scope.id);
        while let Some(id) = cursor {
            for symbol in self.symbols.iter().filter(|s| s.scope == id) {
                if seen.insert(&symbol.name) && symbol.name.starts_with(prefix) {
                    matches.push(symbol);
                }
            }
            cursor = self.scopes[id].parent;
        }
        matches.sort_by(|a, b| a.name.cmp(&b.name));
        matches
    }
    fn scope_depth(&self, mut id: usize) -> usize {
        let mut n = 0;
        while let Some(parent) = self.scopes[id].parent {
            n += 1;
            id = parent;
        }
        n
    }
    pub fn to_json(&self) -> String {
        use crate::protocol::quote;
        let symbols=self.symbols.iter().map(|s|format!(
            "{{\"id\":{},\"name\":{},\"kind\":{},\"scope\":{},\"declarations\":[{}],\"parameters\":[{}],\"captures\":[{}]}}",
            s.id,quote(&s.name),quote(&s.kind),s.scope,
            s.declarations.iter().map(range_json).collect::<Vec<_>>().join(","),
            s.parameters.iter().map(|p|quote(p)).collect::<Vec<_>>().join(","),
            ids(&s.captures))).collect::<Vec<_>>().join(",");
        let scopes = self
            .scopes
            .iter()
            .map(|s| {
                format!(
                    "{{\"id\":{},\"parent\":{},\"kind\":{},\"range\":{},\"owner_function\":{}}}",
                    s.id,
                    optional_id(s.parent),
                    quote(&s.kind),
                    s.range
                        .as_ref()
                        .map(range_json)
                        .unwrap_or_else(|| "null".into()),
                    optional_id(s.owner_function)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let references=self.references.iter().map(|r|format!(
            "{{\"name\":{},\"range\":{},\"scope\":{},\"role\":{},\"binding\":{},\"candidates\":[{}]}}",
            quote(&r.name),range_json(&r.range),r.scope,quote(&r.role),optional_id(r.binding),ids(&r.candidates))).collect::<Vec<_>>().join(",");
        let diagnostics = self
            .diagnostics
            .iter()
            .take(crate::protocol::MAX_DIAGNOSTICS)
            .map(crate::protocol::diagnostic_json)
            .collect::<Vec<_>>()
            .join(",");
        let diagnostic_count = self.diagnostics.len();
        let diagnostics_truncated = diagnostic_count > crate::protocol::MAX_DIAGNOSTICS;
        format!("{{\"schema_version\":1,\"position_encoding\":\"unicode-scalar-1-based\",\"resolution\":\"lexical-candidates\",\"symbols\":[{symbols}],\"scopes\":[{scopes}],\"references\":[{references}],\"diagnostic_count\":{diagnostic_count},\"diagnostics_truncated\":{diagnostics_truncated},\"diagnostics\":[{diagnostics}]}}")
    }
}
fn ids(ids: &[usize]) -> String {
    ids.iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(",")
}
fn optional_id(id: Option<usize>) -> String {
    id.map(|n| n.to_string()).unwrap_or_else(|| "null".into())
}
fn range_json(range: &SourceRange) -> String {
    format!(
        "{{\"start\":{{\"line\":{},\"col\":{}}},\"end\":{{\"line\":{},\"col\":{}}}}}",
        range.start.line, range.start.col, range.end.line, range.end.col
    )
}

// The normal parser validates syntax and nesting first. This bounded token walk
// retains source details the execution AST intentionally discards (notably
// declaration/parameter locations and block boundaries).
struct Builder {
    tokens: Vec<Token>,
    pos: usize,
    scope: usize,
    index: SemanticIndex,
    bindings: Vec<BTreeMap<String, usize>>,
}
impl Builder {
    fn peek(&self) -> &Tok {
        &self.tokens[self.pos].tok
    }
    fn bump(&mut self) -> Token {
        let t = self.tokens[self.pos].clone();
        self.pos += 1;
        t
    }
    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == t {
            self.bump();
            true
        } else {
            false
        }
    }
    fn expect(&mut self, t: Tok) {
        debug_assert_eq!(self.peek(), &t);
        self.bump();
    }
    fn ident(&mut self) -> (String, SourceRange) {
        let token = self.bump();
        let Tok::Ident(name) = token.tok else {
            unreachable!("validated identifier")
        };
        let range = SourceRange {
            start: token.span,
            end: Span {
                line: token.span.line,
                col: token.span.col + name.chars().count(),
            },
        };
        (name, range)
    }
    fn declare(
        &mut self,
        name: &str,
        kind: &str,
        range: Option<SourceRange>,
        scope: usize,
        parameters: Vec<String>,
    ) -> usize {
        if let Some(&id) = self.bindings[scope].get(name) {
            let s = &mut self.index.symbols[id];
            if let Some(range) = range {
                s.declarations.push(range);
            }
            if s.kind != kind {
                s.kind = "binding".into();
            }
            if kind == "function" {
                s.parameters = parameters;
            }
            return id;
        }
        let id = self.index.symbols.len();
        self.bindings[scope].insert(name.into(), id);
        self.index.symbols.push(Symbol {
            id,
            name: name.into(),
            kind: kind.into(),
            scope,
            declarations: range.into_iter().collect(),
            parameters,
            captures: Vec::new(),
        });
        id
    }
    fn new_scope(&mut self, kind: &str, start: Span, owner: Option<usize>) -> usize {
        let id = self.index.scopes.len();
        self.index.scopes.push(Scope {
            id,
            parent: Some(self.scope),
            kind: kind.into(),
            range: Some(SourceRange { start, end: start }),
            owner_function: owner.or(self.index.scopes[self.scope].owner_function),
        });
        self.bindings.push(BTreeMap::new());
        id
    }
    fn block(
        &mut self,
        kind: &str,
        owner: Option<usize>,
        pre: Vec<(String, SourceRange, String)>,
    ) -> usize {
        let start = self.bump().span; // validated '{'
        let outer = self.scope;
        let id = self.new_scope(kind, start, owner);
        self.scope = id;
        for (n, r, k) in pre {
            self.declare(&n, &k, Some(r), id, Vec::new());
        }
        while self.peek() != &Tok::RBrace {
            self.stmt();
        }
        let end = self.bump().span;
        self.index.scopes[id].range.as_mut().unwrap().end = Span {
            line: end.line,
            col: end.col + 1,
        };
        self.scope = outer;
        id
    }
    fn stmt(&mut self) {
        match self.peek() {
            Tok::Let => {
                self.bump();
                let (n, r) = self.ident();
                self.declare(&n, "variable", Some(r), self.scope, Vec::new());
                self.expect(Tok::Assign);
                self.expr(0);
                self.expect(Tok::Semi);
            }
            Tok::Fn => {
                self.bump();
                let (n, r) = self.ident();
                self.expect(Tok::LParen);
                let mut params = Vec::new();
                if self.peek() != &Tok::RParen {
                    loop {
                        let (n, r) = self.ident();
                        params.push((n, r, "parameter".into()));
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                }
                self.expect(Tok::RParen);
                let signature = params.iter().map(|(n, _, _)| n.clone()).collect();
                let id = self.declare(&n, "function", Some(r), self.scope, signature);
                self.block("function", Some(id), params);
            }
            Tok::If => {
                self.bump();
                self.expr(0);
                self.block("block", None, Vec::new());
                if self.eat(&Tok::Else) {
                    if self.peek() == &Tok::If {
                        let start = self.tokens[self.pos].span;
                        let outer = self.scope;
                        let id = self.new_scope("else-if", start, None);
                        self.scope = id;
                        self.stmt();
                        let end = self.tokens[self.pos - 1].span;
                        self.index.scopes[id].range.as_mut().unwrap().end = Span {
                            line: end.line,
                            col: end.col + 1,
                        };
                        self.scope = outer;
                    } else {
                        self.block("block", None, Vec::new());
                    }
                }
            }
            Tok::While => {
                self.bump();
                self.expr(0);
                self.block("loop", None, Vec::new());
            }
            Tok::For => {
                self.bump();
                let (n, r) = self.ident();
                self.expect(Tok::In);
                self.expr(0);
                self.block("loop", None, vec![(n, r, "loop-variable".into())]);
            }
            Tok::Return => {
                self.bump();
                if self.peek() != &Tok::Semi {
                    self.expr(0);
                }
                self.expect(Tok::Semi);
            }
            Tok::Break | Tok::Continue => {
                self.bump();
                self.expect(Tok::Semi);
            }
            _ => {
                let direct = self.expr(0);
                if self.eat(&Tok::Assign) {
                    if let Some(id) = direct {
                        self.index.references[id].role = "write".into();
                    }
                    self.expr(0);
                }
                self.expect(Tok::Semi);
            }
        }
    }
    fn expr(&mut self, min: u8) -> Option<usize> {
        let mut direct = match self.peek() {
            Tok::Ident(_) => {
                let (n, range) = self.ident();
                let id = self.index.references.len();
                self.index.references.push(Reference {
                    name: n,
                    range,
                    scope: self.scope,
                    role: "read".into(),
                    binding: None,
                    candidates: Vec::new(),
                });
                Some(id)
            }
            Tok::Not => {
                self.bump();
                self.expr(3);
                None
            }
            Tok::Minus => {
                self.bump();
                self.expr(8);
                None
            }
            Tok::LParen => {
                self.bump();
                let d = self.expr(0);
                self.expect(Tok::RParen);
                d
            }
            Tok::LBracket => {
                self.bump();
                while self.peek() != &Tok::RBracket {
                    self.expr(0);
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
                self.expect(Tok::RBracket);
                None
            }
            Tok::LBrace => {
                self.bump();
                while self.peek() != &Tok::RBrace {
                    self.expr(0);
                    self.expect(Tok::Colon);
                    self.expr(0);
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
                self.expect(Tok::RBrace);
                None
            }
            _ => {
                self.bump();
                None
            }
        };
        loop {
            if self.peek() == &Tok::LParen {
                self.bump();
                if let Some(id) = direct {
                    self.index.references[id].role = "call".into();
                }
                if self.peek() != &Tok::RParen {
                    loop {
                        self.expr(0);
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                }
                self.expect(Tok::RParen);
                direct = None;
                continue;
            }
            if self.peek() == &Tok::LBracket {
                self.bump();
                self.expr(0);
                self.expect(Tok::RBracket);
                direct = None;
                continue;
            }
            let prec = match self.peek() {
                Tok::Or => 1,
                Tok::And => 2,
                Tok::Eq | Tok::Ne | Tok::Lt | Tok::Le | Tok::Gt | Tok::Ge => 4,
                Tok::DotDot => 5,
                Tok::Plus | Tok::Minus => 6,
                Tok::Star | Tok::Slash | Tok::Percent => 7,
                _ => break,
            };
            if prec < min {
                break;
            }
            self.bump();
            self.expr(prec + 1);
            direct = None;
        }
        direct
    }
    fn resolve(&mut self) {
        let mut captures: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
        for r in &mut self.index.references {
            let owner = self.index.scopes[r.scope].owner_function;
            let mut scope = Some(r.scope);
            while let Some(id) = scope {
                if let Some(&binding) = self.bindings[id].get(&r.name) {
                    r.candidates.push(binding);
                    let s = &self.index.symbols[binding];
                    if let Some(owner) = owner {
                        if s.kind != "builtin"
                            && self.index.scopes[s.scope].owner_function != Some(owner)
                        {
                            captures.entry(owner).or_default().insert(binding);
                        }
                    }
                }
                scope = self.index.scopes[id].parent;
            }
            r.binding = r.candidates.first().copied();
        }
        for (id, bindings) in captures {
            self.index.symbols[id].captures = bindings.into_iter().collect();
        }
    }
}
