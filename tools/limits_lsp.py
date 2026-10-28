#!/usr/bin/env python3
"""Small, dependency-free LSP bridge for the Limits semantic index.

Run with: python tools/limits_lsp.py --lim-bin compiler/target/release/lim
The configured CLI is trusted; documents are passed on stdin, never as paths.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import threading
from dataclasses import dataclass
from typing import BinaryIO, Callable

MAX_MESSAGE_BYTES = 16 * 1024 * 1024
MAX_DOCUMENTS = 32
MAX_DOCUMENT_BYTES = 16 * 1024 * 1024
MAX_INDEX_BYTES = 8 * 1024 * 1024
MAX_CACHE_BYTES = 32 * 1024 * 1024
MAX_URI_BYTES = 4096
MAX_ID_BYTES = 4096
MAX_QUERY_RESULTS = 1024
KEYWORDS = "let fn return if else while for in true false nil and or not break continue".split()
BUILTINS = "print len push pop str int float type keys assert get has join sum".split()


class FrameError(ValueError):
    """An invalid or truncated JSON-RPC transport message."""


class RequestError(ValueError):
    def __init__(self, code: int, message: str):
        super().__init__(message)
        self.code = code
        self.message = message


def read_message(stream: BinaryIO) -> dict | None:
    length = None
    header_bytes = 0
    while True:
        line = stream.readline(8193)
        if not line and header_bytes == 0:
            return None
        header_bytes += len(line)
        if not line or header_bytes > 8192 or not line.endswith(b"\r\n"):
            raise FrameError("Invalid LSP headers")
        if line == b"\r\n":
            break
        try:
            name, value = line[:-2].decode("ascii").split(":", 1)
        except (UnicodeError, ValueError) as error:
            raise FrameError("Invalid LSP header") from error
        if name.lower() == "content-length":
            value = value.strip()
            if length is not None or len(value) > 8 or not value.isascii() or not value.isdecimal():
                raise FrameError("Invalid Content-Length")
            length = int(value)
    if length is None or not 0 < length <= MAX_MESSAGE_BYTES:
        raise FrameError("Content-Length is missing or exceeds the message limit")
    payload = stream.read(length)
    if len(payload) != length:
        raise FrameError("Truncated LSP payload")
    try:
        message = json.loads(payload.decode("utf-8"), parse_constant=lambda _: (_ for _ in ()).throw(ValueError("Invalid JSON number")))
    except (UnicodeError, ValueError, RecursionError) as error:
        raise FrameError("Invalid JSON payload") from error
    if not isinstance(message, dict):
        raise FrameError("JSON-RPC message must be an object")
    return message


def write_message(stream: BinaryIO, message: dict) -> None:
    body = json.dumps(message, ensure_ascii=True, allow_nan=False, separators=(",", ":")).encode("utf-8")
    if len(body) > MAX_MESSAGE_BYTES:
        raise FrameError("Response exceeds the message limit")
    stream.write(f"Content-Length: {len(body)}\r\n\r\n".encode("ascii") + body)
    stream.flush()


def _object(value: object) -> dict:
    if not isinstance(value, dict):
        raise RequestError(-32602, "Expected an object parameter")
    return value


def _lines(text: str) -> list[str]:
    # split("\n") preserves the empty EOF line; columns count Unicode scalars.
    return text.split("\n")


def to_lsp(text: str, point: dict) -> dict:
    lines = _lines(text)
    line = max(0, min(int(point.get("line", 1)) - 1, len(lines) - 1))
    col = max(0, min(int(point.get("col", 1)) - 1, len(lines[line].rstrip("\r"))))
    return {"line": line, "character": len(lines[line][:col].encode("utf-16-le")) // 2}


def from_lsp(text: str, point: dict) -> dict:
    point = _object(point)
    lines = _lines(text)
    line, units = point.get("line"), point.get("character")
    if type(line) is not int or type(units) is not int or line < 0 or line >= len(lines) or units < 0:
        raise RequestError(-32602, "Invalid document position")
    used = 0
    for col, char in enumerate(lines[line].rstrip("\r")):
        if used == units:
            return {"line": line + 1, "col": col + 1}
        used += 2 if ord(char) > 0xFFFF else 1
        if used > units:
            raise RequestError(-32602, "Position splits a UTF-16 surrogate pair")
    if used != units:
        raise RequestError(-32602, "Position exceeds the line")
    return {"line": line + 1, "col": len(lines[line].rstrip("\r")) + 1}


def _position(point: dict) -> tuple[int, int]:
    return point["line"], point["col"]


def _contains(span: dict, point: dict, include_end: bool = False) -> bool:
    start, end, at = _position(span["start"]), _position(span["end"]), _position(point)
    return start <= at and (at <= end if include_end else at < end)


def _range(text: str, span: dict) -> dict:
    return {"start": to_lsp(text, span["start"]), "end": to_lsp(text, span["end"])}


class CliIndexer:
    def __init__(self, executable: str, timeout: float = 5.0):
        self.executable = executable
        self.timeout = timeout

    def __call__(self, text: str) -> dict:
        source = text.encode("utf-8")
        command = [self.executable, "index", "-", "--json"]
        try:
            process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                       stderr=subprocess.DEVNULL)
        except OSError as error:
            raise RequestError(-32603, "Cannot start the configured Limits CLI") from error
        output = bytearray()
        oversized = threading.Event()

        def read_output() -> None:
            while chunk := process.stdout.read(65536):
                if len(output) + len(chunk) > MAX_MESSAGE_BYTES:
                    oversized.set()
                    try:
                        process.kill()
                    except OSError:
                        pass
                    break
                output.extend(chunk)

        def write_input() -> None:
            try:
                process.stdin.write(source)
                process.stdin.close()
            except (BrokenPipeError, OSError):
                pass

        reader = threading.Thread(target=read_output, daemon=True)
        writer = threading.Thread(target=write_input, daemon=True)
        reader.start()
        writer.start()
        try:
            process.wait(timeout=self.timeout)
        except subprocess.TimeoutExpired as error:
            process.kill()
            process.wait()
            raise RequestError(-32603, "Limits indexing deadline exceeded") from error
        finally:
            reader.join(timeout=1.0)
            writer.join(timeout=1.0)
            process.stdout.close()
        if oversized.is_set():
            raise RequestError(-32603, "Limits index exceeds the response limit")
        try:
            envelope = json.loads(output.decode("utf-8"))
        except (UnicodeError, ValueError, RecursionError) as error:
            raise RequestError(-32603, "Limits CLI returned an invalid index response") from error
        if not isinstance(envelope, dict) or envelope.get("schema_version") != 1:
            raise RequestError(-32603, "Unsupported Limits CLI response schema")
        # Index may exit 1 with diagnostics; its checked semantic data is still useful.
        if process.returncode not in (0, 1) or envelope.get("command") != "index":
            raise RequestError(-32603, "Limits CLI could not index the document")
        return envelope


@dataclass
class Document:
    text: str
    version: int
    envelope: dict
    index_bytes: int = 0


class Server:
    def __init__(self, indexer: Callable[[str], dict], emit: Callable[[dict], None]):
        self.indexer = indexer
        self.emit = emit
        self.documents: dict[str, Document] = {}
        self.initialized = False
        self.shutdown = False
        self.exit_requested = False

    def _publish(self, uri: str, doc: Document | None) -> None:
        diagnostics = []
        for item in doc.envelope.get("diagnostics", []) if doc else []:
            point = {"line": item.get("line", 1), "col": item.get("col", 1)}
            start = to_lsp(doc.text, point)
            line = _lines(doc.text)[start["line"]].rstrip("\r")
            # Highlight one Unicode scalar if present; EOF is an empty range.
            end_point = dict(point)
            if point["col"] - 1 < len(line):
                end_point["col"] += 1
            diagnostics.append({"range": {"start": start, "end": to_lsp(doc.text, end_point)},
                                "severity": 1, "source": "limits", "code": item.get("code", item.get("kind", "check")),
                                "message": item.get("message", item.get("msg", "Limits diagnostic"))})
        params = {"uri": uri, "diagnostics": diagnostics}
        if doc:
            params["version"] = doc.version
        self.emit({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": params})

    def _put(self, uri: str, text: str, version: int) -> None:
        if not isinstance(uri, str) or not uri or not isinstance(text, str) or type(version) is not int:
            raise RequestError(-32602, "Invalid document")
        try:
            if len(uri.encode("utf-8")) > MAX_URI_BYTES:
                raise RequestError(-32602, "Document URI exceeds its byte limit")
            size = len(text.encode("utf-8"))
        except UnicodeError as error:
            raise RequestError(-32602, "Document contains invalid Unicode") from error
        old = self.documents.get(uri)
        if old and version <= old.version:
            raise RequestError(-32602, "Document version must increase")
        total = sum(len(d.text.encode("utf-8")) for key, d in self.documents.items() if key != uri)
        if size + total > MAX_DOCUMENT_BYTES or (not old and len(self.documents) >= MAX_DOCUMENTS):
            raise RequestError(-32602, "Open document budget exceeded")
        try:
            envelope = self.indexer(text)
        except RequestError as error:
            envelope = {"schema_version": 1, "command": "index", "data": None,
                        "diagnostics": [{"kind": "index", "message": error.message, "line": 1, "col": 1}]}
        index_bytes = len(json.dumps(envelope, ensure_ascii=False, allow_nan=False, separators=(",", ":")).encode("utf-8"))
        other_bytes = sum(d.index_bytes for key, d in self.documents.items() if key != uri)
        if index_bytes > MAX_INDEX_BYTES or other_bytes + index_bytes > MAX_CACHE_BYTES:
            envelope = {"schema_version": 1, "command": "index", "data": None,
                        "diagnostics": [{"kind": "index", "message": "Semantic index cache budget exceeded", "line": 1, "col": 1}]}
            index_bytes = 0
        self.documents[uri] = Document(text, version, envelope, index_bytes)
        self._publish(uri, self.documents[uri])

    def _document(self, params: dict) -> tuple[str, Document]:
        uri = _object(params.get("textDocument", {})).get("uri")
        if uri not in self.documents:
            raise RequestError(-32602, "Document is not open")
        return uri, self.documents[uri]

    def _symbol_ids(self, index: dict, point: dict) -> list[int]:
        for symbol in index.get("symbols", []):
            if any(_contains(span, point) for span in symbol.get("declarations", [])):
                return [symbol["id"]]
        for ref in index.get("references", []):
            if _contains(ref["range"], point):
                return ref.get("candidates", [])
        return []

    def _completion(self, doc: Document, index: dict, point: dict) -> list[dict]:
        line = _lines(doc.text)[point["line"] - 1][:point["col"] - 1]
        prefix = ""
        for char in reversed(line):
            if char.isalnum() or char == "_":
                prefix = char + prefix
            else:
                break
        scopes = {s["id"]: s for s in index.get("scopes", [])}
        candidates = []
        for scope in scopes.values():
            if scope.get("kind") == "builtins" or (scope.get("range") and _contains(scope["range"], point, scope.get("kind") == "module")):
                depth, parent = 0, scope.get("parent")
                while parent in scopes:
                    depth += 1
                    parent = scopes[parent].get("parent")
                candidates.append((depth, scope["id"]))
        visible, seen = [], set()
        scope_id = max(candidates)[1] if candidates else None
        while scope_id in scopes:
            for symbol in index.get("symbols", []):
                name = symbol["name"]
                if symbol["scope"] == scope_id and name not in seen:
                    seen.add(name)
                    if name.startswith(prefix):
                        visible.append({"label": name, "kind": 3 if symbol["kind"] in ("function", "builtin") else 6,
                                        "detail": symbol["kind"]})
            scope_id = scopes[scope_id].get("parent")
        # Parse errors have no scope tree. Offer only language-level names then.
        fallback = KEYWORDS + (BUILTINS if not scopes else [])
        for name in fallback:
            if name.startswith(prefix) and name not in seen:
                visible.append({"label": name, "kind": 14 if name in KEYWORDS else 3})
        return sorted(visible, key=lambda item: item["label"])

    def dispatch(self, method: str, params: dict) -> object:
        if method == "initialize":
            if self.initialized:
                raise RequestError(-32600, "Already initialized")
            self.initialized = True
            return {"capabilities": {"positionEncoding": "utf-16", "textDocumentSync": 1,
                                     "documentSymbolProvider": True, "definitionProvider": True,
                                     "referencesProvider": True, "completionProvider": {"triggerCharacters": []},
                                     "hoverProvider": True},
                    "serverInfo": {"name": "limits-lsp-bridge", "version": "1"}}
        if method == "exit":
            self.exit_requested = True
            return None
        if not self.initialized:
            raise RequestError(-32002, "Server not initialized")
        if method == "shutdown":
            self.shutdown = True
            return None
        if self.shutdown:
            raise RequestError(-32600, "Server has shut down")
        if method in ("initialized", "$/cancelRequest", "$/setTrace"):
            return None
        if method == "textDocument/didOpen":
            item = _object(params.get("textDocument", {}))
            self._put(item.get("uri"), item.get("text"), item.get("version"))
            return None
        if method == "textDocument/didChange":
            uri, _ = self._document(params)
            changes = params.get("contentChanges", [])
            if not isinstance(changes, list) or len(changes) != 1 or not isinstance(changes[0], dict) or changes[0].get("range") is not None:
                raise RequestError(-32602, "Only full document synchronization is supported")
            self._put(uri, changes[0].get("text"), params["textDocument"].get("version"))
            return None
        if method == "textDocument/didClose":
            uri, _ = self._document(params)
            del self.documents[uri]
            self._publish(uri, None)
            return None
        if method not in ("textDocument/documentSymbol", "textDocument/definition", "textDocument/references", "textDocument/completion", "textDocument/hover"):
            raise RequestError(-32601, "Method not found")
        uri, doc = self._document(params)
        index = doc.envelope.get("data") or {}
        if method == "textDocument/documentSymbol":
            return [{"name": s["name"], "kind": 12 if s["kind"] == "function" else 13,
                     "location": {"uri": uri, "range": _range(doc.text, span)}}
                    for s in index.get("symbols", []) for span in s.get("declarations", [])][:MAX_QUERY_RESULTS]
        point = from_lsp(doc.text, params.get("position", {}))
        if method == "textDocument/completion":
            items = self._completion(doc, index, point)
            return {"isIncomplete": len(items) > MAX_QUERY_RESULTS, "items": items[:MAX_QUERY_RESULTS]}
        ids = self._symbol_ids(index, point)
        symbols = [s for s in index.get("symbols", []) if s["id"] in ids]
        if method == "textDocument/definition":
            return [{"uri": uri, "range": _range(doc.text, span)}
                    for s in symbols for span in s.get("declarations", [])][:MAX_QUERY_RESULTS]
        if method == "textDocument/references":
            locations = [{"uri": uri, "range": _range(doc.text, r["range"])}
                         for r in index.get("references", []) if any(i in r.get("candidates", []) for i in ids)]
            if _object(params.get("context", {})).get("includeDeclaration", False):
                locations = [{"uri": uri, "range": _range(doc.text, span)}
                             for s in symbols for span in s.get("declarations", [])] + locations
            return locations[:MAX_QUERY_RESULTS]
        if method == "textDocument/hover":
            if not symbols:
                return None
            lines = []
            for symbol in symbols:
                name = symbol["name"]
                if symbol["kind"] == "function":
                    name = f"fn {name}({', '.join(symbol.get('parameters', []))})"
                lines.append(f"{symbol['kind']}: {name}")
            if len(ids) > 1:
                lines.append("Possible lexical bindings; runtime may fall through a later declaration.")
            return {"contents": {"kind": "plaintext", "value": "\n".join(lines)}}
        raise RequestError(-32601, "Method not found")

    def handle(self, message: dict) -> None:
        incoming_id = message.get("id")
        ident = incoming_id if type(incoming_id) in (str, int) else None
        request = "id" in message
        try:
            if message.get("jsonrpc") != "2.0" or not isinstance(message.get("method"), str):
                raise RequestError(-32600, "Invalid JSON-RPC request")
            if request and type(incoming_id) not in (str, int):
                raise RequestError(-32600, "Invalid JSON-RPC request id")
            if isinstance(ident, str):
                try:
                    if len(ident.encode("utf-8")) > MAX_ID_BYTES:
                        ident = None
                        raise RequestError(-32600, "JSON-RPC request id exceeds its byte limit")
                except UnicodeError as error:
                    ident = None
                    raise RequestError(-32600, "Invalid JSON-RPC request id") from error
            params = message.get("params", {})
            if not isinstance(params, dict):
                raise RequestError(-32602, "Parameters must be an object")
            result = self.dispatch(message["method"], params)
            if request:
                self.emit({"jsonrpc": "2.0", "id": ident, "result": result})
        except (RequestError, KeyError, TypeError, ValueError) as error:
            if request:
                code, detail = (error.code, error.message) if isinstance(error, RequestError) else (-32602, "Invalid parameters")
                self.emit({"jsonrpc": "2.0", "id": ident, "error": {"code": code, "message": detail}})


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lim-bin", default=os.environ.get("LIM_BIN", "lim"))
    args = parser.parse_args()
    server = Server(CliIndexer(args.lim_bin), lambda message: write_message(sys.stdout.buffer, message))
    try:
        while not server.exit_requested:
            message = read_message(sys.stdin.buffer)
            if message is None:
                break
            server.handle(message)
    except FrameError:
        write_message(sys.stdout.buffer, {"jsonrpc": "2.0", "id": None,
                                         "error": {"code": -32700, "message": "Invalid LSP transport message"}})
        return 1
    except (BrokenPipeError, OSError):
        return 1
    return 0 if server.shutdown else 1


if __name__ == "__main__":
    raise SystemExit(main())
