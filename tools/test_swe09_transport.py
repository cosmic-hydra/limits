"""Framed JSON-RPC and bounded CLI pipe lifecycle regressions (stdlib only)."""
import copy
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

from limits_lsp import CliIndexer, FrameError, RequestError, ResponseError, Server, _SourceMap, read_message, write_message


def frame(payload):
    return b"Content-Length: " + str(len(payload)).encode("ascii") + b"\r\n\r\n" + payload


def envelope():
    return {"schema_version": 1, "command": "index", "data": None, "diagnostics": []}


class Transport(unittest.TestCase):
    def test_duplicate_members_and_nonfinite_numbers_are_rejected(self):
        for payload in (
            b'{"jsonrpc":"2.0","id":1,"id":2,"method":"initialize"}',
            b'{"params":{"text":"first","text":"second"}}',
            b'{"id":NaN}', b'{"id":Infinity}', b'{"params":{"number":1e999}}',
            b'{"params":{"number":-1e999}}',
        ):
            with self.subTest(payload=payload), self.assertRaises(FrameError):
                read_message(io.BytesIO(frame(payload)))

    def test_invalid_header_names_and_case_insensitive_duplicates(self):
        for header in (
            b"Content-Length : 2\r\nContent-Length: 2\r\n",
            b" Content-Length: 2\r\n", b": ignored\r\nContent-Length: 2\r\n",
            b"Content-Length: 2\r\ncontent-length: 2\r\n",
            b"Content-Length: 2\r\nCONTENT-LENGTH: 3\r\n",
            b"Content-Length: +2\r\n", b"Content-Length: 2.0\r\n",
        ):
            with self.subTest(header=header), self.assertRaises(FrameError):
                read_message(io.BytesIO(header + b"\r\n{}"))
        self.assertEqual(read_message(io.BytesIO(b"content-length: 2\r\nX-Extra: ignored\r\n\r\n{}")), {})

    def test_short_payload_reads_preserve_following_frame(self):
        class ShortReads(io.BytesIO):
            def read(self, size=-1):
                return super().read(min(size, 3))

        first, second = {"method": "test", "params": {"text": "😀é"}}, {"id": 2}
        stream = ShortReads(frame(json.dumps(first).encode()) + frame(json.dumps(second).encode()))
        self.assertEqual(read_message(stream), first)
        self.assertEqual(read_message(stream), second)
        self.assertIsNone(read_message(stream))

    def test_size_utf8_truncation_and_depth_fail_closed(self):
        with patch("limits_lsp.MAX_MESSAGE_BYTES", 16):
            with self.assertRaises(FrameError):
                read_message(io.BytesIO(b"Content-Length: 17\r\n\r\n"))
            with self.assertRaises(ResponseError):
                write_message(io.BytesIO(), {"text": "x" * 32})
        for payload in (b'{"text":"\xff"}', b"{} trailing", b'{"text":', b"[]"):
            with self.subTest(payload=payload), self.assertRaises(FrameError):
                read_message(io.BytesIO(frame(payload)))
        with self.assertRaises(FrameError):
            read_message(io.BytesIO(frame(b'{"x":' + b"[" * 5000 + b"0" + b"]" * 5000 + b"}")))
        with self.assertRaises(FrameError):
            read_message(io.BytesIO(b"Content-Length: 10\r\n\r\n{}"))

    def test_depth_limit_ignores_brackets_quotes_and_escapes_inside_strings(self):
        message = {"text": '[{\\"' * 5000, "value": [[0]]}
        self.assertEqual(read_message(io.BytesIO(frame(json.dumps(message).encode()))), message)
        with patch("limits_lsp.MAX_JSON_DEPTH", 3):
            self.assertEqual(read_message(io.BytesIO(frame(b'{"value":[[0]]}'))), {"value": [[0]]})
            with self.assertRaises(FrameError):
                read_message(io.BytesIO(frame(b'{"value":[[[0]]]}')))

    def test_ids_are_checked_without_dispatching(self):
        for ident in (None, True, 1.5, [], {}, "\ud800", "x" * 4097):
            messages = []
            server = Server(lambda _: self.fail("Unexpected index call"), messages.append)
            server.handle({"jsonrpc": "2.0", "id": ident, "method": "initialize"})
            self.assertFalse(server.initialized)
            self.assertEqual(messages[-1]["error"]["code"], -32600)
            self.assertIsNone(messages[-1]["id"])
        for ident in (0, -1, "", "😀" * 1024):
            messages = []
            server = Server(lambda _: {}, messages.append)
            server.handle({"jsonrpc": "2.0", "id": ident, "method": "initialize"})
            self.assertEqual(messages[-1]["id"], ident)
            self.assertIn("result", messages[-1])

    def test_invalid_frame_main_returns_one_parse_error_without_traceback(self):
        result = subprocess.run([sys.executable, str(Path(__file__).with_name("limits_lsp.py"))],
                                input=frame(b'{"id":1,"id":2}'), capture_output=True, timeout=3)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stderr, b"")
        response = read_message(io.BytesIO(result.stdout))
        self.assertEqual(response["error"]["code"], -32700)
        self.assertIsNone(response["id"])

    def test_repeated_shutdown_is_an_invalid_request(self):
        messages = []
        server = Server(lambda _: {}, messages.append)
        for method in ("initialize", "shutdown", "shutdown"):
            server.handle({"jsonrpc": "2.0", "id": 1, "method": method})
        self.assertEqual(messages[-1]["error"]["code"], -32600)

    def test_cached_astral_offsets_are_sparse_and_reused(self):
        positions = _SourceMap("x" * 100_000 + "😀" + "é" + "😀")
        point = {"line": 1, "col": 100_004}
        self.assertEqual(positions.to_lsp(point), {"line": 0, "character": 100_005})
        offsets = positions.astrals[0]
        self.assertEqual(list(offsets), [100_000, 100_002])
        for _ in range(8):
            self.assertEqual(positions.from_lsp(positions.to_lsp(point)), point)
        self.assertIs(positions.astrals[0], offsets)
        for units in (100_001, 100_004):
            with self.assertRaisesRegex(RequestError, "surrogate pair"):
                positions.from_lsp({"line": 0, "character": units})


