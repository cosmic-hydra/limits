#!/usr/bin/env python3
"""Check that .lim files parse according to grammar.ebnf.

Usage: python3 tools/grammar_check.py [--grammar grammar.ebnf] FILE.lim...
Exit code 0 if every file matches the grammar's `file` rule, 1 otherwise.

The lexer is a port of src/lexer.rs (NEWLINE / INDENT / DEDENT layout
tokens). The grammar is read from the EBNF file and matched with a
memoized all-alternatives recursive matcher (each rule at each position
yields the set of positions where it can end), so ambiguity is fine; the
grammar must not be left recursive. This checks the grammar, not the
compiler: it does not apply the contextual rules listed in the EBNF header.
"""
import re
import sys

KEYWORDS = set(
    "let var fn return if elif else while for in true false and or not break "
    "continue pass struct import pub enum match".split()
)
SYMS = [
    "..", "==", "!=", "<=", ">=", "->", "+=", "-=", "*=", "/=", "%=", "(", ")", "{", "}",
    "[", "]", ",", ";", ":", "+", "-", "*", "/", "%", "=", "<", ">", ".", "?", "&",
]


class LexError(Exception):
    pass


def lex(src):
    """Tokens as (kind, text, line). Mirrors src/lexer.rs."""
    out, indents, depth = [], [0], 0
    i, line, at_start, has_tok = 0, 1, True, False
    n = len(src)
    while i < n:
        if at_start and depth == 0:
            k = 0
            while i + k < n and src[i + k] == " ":
                k += 1
            nxt = src[i + k] if i + k < n else ""
            if nxt == "\t":
                raise LexError(f"{line}: tab")
            if nxt not in ("", "\n", "\r", "#"):
                top = indents[-1]
                if k > top:
                    if k != top + 4:
                        raise LexError(f"{line}: bad indent")
                    indents.append(k)
                    out.append(("INDENT", "", line))
                else:
                    while k < indents[-1]:
                        indents.pop()
                        out.append(("DEDENT", "", line))
                    if k != indents[-1]:
                        raise LexError(f"{line}: bad unindent")
            at_start = False
            i += k
            continue
        c = src[i]
        if c == "\n":
            if depth == 0:
                if has_tok:
                    out.append(("NEWLINE", "", line))
                    has_tok = False
                at_start = True
            i += 1
            line += 1
            continue
        if c == "\t":
            raise LexError(f"{line}: tab")
        if c in " \r\f\v":
            i += 1
            continue
        if c == "#":
            while i < n and src[i] != "\n":
                i += 1
            continue
        if c.isascii() and c.isdigit():
            m = re.compile(r"[0-9][0-9_]*").match(src, i)
            j = m.end()
            if j + 1 < n and src[j] == "." and src[j + 1].isascii() and src[j + 1].isdigit():
                j = re.compile(r"[0-9]+").match(src, j + 1).end()
                out.append(("FLOAT", src[i:j], line))
            else:
                out.append(("INT", src[i:j], line))
            i = j
        elif c.isascii() and (c.isalpha() or c == "_"):
            j = re.compile(r"[A-Za-z0-9_]*").match(src, i + 1).end()
            w = src[i:j]
            out.append(("KW" if w in KEYWORDS else "IDENT", w, line))
            i = j
        elif c == '"':
            j = i + 1
            while True:
                if j >= n or src[j] == "\n":
                    raise LexError(f"{line}: unterminated string")
                if src[j] == '"':
                    break
                if src[j] == "\\":
                    if j + 1 >= n or src[j + 1] not in 'nt\\"':
                        raise LexError(f"{line}: unknown escape")
                    j += 2
                else:
                    j += 1
            out.append(("STRING", src[i : j + 1], line))
            i = j + 1
        else:
            s = next((s for s in SYMS if src.startswith(s, i)), None)
            if s is None:
                raise LexError(f"{line}: unexpected character {c!r}")
            if s in "([{":
                depth += 1
            elif s in ")]}":
                depth = max(0, depth - 1)
            out.append(("SYM", s, line))
            i += len(s)
        has_tok = True
    if has_tok:
        out.append(("NEWLINE", "", line))
    out += [("DEDENT", "", line)] * (len(indents) - 1)
    return out


