#!/usr/bin/env python3
"""Run with python -m unittest discover -s tools -p 'test_limits_lsp.py'."""
import io
import json
import os
import unittest
from unittest.mock import patch
from pathlib import Path

from limits_lsp import CliIndexer, FrameError, RequestError, Server, from_lsp, read_message, to_lsp, write_message


def span(line, start, end):
    return {"start": {"line": line, "col": start}, "end": {"line": line, "col": end}}


def index_fixture(text):
    return {"schema_version": 1, "command": "index", "ok": True, "diagnostics": [], "data": {
        "schema_version": 1,
        "symbols": [
            {"id": 0, "name": "print", "kind": "builtin", "scope": 0, "declarations": [], "parameters": []},
            {"id": 1, "name": "x", "kind": "variable", "scope": 1, "declarations": [span(1, 5, 6)], "parameters": []},
            {"id": 2, "name": "helper", "kind": "function", "scope": 1, "declarations": [span(2, 4, 10)], "parameters": ["n"]},
        ],
        "scopes": [
            {"id": 0, "kind": "builtins", "parent": None, "range": None},
            {"id": 1, "kind": "module", "parent": 0, "range": {"start": {"line": 1, "col": 1}, "end": {"line": 4, "col": 1}}},
        ],
        "references": [
            {"name": "x", "range": span(3, 7, 8), "candidates": [1], "binding": 1, "scope": 1, "role": "read"},
        ],
    }}


class Framing(unittest.TestCase):
    def test_roundtrip_and_utf8_byte_length(self):
        message = {"jsonrpc": "2.0", "id": 1, "result": "😀é"}
        output = io.BytesIO()
        write_message(output, message)
        raw = output.getvalue()
        header, payload = raw.split(b"\r\n\r\n")
        self.assertEqual(int(header.split(b":")[1]), len(payload))
        self.assertEqual(read_message(io.BytesIO(raw)), message)
        self.assertIsNone(read_message(io.BytesIO()))

    def test_bad_frames_are_rejected(self):
        frames = [b"Content-Length: 2\n\n{}", b"\r\n{}", b"Content-Length: -1\r\n\r\n",
                  b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
                  b"Content-Length: 999999999\r\n\r\n", b"Content-Length: 4\r\n\r\n{}",
                  b"Content-Length: 3\r\n\r\nNaN", b"Content-Length: 2\r\n\r\n[]",
                  b"Content-Length: " + b"9" * 5000 + b"\r\n\r\n"]
        for raw in frames:
            with self.subTest(raw=raw[:80]), self.assertRaises(FrameError):
                read_message(io.BytesIO(raw))

    def test_nested_payload_and_surrogate_response_cannot_crash_transport(self):
        payload = b'{"nested":' + b"[" * 10_000 + b"0" + b"]" * 10_000 + b"}"
        raw = f"Content-Length: {len(payload)}\r\n\r\n".encode() + payload
        with self.assertRaises(FrameError):
            read_message(io.BytesIO(raw))
        out = io.BytesIO()
        write_message(out, {"jsonrpc": "2.0", "id": "\ud800", "result": None})
        self.assertEqual(read_message(io.BytesIO(out.getvalue()))["id"], "\ud800")

    def test_surrogate_pairs_and_crlf_roundtrip(self):
        source = 'let 名 = "😀é";\r\nprint(名);\n'
        for col in range(1, len(source.split("\n")[0].rstrip("\r")) + 2):
            point = {"line": 1, "col": col}
            self.assertEqual(from_lsp(source, to_lsp(source, point)), point)
        self.assertEqual(to_lsp(source, {"line": 1, "col": 11}), {"line": 0, "character": 11})
        self.assertEqual(from_lsp(source, {"line": 2, "character": 0}), {"line": 3, "col": 1})
        with self.assertRaises(RequestError):
            from_lsp("😀", {"line": 0, "character": 1})
        with self.assertRaises(RequestError):
            from_lsp("a", {"line": 0, "character": 2})


