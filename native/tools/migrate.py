#!/usr/bin/env python3
"""One-shot converter from the old brace/semicolon Limits syntax to the
indentation-based syntax (SPEC.md v0.2).

Usage: python3 tools/migrate.py FILE.lim...        (rewrites the files in place)
       python3 tools/migrate.py - < old.lim         (prints the result)

What it does:
- `{ ... }` blocks become `:` plus a 4-space indented block; `;` is dropped,
  one statement per line (one-line `a; b; c` bodies are split).
- `else if` becomes `elif`; an empty block becomes `pass`.
- `struct S { a: T, b: U }` becomes a block of `a: T` lines.
- `requires e;` / `ensures e;` move to the first lines of the fn body.
- `let x` becomes `var x` when `x` is reassigned in the same function;
  a reassigned parameter gets `var p = p` as the first body line.
- `x = x + e` becomes `x += e` (also - * / %, also for `xs[i]` / `p.f`
  targets) when `e` has no operator that would change the grouping.
- A final `return e;` in a function body becomes the implicit result `e`.
- `fn main() { body }` becomes top-level statements (script mode), unless
  `--keep-main` is given.
Comments are kept (own-line comments before the next statement, trailing
comments after the statement on their line). Review the output: loop
variables that are reassigned are reported on stderr and need a hand fix.
"""
import re
import sys

KW = {"let", "fn", "return", "if", "else", "while", "for", "in", "true", "false", "and", "or",
      "not", "break", "continue", "struct", "import", "pub"}
SYMS = ["..", "==", "!=", "<=", ">=", "->", "(", ")", "{", "}", "[", "]", ",", ";", ":", "+",
        "-", "*", "/", "%", "=", "<", ">", "."]
OPEN, CLOSE = "([{", ")]}"


class Tok:
    __slots__ = ("kind", "text", "start", "end", "line")

    def __init__(self, kind, text, start, end, line):
        self.kind, self.text, self.start, self.end, self.line = kind, text, start, end, line

    def __repr__(self):
        return f"{self.kind}:{self.text}"


def tokenize(src):
    toks, comments = [], []
    i, line = 0, 1
    while i < len(src):
        c = src[i]
        if c == "\n":
            line += 1
            i += 1
        elif c.isspace():
            i += 1
        elif c == "#":
            j = src.find("\n", i)
            j = len(src) if j < 0 else j
            comments.append(Tok("comment", src[i:j], i, j, line))
            i = j
        elif c.isdigit():
            m = re.compile(r"[0-9_]+(\.[0-9]+)?").match(src, i)
            toks.append(Tok("num", m.group(), i, m.end(), line))
            i = m.end()
        elif c.isalpha() or c == "_":
            m = re.compile(r"[A-Za-z0-9_]+").match(src, i)
            w = m.group()
            toks.append(Tok("kw" if w in KW else "id", w, i, m.end(), line))
            i = m.end()
        elif c == '"':
            j = i + 1
            while src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            toks.append(Tok("str", src[i:j + 1], i, j + 1, line))
            i = j + 1
        else:
            s = next((s for s in SYMS if src.startswith(s, i)), None)
            if s is None:
                raise SystemExit(f"migrate: unexpected character {c!r} at line {line}")
            toks.append(Tok("sym", s, i, i + len(s), line))
            i += len(s)
    toks.append(Tok("eof", "", len(src), len(src), line))
    return toks, comments


def is_operand_end(t):
    return t.kind in ("id", "num", "str") or t.text in (")", "]", "}", "true", "false")


# Operators that, at depth 0 in `rest`, would make `x = x op rest` differ
# from `x op= rest`.
LOWER = {
    "+": {"+", "-", "==", "!=", "<", "<=", ">", ">=", "and", "or", "not", ".."},
}
LOWER["-"] = LOWER["+"]
LOWER["*"] = LOWER["+"] | {"*", "/", "%"}
LOWER["/"] = LOWER["*"]
LOWER["%"] = LOWER["*"]


