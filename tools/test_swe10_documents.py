"""Editor document, position and lexical-query regression tests (stdlib only)."""
import copy
import os
from pathlib import Path
import unittest
from unittest.mock import patch

import limits_lsp
from limits_lsp import CliIndexer, RequestError, Server, from_lsp, to_lsp
from test_limits_lsp import index_fixture, span


class EditorHarness:
    def make_server(self, indexer):
        self.messages = []
        self.server = Server(indexer, self.messages.append)
        self.request("initialize", {})

    def request(self, method, params):
        self.server.handle({"jsonrpc": "2.0", "id": 23, "method": method, "params": params})
        return self.messages[-1]

    def notify(self, method, params):
        self.server.handle({"jsonrpc": "2.0", "method": method, "params": params})

    def open(self, text, uri="untitled:document", version=1):
        self.notify("textDocument/didOpen", {"textDocument": {"uri": uri, "text": text, "version": version}})

    def change(self, text, uri="untitled:document", version=2):
        self.notify("textDocument/didChange", {"textDocument": {"uri": uri, "version": version},
                                                "contentChanges": [{"text": text}]})

    def at(self, line, character, uri="untitled:document"):
        return {"textDocument": {"uri": uri}, "position": {"line": line, "character": character}}


class PositionConversions(unittest.TestCase):
    def test_empty_eof_combining_marks_astral_and_crlf(self):
        source = "é😀e\u0301\r\nx\n"
        cases = [({"line": 1, "col": 1}, {"line": 0, "character": 0}),
                 ({"line": 1, "col": 2}, {"line": 0, "character": 1}),
                 ({"line": 1, "col": 3}, {"line": 0, "character": 3}),
                 ({"line": 1, "col": 5}, {"line": 0, "character": 5}),
                 ({"line": 2, "col": 2}, {"line": 1, "character": 1}),
                 ({"line": 3, "col": 1}, {"line": 2, "character": 0})]
        for scalar, utf16 in cases:
            with self.subTest(scalar=scalar):
                self.assertEqual(to_lsp(source, scalar), utf16)
                self.assertEqual(from_lsp(source, utf16), scalar)
        self.assertEqual(from_lsp("", {"line": 0, "character": 0}), {"line": 1, "col": 1})
        with self.assertRaises(RequestError):
            from_lsp(source, {"line": 0, "character": 2})

    def test_cr_lf_crlf_share_editor_scalar_lines(self):
        # Both the CLI and LSP treat standalone CR as a line break, while a
        # CRLF pair remains one line break. Columns differ after astral scalars.
        source = "😀\ré\r\nz\n"
        cases = [({"line": 1, "col": 1}, {"line": 0, "character": 0}),
                 ({"line": 1, "col": 2}, {"line": 0, "character": 2}),
                 ({"line": 2, "col": 1}, {"line": 1, "character": 0}),
                 ({"line": 2, "col": 2}, {"line": 1, "character": 1}),
                 ({"line": 3, "col": 1}, {"line": 2, "character": 0}),
                 ({"line": 4, "col": 1}, {"line": 3, "character": 0})]
        for scalar, utf16 in cases:
            with self.subTest(scalar=scalar):
                self.assertEqual(to_lsp(source, scalar), utf16)
                self.assertEqual(from_lsp(source, utf16), scalar)
        self.assertEqual(to_lsp("a\r", {"line": 2, "col": 1}), {"line": 1, "character": 0})
        self.assertEqual(from_lsp("a\r", {"line": 1, "character": 0}), {"line": 2, "col": 1})