class Protocol(unittest.TestCase):
    def setUp(self):
        self.messages = []
        self.calls = []
        self.server = Server(self.indexer, self.messages.append)
        self.source = "let x = 1;\nfn helper(n) { return n; }\nprint(x);\n"
        self.request("initialize", {})
        self.notify("textDocument/didOpen", {"textDocument": {"uri": "untitled:example", "version": 1, "text": self.source}})

    def indexer(self, text):
        self.calls.append(text)
        return index_fixture(text)

    def request(self, method, params):
        self.server.handle({"jsonrpc": "2.0", "id": 7, "method": method, "params": params})
        return self.messages[-1]

    def notify(self, method, params):
        self.server.handle({"jsonrpc": "2.0", "method": method, "params": params})

    def at(self, line, character):
        return {"textDocument": {"uri": "untitled:example"}, "position": {"line": line, "character": character}}

    def test_queries_use_one_cached_index_for_document_version(self):
        result = self.request("textDocument/definition", self.at(2, 6))["result"]
        self.assertEqual(result[0]["range"], {"start": {"line": 0, "character": 4}, "end": {"line": 0, "character": 5}})
        refs = self.request("textDocument/references", {**self.at(0, 4), "context": {"includeDeclaration": True}})["result"]
        self.assertEqual(len(refs), 2)
        symbols = self.request("textDocument/documentSymbol", {"textDocument": {"uri": "untitled:example"}})["result"]
        self.assertEqual([s["name"] for s in symbols], ["x", "helper"])
        hover = self.request("textDocument/hover", self.at(1, 3))["result"]
        self.assertIn("fn helper(n)", hover["contents"]["value"])
        items = self.request("textDocument/completion", self.at(3, 0))["result"]["items"]
        self.assertIn("x", [i["label"] for i in items])
        self.assertIn("print", [i["label"] for i in items])
        self.assertEqual(len(self.calls), 1)

    def test_full_sync_reindexes_and_stale_edits_do_not_replace_cache(self):
        params = {"textDocument": {"uri": "untitled:example", "version": 2}, "contentChanges": [{"text": self.source + "\n"}]}
        self.notify("textDocument/didChange", params)
        self.assertEqual(len(self.calls), 2)
        self.notify("textDocument/didChange", params)
        self.assertEqual(len(self.calls), 2)
        self.assertEqual(self.server.documents["untitled:example"].version, 2)
        self.notify("textDocument/didClose", {"textDocument": {"uri": "untitled:example"}})
        self.assertEqual(self.messages[-1]["params"]["diagnostics"], [])
        self.assertFalse(self.server.documents)

    def test_cache_budget_and_malformed_notifications_are_bounded(self):
        before = len(self.calls)
        for method, params in [
            ("textDocument/didOpen", {"textDocument": []}),
            ("textDocument/didChange", {"textDocument": {"uri": "untitled:example", "version": 2}, "contentChanges": ["bad"]}),
            ("textDocument/didClose", {"textDocument": []}),
        ]:
            self.notify(method, params)
        self.assertEqual(len(self.calls), before)
        self.assertEqual(self.server.documents["untitled:example"].version, 1)
        with patch("limits_lsp.MAX_CACHE_BYTES", 1):
            self.notify("textDocument/didChange", {"textDocument": {"uri": "untitled:example", "version": 2}, "contentChanges": [{"text": self.source}]})
        doc = self.server.documents["untitled:example"]
        self.assertIsNone(doc.envelope["data"])
        self.assertEqual(doc.index_bytes, 0)
        self.assertEqual(self.messages[-1]["params"]["diagnostics"][0]["message"], "Semantic index cache budget exceeded")

    def test_unknown_methods_and_shutdown(self):
        self.assertEqual(self.request("not/a/method", {})["error"]["code"], -32601)
        self.assertIsNone(self.request("shutdown", {})["result"])
        self.assertEqual(self.request("textDocument/completion", self.at(0, 0))["error"]["code"], -32600)
        self.notify("exit", {})
        self.assertTrue(self.server.exit_requested)

    def test_invalid_ids_and_params_do_not_escape(self):
        self.server.handle({"jsonrpc": "2.0", "id": {}, "method": "initialize"})
        self.assertIsNone(self.messages[-1]["id"])
        self.assertEqual(self.messages[-1]["error"]["code"], -32600)
        self.server.handle({"jsonrpc": "2.0", "id": "\ud800", "method": "textDocument/documentSymbol", "params": {}})
        self.assertIsNone(self.messages[-1]["id"])
        self.assertEqual(self.messages[-1]["error"]["code"], -32600)
        self.assertEqual(self.request("textDocument/definition", self.at(0, 999))["error"]["code"], -32602)
        self.assertEqual(self.request("textDocument/definition", {"textDocument": []})["error"]["code"], -32602)

    def test_parse_error_publishes_diagnostic_and_only_global_completion(self):
        self.server.indexer = lambda _: {"schema_version": 1, "command": "index", "data": None,
                                         "diagnostics": [{"kind": "parse", "message": "missing ;", "line": 1, "col": 2}]}
        self.notify("textDocument/didChange", {"textDocument": {"uri": "untitled:example", "version": 2}, "contentChanges": [{"text": "x"}]})
        diagnostic = self.messages[-1]["params"]["diagnostics"][0]
        self.assertEqual(diagnostic["message"], "missing ;")
        self.assertEqual(diagnostic["range"]["start"], {"line": 0, "character": 1})
        self.assertEqual(self.request("textDocument/definition", self.at(0, 0))["result"], [])
        self.assertEqual(self.request("textDocument/completion", self.at(0, 1))["result"], {"isIncomplete": False, "items": []})


class ActualCli(unittest.TestCase):
    def test_real_cli_index_roundtrip_when_configured(self):
        executable = os.environ.get("LIM_BIN")
        if not executable:
            self.skipTest("Set LIM_BIN to test the real compiled CLI")
        response = CliIndexer(str(Path(executable).resolve()))("let x=1; print(x);\n")
        self.assertEqual(response["schema_version"], 1)
        self.assertEqual(response["command"], "index")
        self.assertTrue(any(s["name"] == "x" for s in response["data"]["symbols"]))
        self.assertTrue(response["data"]["references"])

    def test_real_editor_positions_after_astral_unicode(self):
        executable = os.environ.get("LIM_BIN")
        if not executable:
            self.skipTest("Set LIM_BIN to test the real compiled CLI")
        messages = []
        server = Server(CliIndexer(str(Path(executable).resolve())), messages.append)
        server.handle({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}})
        source = 'let 名="😀"; print(名);\n'
        server.handle({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {"uri": "untitled:unicode", "version": 1, "text": source}}})
        scalar = source.rindex("名")
        utf16 = len(source[:scalar].encode("utf-16-le")) // 2
        server.handle({"jsonrpc": "2.0", "id": 2, "method": "textDocument/definition", "params": {"textDocument": {"uri": "untitled:unicode"}, "position": {"line": 0, "character": utf16}}})
        self.assertEqual(messages[-1]["result"], [{"uri": "untitled:unicode", "range": {"start": {"line": 0, "character": 4}, "end": {"line": 0, "character": 5}}}])


if __name__ == "__main__":
    unittest.main()