class Migrator:
    def __init__(self, src, keep_main=False):
        self.src = src
        self.toks, self.comments = tokenize(src)
        self.lines = src.split("\n")
        self.pos = 0
        self.out = []
        self.keep_main = keep_main
        self.last_line = 0  # source line of the last emitted text
        self.warnings = []

    # ---- token helpers ----
    def peek(self, k=0):
        return self.toks[min(self.pos + k, len(self.toks) - 1)]

    def next(self):
        t = self.toks[self.pos]
        self.pos += 1
        return t

    def expect(self, text):
        t = self.next()
        if t.text != text:
            raise SystemExit(f"migrate: expected {text!r} at line {t.line}, found {t.text!r}")
        return t

    def until(self, stops, block_brace=False):
        """Tokens up to (not including) the first depth-0 token in `stops`.
        With block_brace, a depth-0 `{` also stops (block heads)."""
        out, depth = [], 0
        while True:
            t = self.peek()
            if t.kind == "eof":
                raise SystemExit("migrate: unexpected end of file")
            if depth == 0 and (t.text in stops or (block_brace and t.text == "{")):
                return out
            if t.text in OPEN:
                depth += 1
            elif t.text in CLOSE:
                depth -= 1
            out.append(self.next())

    def text(self, toks):
        s = self.src[toks[0].start:toks[-1].end]
        for c in self.comments:
            if toks[0].start <= c.start < toks[-1].end:
                c.kind = "used"
        return s

    # ---- output ----
    def emit(self, indent, s, line):
        gap = self.lines[self.last_line:max(self.last_line, line - 1)]
        if self.out and self.out[-1].strip() and any(not g.strip() for g in gap):
            self.out.append("")
        self.out.append("    " * indent + s)
        self.last_line = max(line, s.count("\n") + line)

    def flush_comments(self, before, indent):
        for c in self.comments:
            if c.kind == "comment" and c.start < before:
                c.kind = "used"
                self.emit(indent, c.text, c.line)

    def trailing_comment(self, last):
        """A comment right after token `last` on the same line."""
        nxt = self.peek()
        for c in self.comments:
            if c.kind == "comment" and c.line == last.line and c.start >= last.end and c.start < nxt.start:
                c.kind = "used"
                self.out[-1] += "  " + c.text
                return

    # ---- items ----
    def program(self):
        while self.peek().kind != "eof":
            t = self.peek()
            self.flush_comments(t.start, 0)
            if t.text in ("import",) or (t.text == "uses" and self.peek(1).kind == "id"):
                toks = self.until({";"})
                self.emit(0, self.text(toks), t.line)
                end = self.next()
                self.trailing_comment(end)
            elif t.text == "test" and self.peek(1).kind == "str":
                self.next()
                name = self.next()
                self.emit(0, f"test {name.text}:", t.line)
                self.block(1, set(), None)
            elif t.text in ("pub", "fn", "struct"):
                pub = t.text == "pub"
                if pub:
                    self.next()
                kw = self.next()
                if kw.text == "fn":
                    self.func(pub, t)
                else:
                    self.struct(pub, t)
            else:
                raise SystemExit(f"migrate: unexpected {t.text!r} at line {t.line}")
        self.flush_comments(len(self.src) + 1, 0)
        return "\n".join(self.out).rstrip("\n") + "\n"

    def struct(self, pub, start):
        name = self.next().text
        self.expect("{")
        fields = []
        while self.peek().text != "}":
            ft = self.until({",", "}"})
            fields.append((self.text(ft), ft[0].line))
            if self.peek().text == ",":
                self.next()
        self.expect("}")
        self.emit(0, ("pub " if pub else "") + f"struct {name}:", start.line)
        for f, line in fields:
            self.emit(1, f, line)

    def func(self, pub, start):
        name_tok = self.next()
        sig = [name_tok] + self.until({"{", "requires", "ensures"})
        sig_text = self.text(sig)
        params = re.findall(r"[(,]\s*([A-Za-z_][A-Za-z0-9_]*)\s*:", sig_text)
        has_ret = "->" in [t.text for t in sig]
        contracts = []
        while self.peek().text in ("requires", "ensures"):
            kw = self.next()
            e = self.until({";"})
            self.next()
            contracts.append((kw.text + " " + self.text(e), kw.line))
        assigned = self.assigned_names()
        script = (name_tok.text == "main" and not params and not has_ret and not self.keep_main)
        if script:
            indent = 0
        else:
            self.emit(0, ("pub " if pub else "") + "fn " + sig_text.rstrip() + ":", start.line)
            indent = 1
        pre = [(c, l) for c, l in contracts]
        declared = self.declared_names()
        for p in params:
            if p in assigned and p not in declared:
                pre.append((f"var {p} = {p}", start.line))
        self.block(indent, assigned, has_ret, pre=pre, fn_body=True)

    def assigned_names(self):
        """Names assigned (`x =` / `x op=` form) anywhere in the block starting at the
        current `{`, by scanning tokens to the matching `}`."""
        names, depth, i = set(), 0, self.pos
        prev = None
        while True:
            t = self.toks[i]
            if t.text in OPEN:
                depth += 1
            elif t.text in CLOSE:
                depth -= 1
                if depth == 0:
                    return names
            if (t.kind == "id" and (prev is None or prev.text in (";", "{", "}"))
                    and self.toks[i + 1].text == "="):
                names.add(t.text)
            prev = t
            i += 1

    def declared_names(self):
        names, depth, i = set(), 0, self.pos
        while True:
            t = self.toks[i]
            if t.text in OPEN:
                depth += 1
            elif t.text in CLOSE:
                depth -= 1
                if depth == 0:
                    return names
            if t.text == "let":
                names.add(self.toks[i + 1].text)
            i += 1

    def block(self, indent, assigned, has_ret, pre=(), fn_body=False):
        open_tok = self.expect("{")
        if indent > 0 or not fn_body:
            self.trailing_comment(open_tok)
        n0 = len(self.out)
        for p, line in pre:
            self.emit(indent, p, line)
        while self.peek().text != "}":
            self.stmt(indent, assigned, has_ret, fn_body)
        close = self.peek()
        self.flush_comments(close.start, indent)
        self.next()
        if len(self.out) == n0:
            self.emit(indent, "pass", close.line)

    def stmt(self, indent, assigned, has_ret, fn_body):
        t = self.peek()
        self.flush_comments(t.start, indent)
        if t.text in ("if", "while", "for"):
            self.next()
            head = self.until(set(), block_brace=True)
            kw = t.text
            if kw == "for":
                v = head[0].text
                if v in assigned:
                    self.warnings.append(f"line {t.line}: loop variable '{v}' is reassigned")
            self.emit(indent, f"{kw} {self.text(head)}:", t.line)
            self.block(indent + 1, assigned, has_ret)
            while kw == "if" and self.peek().text == "else":
                e = self.next()
                if self.peek().text == "if":
                    self.next()
                    head = self.until(set(), block_brace=True)
                    self.emit(indent, f"elif {self.text(head)}:", e.line)
                else:
                    self.emit(indent, "else:", e.line)
                self.block(indent + 1, assigned, has_ret)
            return
        toks = self.until({";"})
        end = self.next()
        s = self.text(toks)
        if toks[0].text == "let" and toks[1].text in assigned:
            s = "var" + s[3:]
        elif toks[0].text == "return" and len(toks) > 1 and fn_body and has_ret and indent == 1 \
                and self.peek().text == "}":
            s = self.text(toks[1:])
        else:
            s = self.compound(toks) or s
        self.emit(indent, s, t.line)
        self.trailing_comment(end)

    def compound(self, toks):
        """`T = T op rest` -> `T op= rest` when grouping is unchanged."""
        depth, eq = 0, None
        for i, t in enumerate(toks):
            if t.text in OPEN:
                depth += 1
            elif t.text in CLOSE:
                depth -= 1
            elif depth == 0 and t.text == "=":
                eq = i
                break
        if eq is None or eq == 0:
            return None
        target, rhs = toks[:eq], toks[eq + 1:]
        n = len(target)
        if len(rhs) <= n + 1 or [t.text for t in rhs[:n]] != [t.text for t in target]:
            return None
        op = rhs[n].text
        if op not in LOWER:
            return None
        rest = rhs[n + 1:]
        depth = 0
        for i, t in enumerate(rest):
            if t.text in OPEN:
                depth += 1
            elif t.text in CLOSE:
                depth -= 1
            elif depth == 0 and t.text in LOWER[op]:
                binary = i > 0 and is_operand_end(rest[i - 1])
                if binary or t.text in ("not", ".."):
                    return None
        return f"{self.text(target)} {op}= {self.text(rest)}"