class DocumentLifecycle(EditorHarness, unittest.TestCase):
    source = "let x = 1;\nfn helper(n) { return n; }\nprint(x);\n"

    def setUp(self):
        self.calls = []

        def indexer(text):
            self.calls.append(text)
            return copy.deepcopy(index_fixture(text))

        self.make_server(indexer)

    def test_index_failure_replaces_semantics_and_close_allows_version_reset(self):
        self.open(self.source)
        self.assertTrue(self.request("textDocument/definition", self.at(2, 6))["result"])

        def failed_indexer(text):
            self.calls.append(text)
            raise RequestError(-32603, "index unavailable")

        self.server.indexer = failed_indexer
        self.change("broken", version=5)
        publication = self.messages[-1]["params"]
        self.assertEqual(publication["version"], 5)
        self.assertEqual(publication["diagnostics"][0]["message"], "index unavailable")
        self.assertEqual(self.request("textDocument/definition", self.at(0, 0))["result"], [])
        published = len(self.messages)
        self.change(self.source, version=4)
        self.change(self.source, version=5)
        self.assertEqual(len(self.messages), published)
        self.assertEqual(self.calls, [self.source, "broken"])
        self.assertEqual(self.server.documents["untitled:document"].text, "broken")

        self.notify("textDocument/didClose", {"textDocument": {"uri": "untitled:document"}})
        self.assertEqual(self.messages[-1]["params"], {"uri": "untitled:document", "diagnostics": []})
        self.assertEqual(self.request("textDocument/definition", self.at(0, 0))["error"]["code"], -32602)
        self.server.indexer = lambda text: copy.deepcopy(index_fixture(text))
        self.open(self.source, version=1)
        self.assertEqual(self.messages[-1]["params"]["version"], 1)
        self.assertEqual(self.messages[-1]["params"]["diagnostics"], [])
        self.assertTrue(self.request("textDocument/definition", self.at(2, 6))["result"])

    def test_unopened_uris_cannot_query_or_mutate_an_open_document(self):
        self.open(self.source)
        original = self.server.documents["untitled:document"]
        for method in ("textDocument/definition", "textDocument/references", "textDocument/hover",
                       "textDocument/completion", "textDocument/documentSymbol"):
            with self.subTest(method=method):
                response = self.request(method, self.at(2, 6, "file:///not-open.lim"))
                self.assertEqual(response["error"]["code"], -32602)
        count = len(self.messages)
        self.change("replacement", uri="file:///not-open.lim", version=2)
        self.notify("textDocument/didClose", {"textDocument": {"uri": "file:///not-open.lim"}})
        self.assertEqual(len(self.messages), count)
        self.assertEqual(self.calls, [self.source])
        self.assertIs(self.server.documents["untitled:document"], original)

    def test_invalid_full_sync_batches_are_atomic(self):
        self.open(self.source)
        original = self.server.documents["untitled:document"]
        count = len(self.messages)
        for changes in ([], [{"text": "one"}, {"text": "two"}],
                        [{"text": "partial", "range": {"start": {"line": 0, "character": 0},
                                                                  "end": {"line": 0, "character": 1}}}],
                        [{"text": "\ud800"}], [{"text": None}]):
            with self.subTest(changes=repr(changes)):
                self.notify("textDocument/didChange", {"textDocument": {"uri": "untitled:document", "version": 2},
                                                       "contentChanges": changes})
                self.assertIs(self.server.documents["untitled:document"], original)
        self.assertEqual(self.calls, [self.source])
        self.assertEqual(len(self.messages), count)
        self.change(self.source + "\n", version=2)
        self.assertEqual(self.calls, [self.source, self.source + "\n"])
        self.assertEqual(self.server.documents["untitled:document"].version, 2)

    def test_document_and_uri_budgets_count_utf8_bytes_and_free_on_close(self):
        with patch("limits_lsp.MAX_DOCUMENT_BYTES", 4), patch("limits_lsp.MAX_DOCUMENTS", 1), \
                patch("limits_lsp.MAX_URI_BYTES", 6):
            self.open("😀", uri="u:😀")
            self.assertIn("u:😀", self.server.documents)
            self.open("", uri="u:other")
            self.assertNotIn("u:other", self.server.documents)
            self.change("ééé", uri="u:😀", version=2)
            self.assertEqual(self.server.documents["u:😀"].text, "😀")
            self.change("éé", uri="u:😀", version=2)
            self.assertEqual(self.server.documents["u:😀"].text, "éé")
            self.notify("textDocument/didClose", {"textDocument": {"uri": "u:😀"}})
            self.open("", uri="u:😀x")
            self.assertNotIn("u:😀x", self.server.documents)
            self.open("😀", uri="u:new", version=-1)
            self.assertEqual(self.server.documents["u:new"].version, -1)
        self.assertEqual(self.calls, ["😀", "éé", "😀"])

    def test_cache_budget_replaces_old_semantics_and_recovers_after_close(self):
        self.open(self.source, uri="u:first")
        size = self.server.documents["u:first"].index_bytes
        self.assertGreater(size, 0)
        with patch("limits_lsp.MAX_CACHE_BYTES", size):
            self.open(self.source, uri="u:second")
            second = self.server.documents["u:second"]
            self.assertIsNone(second.envelope["data"])
            self.assertEqual(second.index_bytes, 0)
            self.assertEqual(self.messages[-1]["params"]["diagnostics"][0]["message"],
                             "Semantic index cache budget exceeded")
            self.assertEqual(self.request("textDocument/definition", self.at(2, 6, "u:second"))["result"], [])
            self.assertTrue(self.request("textDocument/definition", self.at(2, 6, "u:first"))["result"])
            self.notify("textDocument/didClose", {"textDocument": {"uri": "u:first"}})
            self.change(self.source, uri="u:second", version=2)
            self.assertEqual(self.server.documents["u:second"].index_bytes, size)
            self.assertEqual(self.messages[-1]["params"]["diagnostics"], [])
            self.assertTrue(self.request("textDocument/definition", self.at(2, 6, "u:second"))["result"])

    def test_result_cap_bounds_conversion_work_and_reuses_document_lines(self):
        source = "let x=1;\n" * 64
        envelope = index_fixture(source)
        envelope["data"]["symbols"] = envelope["data"]["symbols"][:2]
        envelope["data"]["symbols"][1]["declarations"] = [span(line, 5, 6) for line in range(1, 65)]
        envelope["data"]["scopes"][1]["range"]["end"] = {"line": 65, "col": 1}
        envelope["data"]["references"] = []
        self.server.indexer = lambda text: envelope
        self.open(source)
        with patch("limits_lsp.MAX_QUERY_RESULTS", 3), \
                patch("limits_lsp._lines", wraps=limits_lsp._lines) as lines, \
                patch("limits_lsp._range", wraps=limits_lsp._range) as ranges:
            result = self.request("textDocument/documentSymbol", {"textDocument": {"uri": "untitled:document"}})["result"]
            self.assertEqual([item["location"]["range"]["start"] for item in result],
                             [{"line": line, "character": 4} for line in range(3)])
            self.assertLessEqual(ranges.call_count, 3)
            self.assertLessEqual(lines.call_count, 1)

    def test_diagnostic_ranges_share_one_version_line_cache(self):
        envelope = {"schema_version": 1, "command": "index", "data": None,
                    "diagnostics": [{"kind": "check", "message": "unresolved", "line": 1, "col": 1}] * 32}
        self.server.indexer = lambda text: envelope
        with patch("limits_lsp._lines", wraps=limits_lsp._lines) as lines:
            self.open("a")
            diagnostics = self.messages[-1]["params"]["diagnostics"]
            self.assertEqual(len(diagnostics), 32)
            self.assertEqual(diagnostics[0]["range"], {"start": {"line": 0, "character": 0},
                                                       "end": {"line": 0, "character": 1}})
            self.change("😀")
            self.assertEqual(self.messages[-1]["params"]["diagnostics"][0]["range"],
                             {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 2}})
            self.assertLessEqual(lines.call_count, 2)