@unittest.skipUnless(os.name == "posix", "Executable script CLI fixtures require POSIX")
class BackendPipes(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.path = Path(self.temp.name) / "trusted-indexer"

    def tearDown(self):
        self.temp.cleanup()

    def indexer(self, code, timeout=1.0):
        self.path.write_text(f"#!{sys.executable}\n" + code, encoding="utf-8")
        self.path.chmod(0o700)
        return CliIndexer(str(self.path), timeout=timeout)

    def assert_no_workers(self):
        self.assertEqual([t.name for t in threading.enumerate() if t.name.startswith("limits-index-")], [])

    def test_full_duplex_large_source_and_output(self):
        result = self.indexer(
            "import json, sys\n"
            "# Writing first fills stdout before the input is consumed.\n"
            "print(json.dumps({'schema_version':1,'command':'index','data':None,'diagnostics':[], 'padding':'x'*2000000}), flush=True)\n"
            "assert len(sys.stdin.buffer.read()) == 2000000\n", timeout=3)("x" * 2_000_000)
        self.assertEqual(result["padding"], "x" * 2_000_000)
        self.assert_no_workers()

    def test_timeout_terminates_child_with_blocked_stdin(self):
        indexer = self.indexer("import time\ntime.sleep(10)\n", timeout=0.15)
        started = time.monotonic()
        with self.assertRaisesRegex(RequestError, "deadline"):
            indexer("x" * 1_000_000)
        self.assertLess(time.monotonic() - started, 1.5)
        self.assert_no_workers()

    def test_output_budget_kills_child_and_reaps_workers(self):
        indexer = self.indexer("import sys, time\nsys.stdout.buffer.write(b'x'*1000000)\nsys.stdout.buffer.flush()\ntime.sleep(10)\n")
        with patch("limits_lsp.MAX_MESSAGE_BYTES", 65536), self.assertRaisesRegex(RequestError, "response limit"):
            indexer("x")
        self.assert_no_workers()

    @unittest.skipUnless(hasattr(os, "fork"), "Requires inherited POSIX pipes")
    def test_exited_child_with_inherited_stdout_respects_deadline(self):
        pid_file = Path(self.temp.name) / "descendant.pid"
        indexer = self.indexer(
            "import os, time\n"
            "pid = os.fork()\n"
            "if pid == 0:\n"
            "    time.sleep(10)\n"
            "    os._exit(0)\n"
            f"open({str(pid_file)!r}, 'w').write(str(pid))\n"
            "os._exit(0)\n", timeout=0.2)
        started = time.monotonic()
        try:
            with self.assertRaisesRegex(RequestError, "deadline"):
                indexer("x")
            self.assertLess(time.monotonic() - started, 1.5)
            self.assert_no_workers()
        finally:
            if pid_file.exists():
                try:
                    os.kill(int(pid_file.read_text()), signal.SIGKILL)
                except ProcessLookupError:
                    pass

    def test_invalid_backend_json_and_schema_are_internal_errors(self):
        invalid = [b'{}', b'{"schema_version":true,"command":"index"}',
                   b'{"schema_version":1,"schema_version":1,"command":"index"}',
                   b'{"schema_version":1,"command":"index","data":NaN}',
                   b'{"schema_version":1,"command":"index","data":1e999}',
                   b'{"schema_version":1,"command":"index","diagnostics":null}',
                   b'{"schema_version":1,"command":"index","diagnostics":["bad"]}',
                   b'{"schema_version":1,"command":"index","data":{"schema_version":1,"scopes":[],"symbols":null,"references":[]}}',
                   b'{"schema_version":1,"command":"index","diagnostics":[{"message":"\\ud800"}]}',
                   b'{"x":' + b"[" * 3000 + b"0" + b"]" * 3000 + b"}"]
        for payload in invalid:
            with self.subTest(payload=payload[:90]):
                indexer = self.indexer(f"import sys\nsys.stdout.buffer.write({payload!r})\n")
                with self.assertRaises(RequestError) as raised:
                    indexer("x")
                self.assertEqual(raised.exception.code, -32603)
                self.assert_no_workers()

    def test_exit_one_diagnostics_are_accepted_and_other_statuses_fail(self):
        payload = json.dumps(envelope()).encode()
        for status in (0, 1, 2):
            indexer = self.indexer(f"import sys\nsys.stdout.buffer.write({payload!r})\nsys.exit({status})\n")
            if status in (0, 1):
                self.assertEqual(indexer("x"), envelope())
            else:
                with self.assertRaises(RequestError):
                    indexer("x")
            self.assert_no_workers()


class BackendSchema(unittest.TestCase):
    @staticmethod
    def symbol_index(name, declarations=1):
        response = envelope()
        response["data"] = {
            "schema_version": 1,
            "scopes": [{"id": 0, "parent": None, "kind": "module", "range": None}],
            "symbols": [{"id": 0, "scope": 0, "name": name, "kind": "variable", "parameters": [],
                         "declarations": [{"start": {"line": 1, "col": 1}, "end": {"line": 1, "col": 2}}] * declarations}],
            "references": [],
        }
        return response

    def publish(self, result):
        messages = []
        server = Server(lambda _: result, messages.append)
        server.handle({"jsonrpc": "2.0", "id": 1, "method": "initialize"})
        server.handle({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
            "textDocument": {"uri": "untitled:test", "version": 1, "text": "x"}}})
        return server, messages[-1]["params"]["diagnostics"]

    def test_null_host_coordinates_publish_at_document_start(self):
        response = envelope()
        response["diagnostics"] = [{"kind": "host", "message": "backend unavailable", "line": None, "col": None}]
        _, diagnostics = self.publish(response)
        self.assertEqual(diagnostics[0]["message"], "backend unavailable")
        self.assertEqual(diagnostics[0]["range"], {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}})

    def test_large_astral_symbol_uses_utf8_budget_and_server_stays_usable(self):
        # This name occupies 6 MB in UTF-8 and 18 MB with ASCII JSON escapes.
        # Its source and semantic index both fit their documented budgets.
        name = "\U00010400" * 1_500_000
        response = self.symbol_index(name)
        stream = io.BytesIO()
        server = Server(lambda _: response, lambda message: write_message(stream, message))
        server.handle({"jsonrpc": "2.0", "id": 1, "method": "initialize"})
        server.handle({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
            "textDocument": {"uri": "untitled:astral", "version": 1, "text": "let " + name + "=1;"}}})
        self.assertIsNotNone(server.documents["untitled:astral"].envelope["data"])
        stream.seek(0)
        stream.truncate()
        server.handle({"jsonrpc": "2.0", "id": "astral-symbol", "method": "textDocument/documentSymbol",
                       "params": {"textDocument": {"uri": "untitled:astral"}}})
        wire = stream.getvalue()
        self.assertLess(len(wire), 8 * 1024 * 1024)
        self.assertIn("\U00010400".encode("utf-8"), wire)
        self.assertNotIn(b"\\ud801", wire)
        result = read_message(io.BytesIO(wire))
        self.assertEqual(result["id"], "astral-symbol")
        self.assertEqual(result["result"][0]["name"], name)
        stream.seek(0)
        stream.truncate()
        server.handle({"jsonrpc": "2.0", "id": 3, "method": "shutdown"})
        self.assertEqual(read_message(io.BytesIO(stream.getvalue())), {"jsonrpc": "2.0", "id": 3, "result": None})

    def test_oversized_response_is_internal_error_with_same_id_then_recovers(self):
        # A small cached index can produce an oversized response by repeating
        # the symbol name and a valid 4-KiB URI for each redeclaration.
        name, uri = "x" * 16_000, "u:" + "x" * 4094
        stream = io.BytesIO()
        server = Server(lambda _: self.symbol_index(name, 1024), lambda message: write_message(stream, message))
        server.handle({"jsonrpc": "2.0", "id": 1, "method": "initialize"})
        server.handle({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
            "textDocument": {"uri": uri, "version": 1, "text": "x"}}})
        stream.seek(0)
        stream.truncate()
        request_id = "resource-\U00010400"
        server.handle({"jsonrpc": "2.0", "id": request_id, "method": "textDocument/documentSymbol",
                       "params": {"textDocument": {"uri": uri}}})
        failure = read_message(io.BytesIO(stream.getvalue()))
        self.assertEqual(failure["id"], request_id)
        self.assertEqual(failure["error"]["code"], -32603)
        self.assertIn("message limit", failure["error"]["message"])
        stream.seek(0)
        stream.truncate()
        server.handle({"jsonrpc": "2.0", "id": 3, "method": "textDocument/hover", "params": {
            "textDocument": {"uri": uri}, "position": {"line": 0, "character": 0}}})
        following = read_message(io.BytesIO(stream.getvalue()))
        self.assertEqual(following["id"], 3)
        self.assertEqual(following["result"]["contents"]["value"], "variable: " + name)
        self.assertFalse(server.exit_requested)

    def test_malformed_schema_and_scope_cycles_degrade_to_diagnostics(self):
        valid = envelope()
        valid["data"] = {"schema_version": 1, "scopes": [{"id": 0, "parent": None, "kind": "builtins", "range": None}],
                         "symbols": [{"id": 0, "scope": 0, "name": "x", "kind": "variable", "parameters": [], "declarations": []}],
                         "references": []}
        cases = []
        for path, bad in (("scope_id", []), ("scope_parent", 0), ("symbol_id", []), ("symbol_scope", []),
                          ("symbol_parameters", [42]), ("declarations", [{"start": {}, "end": {}}]),
                          ("references", [{"range": {"start": {"line": 1, "col": 1}, "end": {"line": 1, "col": 2}}, "candidates": [[]]}])):
            response = copy.deepcopy(valid)
            if path == "scope_id":
                response["data"]["scopes"][0]["id"] = bad
            elif path == "scope_parent":
                response["data"]["scopes"][0]["parent"] = bad
            elif path.startswith("symbol_"):
                response["data"]["symbols"][0][path.removeprefix("symbol_")] = bad
            elif path == "declarations":
                response["data"]["symbols"][0][path] = bad
            else:
                response["data"][path] = bad
            cases.append(response)
        for response in cases:
            with self.subTest(response=response):
                server, diagnostics = self.publish(response)
                self.assertIn("invalid index response", diagnostics[0]["message"])
                self.assertIsNone(server.documents["untitled:test"].envelope["data"])

    def test_completion_with_a_large_identifier_finishes_within_a_deadline(self):
        # An error index is legitimate for a large incomplete identifier. The
        # prefix scan must remain linear even when there are no local symbols.
        script = "\n".join([
            "from limits_lsp import Server",
            "s = Server(lambda _: {'schema_version':1,'command':'index','data':None,'diagnostics':[]}, lambda _: None)",
            "s.dispatch('initialize', {})",
            "text = 'a' * 1000000",
            "s.dispatch('textDocument/didOpen', {'textDocument':{'uri':'u:','version':1,'text':text}})",
            "assert s.dispatch('textDocument/completion', {'textDocument':{'uri':'u:'},'position':{'line':0,'character':len(text)}}) == {'isIncomplete':False,'items':[]}",
        ])
        result = subprocess.run([sys.executable, "-c", script], cwd=Path(__file__).parent,
                                capture_output=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    unittest.main()