NEW_BUILTINS = ("sum", "min", "max", "any", "all")


def rename_clashes(src, warnings):
    """A user fn named like a new builtin (sum, min, max, any, all) is renamed
    to `name_` everywhere (identifiers only, not strings or comments)."""
    toks, _ = tokenize(src)
    clash = {toks[i + 1].text for i, t in enumerate(toks[:-1])
             if t.text == "fn" and toks[i + 1].text in NEW_BUILTINS}
    for name in sorted(clash):
        warnings.append(f"fn '{name}' clashes with the new builtin; renamed to '{name}_'")
    for t in reversed(toks):
        if t.kind == "id" and t.text in clash:
            src = src[:t.start] + t.text + "_" + src[t.end:]
    return src


def migrate(src, keep_main=False):
    warnings = []
    src = rename_clashes(src, warnings)
    m = Migrator(src, keep_main)
    m.warnings = warnings
    out = m.program()
    return out, m.warnings


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    keep = "--keep-main" in sys.argv
    if not args:
        raise SystemExit(__doc__)
    for path in args:
        src = sys.stdin.read() if path == "-" else open(path).read()
        out, warns = migrate(src, keep)
        for w in warns:
            print(f"{path}: {w}", file=sys.stderr)
        if path == "-":
            sys.stdout.write(out)
        else:
            open(path, "w").write(out)


if __name__ == "__main__":
    main()