class RealEditorQueries(EditorHarness, unittest.TestCase):
    def setUp(self):
        executable = os.environ.get("LIM_BIN")
        if not executable:
            self.skipTest("Set LIM_BIN to test the real compiled CLI")
        self.make_server(CliIndexer(str(Path(executable).resolve())))

    def test_mixed_line_endings_definition_ranges_and_eof_completion(self):
        self.open('let x=1;\r\nlet 名="😀";\rprint(名);\nprint(x);\r')
        self.assertEqual(self.messages[-1]["params"]["diagnostics"], [])
        for line, name, declaration_line in ((2, "名", 1), (3, "x", 0)):
            with self.subTest(name=name):
                response = self.request("textDocument/definition", self.at(line, 6))
                self.assertEqual(response["result"], [{"uri": "untitled:document", "range": {
                    "start": {"line": declaration_line, "character": 4},
                    "end": {"line": declaration_line, "character": 5}}}])
        items = self.request("textDocument/completion", self.at(4, 0))["result"]["items"]
        self.assertTrue({"x", "名", "print"}.issubset({item["label"] for item in items}))

    def test_scope_visibility_shadowing_references_and_eof(self):
        source = ("let outer=1;\nlet shared=2;\nfn f(p) {\n  let inner=3;\n  let shared=4;\n"
                  "  print(shared);\n}\nfn g(q) { let secret=1; return q; }\nprint(shared);\n")
        self.open(source)
        inside = self.request("textDocument/completion", self.at(5, 2))["result"]["items"]
        names = [item["label"] for item in inside]
        self.assertTrue({"p", "inner", "outer", "shared", "f", "g", "print"}.issubset(names))
        self.assertEqual(names.count("shared"), 1)
        self.assertTrue({"q", "secret"}.isdisjoint(names))
        outside = self.request("textDocument/completion", self.at(9, 0))["result"]["items"]
        self.assertTrue({"p", "inner", "q", "secret"}.isdisjoint(item["label"] for item in outside))
        # References retain all possible lexical bindings in nearest-scope
        # order; the preferred local definition must be the first jump target.
        definitions = self.request("textDocument/definition", self.at(5, 8))["result"]
        self.assertEqual([item["range"]["start"] for item in definitions],
                         [{"line": 4, "character": 6}, {"line": 1, "character": 4}])
        refs = self.request("textDocument/references", {**self.at(4, 6), "context": {"includeDeclaration": True}})["result"]
        self.assertEqual([ref["range"]["start"] for ref in refs],
                         [{"line": 4, "character": 6}, {"line": 5, "character": 8}])

    def test_diagnostic_highlights_one_astral_scalar_and_clears_on_change(self):
        source = 'let 名="😀";\rprint(𐐀);'
        self.open(source)
        diagnostic = self.messages[-1]["params"]["diagnostics"][0]
        self.assertEqual(diagnostic["range"], {"start": {"line": 1, "character": 6},
                                               "end": {"line": 1, "character": 8}})
        self.change('let 名="😀";\rprint(名);')
        self.assertEqual(self.messages[-1]["params"]["version"], 2)
        self.assertEqual(self.messages[-1]["params"]["diagnostics"], [])

    def test_completion_prefix_keeps_alphabetic_combining_marks(self):
        # U+0345 has Unicode's Alphabetic property and is accepted by Rust's
        # lexer, although Python's str.isalnum() reports False for the mark.
        name = "a\u0345"
        source = f"let {name}=1; let other=2; print({name});"
        self.open(source)
        self.assertEqual(self.messages[-1]["params"]["diagnostics"], [])
        prefix_end = source.rindex(name) + len(name)
        character = len(source[:prefix_end].encode("utf-16-le")) // 2
        items = self.request("textDocument/completion", self.at(0, character))["result"]["items"]
        self.assertEqual([item["label"] for item in items], [name])


if __name__ == "__main__":
    unittest.main()
