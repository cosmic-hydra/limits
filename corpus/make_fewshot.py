#!/usr/bin/env python3
"""Regenerate fewshot.md from the verified pairs listed in FEWSHOT."""
import json, os

HERE = os.path.dirname(os.path.abspath(__file__))
FEWSHOT = [  # (pair, what it teaches)
    ("app_payments", "enum with payloads, exhaustive match, Option from a str match"),
    ("app_kv_parse", "Result, the ? operator, structs, any() over a comprehension"),
    ("app_temps", "Option and ? on parse_float, float arithmetic, no implicit int->float"),
    ("app_caesar", "args capability, fn main(env), match on Option with early return, inline test"),
    ("app_data_dir", "fs.read narrowed with fs.sub, passing an Fs capability to a fn"),
    ("app_env_greeting", "env vars capability, match as a value with a block arm"),
    ("app_shuffle", "inout scalars and lists, requires contract, nested loops building a list"),
    ("app_matrix_rotate", "inout [[int]], in-place swaps, comprehension over a range"),
]


def main():
    idx = {}
    for line in open(os.path.join(HERE, "index.jsonl")):
        r = json.loads(line)
        idx[r["name"]] = r
    out = ["# Limits few-shot pairs", "",
           "Eight short Python -> Limits translations from `corpus/pairs/`, each verified by",
           "`verify.py` (same stdout as the Python program on every test input). Use them as",
           "retrieval examples next to `native/LLM.md`. Regenerate with `python3 make_fewshot.py`.", ""]
    for name, teaches in FEWSHOT:
        d = os.path.join(HERE, "pairs", name)
        r = idx[name]
        assert r["status"] == "pass", name
        py = open(os.path.join(d, "prog.py")).read().rstrip("\n")
        lim = open(os.path.join(d, "prog.lim")).read().rstrip("\n")
        assert len(py.splitlines()) < 40 and len(lim.splitlines()) < 40, name
        out += [f"## {name}", "", f"Teaches: {teaches}. "
                f"Tokens (o200k): Python {r['tokens_py']}, Limits {r['tokens_lim']}.", "",
                "```python", py, "```", "", "```limits", lim, "```", ""]
    with open(os.path.join(HERE, "fewshot.md"), "w") as f:
        f.write("\n".join(out))


if __name__ == "__main__":
    main()