# ---- EBNF reader: rule = alts ; alts = seq ("|" seq)* ; seq = item* ;
# item = atom ("?" | "*" | "+")? ; atom = "lit" | NAME | "(" alts ")".
def read_grammar(text):
    text = re.sub(r"\(\*.*?\*\)", " ", text, flags=re.S)
    toks = re.findall(r'"[^"]*"|[A-Za-z_][A-Za-z_0-9]*|[=;|()?*+]', text)
    rules, pos = {}, 0

    def alts():
        nonlocal pos
        out = [seq()]
        while pos < len(toks) and toks[pos] == "|":
            pos += 1
            out.append(seq())
        return ("alt", out)

    def seq():
        nonlocal pos
        items = []
        while pos < len(toks) and toks[pos] not in ("|", ")", ";"):
            t = toks[pos]
            pos += 1
            if t == "(":
                a = alts()
                assert toks[pos] == ")", toks[pos - 3 : pos + 3]
                pos += 1
            elif t.startswith('"'):
                a = ("lit", t[1:-1])
            else:
                a = ("ref", t)
            if pos < len(toks) and toks[pos] in ("?", "*", "+"):
                a = (toks[pos], a)
                pos += 1
            items.append(a)
        return ("seq", items)

    while pos < len(toks):
        name = toks[pos]
        assert toks[pos + 1] == "=", f"expected '=' after {name}"
        pos += 2
        rules[name] = alts()
        assert toks[pos] == ";", f"expected ';' ending {name}, got {toks[pos]}"
        pos += 1
    return rules


CLASSES = {"IDENT", "INT", "FLOAT", "STRING", "NEWLINE", "INDENT", "DEDENT"}


class Matcher:
    def __init__(self, rules, toks):
        self.rules, self.toks, self.memo = rules, toks, {}
        self.far = 0  # furthest token any terminal matched (for messages)

    def ends(self, node, p):
        kind = node[0]
        if kind == "ref":
            name = node[1]
            if name in CLASSES:
                ok = p < len(self.toks) and self.toks[p][0] == name
                return self.hit(p) if ok else frozenset()
            key = (name, p)
            if key not in self.memo:
                self.memo[key] = frozenset()  # guards (unsupported) left recursion
                self.memo[key] = self.ends(self.rules[name], p)
            return self.memo[key]
        if kind == "lit":
            ok = (
                p < len(self.toks)
                and self.toks[p][0] in ("KW", "SYM", "IDENT")
                and self.toks[p][1] == node[1]
            )
            return self.hit(p) if ok else frozenset()
        if kind == "alt":
            r = set()
            for a in node[1]:
                r |= self.ends(a, p)
            return frozenset(r)
        if kind == "seq":
            cur = {p}
            for it in node[1]:
                nxt = set()
                for q in cur:
                    nxt |= self.ends(it, q)
                cur = nxt
                if not cur:
                    break
            return frozenset(cur)
        if kind == "?":
            return frozenset({p} | self.ends(node[1], p))
        if kind in ("*", "+"):
            first = self.ends(node[1], p)
            seen = set(first) if kind == "+" else set(first) | {p}
            todo = list(first)
            while todo:
                q = todo.pop()
                for e in self.ends(node[1], q):
                    if e not in seen:
                        seen.add(e)
                        todo.append(e)
            return frozenset(seen)
        raise ValueError(kind)

    def hit(self, p):
        self.far = max(self.far, p + 1)
        return frozenset({p + 1})


def check(rules, path):
    src = open(path, encoding="utf-8").read()
    try:
        toks = lex(src)
    except LexError as e:
        return f"lex error: {e}"
    m = Matcher(rules, toks)
    if len(toks) in m.ends(("ref", "file"), 0):
        return None
    t = toks[min(m.far, len(toks) - 1)] if toks else ("EOF", "", 0)
    return f"no parse; furthest token matched ends before line {t[2]} ({t[0]} {t[1]!r})"


def main():
    args = sys.argv[1:]
    gpath = "grammar.ebnf"
    if args[:1] == ["--grammar"]:
        gpath, args = args[1], args[2:]
    rules = read_grammar(open(gpath).read())
    missing = {
        r
        for body in rules.values()
        for r in re.findall(r"'ref', '([A-Za-z_]+)'", repr(body))
        if r not in rules and r not in CLASSES
    }
    if missing:
        sys.exit(f"undefined rules: {sorted(missing)}")
    sys.setrecursionlimit(100000)
    bad = 0
    for f in args:
        err = check(rules, f)
        if err:
            bad += 1
            print(f"FAIL {f}: {err}")
    print(f"{len(args) - bad} of {len(args)} files match the grammar")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
