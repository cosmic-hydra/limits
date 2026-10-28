#!/usr/bin/env python3
"""Small, dependency-free LSP bridge for the Limits semantic index.

Run with: python tools/limits_lsp.py --lim-bin compiler/target/release/lim
The configured CLI is trusted; documents are passed on stdin, never as paths.
"""
from __future__ import annotations

import argparse
import json
import math
import os
import re
import select
import subprocess
import sys
import threading
import time
import unicodedata
from array import array
from bisect import bisect_left
from dataclasses import dataclass, field
from itertools import chain, islice
from typing import BinaryIO, Callable

MAX_MESSAGE_BYTES = 16 * 1024 * 1024
MAX_JSON_DEPTH = 128
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


class ResponseError(ValueError):
    """A response that cannot be encoded within the transport budget."""


class RequestError(ValueError):
    def __init__(self, code: int, message: str):
        super().__init__(message)
        self.code = code
        self.message = message


def _json_object(pairs: list[tuple[str, object]]) -> dict:
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("Duplicate JSON object member")
        result[key] = value
    return result


def _json_float(value: str) -> float:
    number = float(value)
    if not math.isfinite(number):
        raise ValueError("Non-finite JSON number")
    return number


def _load_json(payload: bytes) -> object:
    def invalid_constant(_: str) -> None:
        raise ValueError("Invalid JSON number")

    # Reject excessive nesting before entering the native JSON parser; its
    # recursion threshold varies between supported Python versions.
    depth, quoted, escaped = 0, False, False
    for byte in payload:
        if quoted:
            if escaped:
                escaped = False
            elif byte == 92:
                escaped = True
            elif byte == 34:
                quoted = False
        elif byte == 34:
            quoted = True
        elif byte in (91, 123):
            depth += 1
            if depth > MAX_JSON_DEPTH:
                raise ValueError("JSON nesting exceeds its depth limit")
        elif byte in (93, 125):
            depth -= 1
    return json.loads(payload.decode("utf-8"), object_pairs_hook=_json_object,
                      parse_float=_json_float, parse_constant=invalid_constant)


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
        if not re.fullmatch(r"[!#$%&'*+.^_`|~0-9A-Za-z-]+", name):
            raise FrameError("Invalid LSP header name")
        if name.lower() == "content-length":
            value = value.strip()
            if length is not None or len(value) > 8 or not value.isascii() or not value.isdecimal():
                raise FrameError("Invalid Content-Length")
            length = int(value)
    if length is None or not 0 < length <= MAX_MESSAGE_BYTES:
        raise FrameError("Content-Length is missing or exceeds the message limit")
    payload = bytearray()
    while len(payload) < length:
        chunk = stream.read(length - len(payload))
        if not chunk:
            raise FrameError("Truncated LSP payload")
        payload.extend(chunk)
    try:
        message = _load_json(payload)
    except (UnicodeError, ValueError, RecursionError) as error:
        raise FrameError("Invalid JSON payload") from error
    if not isinstance(message, dict):
        raise FrameError("JSON-RPC message must be an object")
    return message


def write_message(stream: BinaryIO, message: dict) -> None:
    try:
        # Preserve UTF-8's byte budget for valid Unicode. Escape isolated
        # surrogates defensively for direct callers; request ids and semantic
        # strings are validated before reaching this function.
        body = json.dumps(message, ensure_ascii=False, allow_nan=False, separators=(",", ":")).encode("utf-8", "backslashreplace")
    except (TypeError, ValueError, RecursionError) as error:
        raise ResponseError("Cannot encode the response") from error
    if len(body) > MAX_MESSAGE_BYTES:
        raise ResponseError("Response exceeds the message limit")
    stream.write(f"Content-Length: {len(body)}\r\n\r\n".encode("ascii") + body)
    stream.flush()


def _object(value: object) -> dict:
    if not isinstance(value, dict):
        raise RequestError(-32602, "Expected an object parameter")
    return value


def _lines(text: str) -> list[str]:
    # LSP and the lexer recognize CR, LF and CRLF; preserve the empty EOF line.
    return re.split(r"\r\n|\r|\n", text)


class _SourceMap:
    def __init__(self, text: str):
        self.lines = _lines(text)
        self.astrals: dict[int, array] = {}

    def _astrals(self, line: int) -> array | tuple:
        text = self.lines[line]
        if text.isascii():
            return ()
        if line not in self.astrals:
            # Compact offsets avoid a Python integer object per astral scalar.
            self.astrals[line] = array("I", (m.start() for m in re.finditer(r"[\U00010000-\U0010ffff]", text)))
        return self.astrals[line]

    def to_lsp(self, point: dict) -> dict:
        line = max(0, min(int(point.get("line", 1)) - 1, len(self.lines) - 1))
        col = max(0, min(int(point.get("col", 1)) - 1, len(self.lines[line])))
        return {"line": line, "character": col + bisect_left(self._astrals(line), col)}

    def from_lsp(self, point: dict) -> dict:
        point = _object(point)
        line, units = point.get("line"), point.get("character")
        if type(line) is not int or type(units) is not int or line < 0 or line >= len(self.lines) or units < 0:
            raise RequestError(-32602, "Invalid document position")
        astrals = self._astrals(line)
        if units > len(self.lines[line]) + len(astrals):
            raise RequestError(-32602, "Position exceeds the line")
        # Astral n's second UTF-16 unit is at scalar_offset + n + 1.
        low, high = 0, len(astrals)
        while low < high:
            middle = (low + high) // 2
            if astrals[middle] + middle + 1 < units:
                low = middle + 1
            else:
                high = middle
        if low < len(astrals) and astrals[low] + low + 1 == units:
            raise RequestError(-32602, "Position splits a UTF-16 surrogate pair")
        return {"line": line + 1, "col": units - low + 1}


def to_lsp(text: str, point: dict) -> dict:
    return _SourceMap(text).to_lsp(point)


def from_lsp(text: str, point: dict) -> dict:
    return _SourceMap(text).from_lsp(point)


def _position(point: dict) -> tuple[int, int]:
    return point["line"], point["col"]


def _contains(span: dict, point: dict, include_end: bool = False) -> bool:
    start, end, at = _position(span["start"]), _position(span["end"]), _position(point)
    return start <= at and (at <= end if include_end else at < end)


def _range(text: str, span: dict, positions: _SourceMap | None = None) -> dict:
    positions = positions or _SourceMap(text)
    return {"start": positions.to_lsp(span["start"]), "end": positions.to_lsp(span["end"])}


def _validate_envelope(envelope: object) -> None:
    """Validate the fields traversed by editor queries, including scope cycles."""
    def require(condition: bool) -> None:
        if not condition:
            raise ValueError("Invalid Limits index schema")

    def integer(value: object, minimum: int = 0) -> bool:
        return type(value) is int and value >= minimum

    def point(value: object) -> bool:
        return isinstance(value, dict) and integer(value.get("line"), 1) and integer(value.get("col"), 1)

    def span(value: object) -> bool:
        return (isinstance(value, dict) and point(value.get("start")) and point(value.get("end"))
                and _position(value["start"]) <= _position(value["end"]))

    require(isinstance(envelope, dict))
    require(type(envelope.get("schema_version")) is int and envelope.get("schema_version") == 1)
    require(envelope.get("command") == "index")
    diagnostics = envelope.get("diagnostics", [])
    require(isinstance(diagnostics, list))
    for item in diagnostics:
        require(isinstance(item, dict))
        for field in ("line", "col"):
            require(item.get(field) is None or integer(item[field], 1))
        require(isinstance(item.get("message", item.get("msg", "Limits diagnostic")), str))
        require(type(item.get("code", item.get("kind", "check"))) in (str, int))
    index = envelope.get("data")
    if index is None:
        return
    require(isinstance(index, dict))
    require(type(index.get("schema_version")) is int and index.get("schema_version") == 1)
    scopes, symbols, references = (index.get(key) for key in ("scopes", "symbols", "references"))
    require(isinstance(scopes, list) and isinstance(symbols, list) and isinstance(references, list))
    scope_map = {}
    for scope in scopes:
        require(isinstance(scope, dict) and integer(scope.get("id")))
        require(scope["id"] not in scope_map and isinstance(scope.get("kind"), str))
        require(scope.get("parent") is None or integer(scope["parent"]))
        require(scope.get("range") is None or span(scope["range"]))
        scope_map[scope["id"]] = scope
    # Each parent chain must terminate. Reuse checked paths so validation stays
    # linear even for a large, deeply nested index.
    checked = set()
    for scope_id in scope_map:
        path = set()
        while scope_id is not None and scope_id not in checked:
            require(scope_id in scope_map and scope_id not in path)
            path.add(scope_id)
            scope_id = scope_map[scope_id].get("parent")
        checked.update(path)
    symbol_ids = set()
    for symbol in symbols:
        require(isinstance(symbol, dict) and integer(symbol.get("id")))
        require(symbol["id"] not in symbol_ids and integer(symbol.get("scope")) and symbol["scope"] in scope_map)
        require(isinstance(symbol.get("name"), str) and isinstance(symbol.get("kind"), str))
        require(isinstance(symbol.get("declarations", []), list) and all(span(item) for item in symbol.get("declarations", [])))
        require(isinstance(symbol.get("parameters", []), list) and all(isinstance(item, str) for item in symbol.get("parameters", [])))
        symbol_ids.add(symbol["id"])
    for ref in references:
        require(isinstance(ref, dict) and span(ref.get("range")))
        require(isinstance(ref.get("candidates", []), list))
        require(all(integer(item) and item in symbol_ids for item in ref.get("candidates", [])))


class CliIndexer:
    def __init__(self, executable: str, timeout: float = 5.0):
        self.executable = executable
        self.timeout = timeout
        self._unusable = False

    def __call__(self, text: str) -> dict:
        if self._unusable:
            raise RequestError(-32603, "Limits CLI pipe cleanup failed; restart the bridge")
        source = text.encode("utf-8")
        command = [self.executable, "index", "-", "--json"]
        try:
            process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                       stderr=subprocess.DEVNULL, bufsize=0)
        except OSError as error:
            raise RequestError(-32603, "Cannot start the configured Limits CLI") from error
        # Nonblocking pipes let the workers stop even if a descendant retains an
        # inherited descriptor. Raw pipes also avoid BufferedReader.close()
        # waiting indefinitely for the lock held by a blocked read.
        for pipe in (process.stdin, process.stdout):
            try:
                os.set_blocking(pipe.fileno(), False)
            except (AttributeError, OSError):
                pass  # Older Windows versions may not support nonblocking pipes.
        output = bytearray()
        oversized = threading.Event()
        stopping = threading.Event()
        output_done = threading.Event()
        input_done = threading.Event()
        io_error = threading.Event()

        def kill() -> None:
            try:
                process.kill()
            except OSError:
                pass

        def wait_pipe(pipe: BinaryIO, writing: bool = False) -> None:
            try:
                select.select([] if writing else [pipe], [pipe] if writing else [], [], 0.05)
            except (OSError, ValueError):
                # Windows select() supports sockets rather than pipe handles.
                stopping.wait(0.001)

        def read_output() -> None:
            try:
                while not stopping.is_set():
                    chunk = process.stdout.read(65536)
                    if chunk is None:
                        wait_pipe(process.stdout)
                        continue
                    if not chunk:
                        break
                    if len(output) + len(chunk) > MAX_MESSAGE_BYTES:
                        oversized.set()
                        kill()
                        break
                    output.extend(chunk)
            except (OSError, ValueError):
                if not stopping.is_set():
                    io_error.set()
                    kill()
            finally:
                output_done.set()

        def write_input() -> None:
            try:
                remaining = memoryview(source)
                while remaining and not stopping.is_set():
                    written = process.stdin.write(remaining[:65536])
                    if written:
                        remaining = remaining[written:]
                    else:
                        wait_pipe(process.stdin, writing=True)
            except (BrokenPipeError, OSError, ValueError):
                pass
            finally:
                process.stdin.close()
                input_done.set()

        reader = threading.Thread(target=read_output, name="limits-index-output", daemon=True)
        writer = threading.Thread(target=write_input, name="limits-index-input", daemon=True)
        deadline = time.monotonic() + self.timeout
        reader.start()
        writer.start()
        try:
            process.wait(timeout=max(0, deadline - time.monotonic()))
            # Child exit is not EOF when another process still holds a pipe.
            if not output_done.wait(max(0, deadline - time.monotonic())) or not input_done.wait(max(0, deadline - time.monotonic())):
                raise subprocess.TimeoutExpired(command, self.timeout)
        except subprocess.TimeoutExpired as error:
            kill()
            try:
                process.wait(timeout=1.0)
            except subprocess.TimeoutExpired:
                self._unusable = True
            raise RequestError(-32603, "Limits indexing deadline exceeded") from error
        finally:
            stopping.set()
            process.stdin.close()
            process.stdout.close()
            reader.join(timeout=0.1)
            writer.join(timeout=0.1)
            if reader.is_alive() or writer.is_alive():
                # Do not accumulate workers on platforms lacking cancellable
                # pipes. A supervisor owns cleanup of CLI descendants.
                self._unusable = True
        if oversized.is_set():
            raise RequestError(-32603, "Limits index exceeds the response limit")
        if io_error.is_set():
            raise RequestError(-32603, "Cannot read the Limits index response")
        try:
            envelope = _load_json(output)
        except (UnicodeError, ValueError, RecursionError) as error:
            raise RequestError(-32603, "Limits CLI returned an invalid index response") from error
        if not isinstance(envelope, dict) or type(envelope.get("schema_version")) is not int or envelope.get("schema_version") != 1:
            raise RequestError(-32603, "Unsupported Limits CLI response schema")
        # Index may exit 1 with diagnostics; its checked semantic data is still useful.
        if process.returncode not in (0, 1) or envelope.get("command") != "index":
            raise RequestError(-32603, "Limits CLI could not index the document")
        try:
            _validate_envelope(envelope)
            # JSON escapes can represent isolated surrogates; semantic strings
            # must be encodable before they enter the document cache.
            json.dumps(envelope, ensure_ascii=False, allow_nan=False).encode("utf-8")
        except (TypeError, ValueError, RecursionError, UnicodeError) as error:
            raise RequestError(-32603, "Limits CLI returned an invalid index response") from error
        return envelope


@dataclass
class Document:
    text: str
    version: int
    envelope: dict
    index_bytes: int = 0
    positions: _SourceMap = field(init=False, repr=False)

    def __post_init__(self) -> None:
        self.positions = _SourceMap(self.text)


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
            # Host/usage diagnostics legitimately have null source coordinates.
            point = {"line": item.get("line") or 1, "col": item.get("col") or 1}
            start = doc.positions.to_lsp(point)
            line = doc.positions.lines[start["line"]]
            # Highlight one Unicode scalar if present; EOF is an empty range.
            end_point = dict(point)
            if point["col"] - 1 < len(line):
                end_point["col"] += 1
            diagnostics.append({"range": {"start": start, "end": doc.positions.to_lsp(end_point)},
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
            _validate_envelope(envelope)
            index_bytes = len(json.dumps(envelope, ensure_ascii=False, allow_nan=False, separators=(",", ":")).encode("utf-8"))
        except RequestError as error:
            envelope = {"schema_version": 1, "command": "index", "data": None,
                        "diagnostics": [{"kind": "index", "message": error.message, "line": 1, "col": 1}]}
            index_bytes = 0
        except (TypeError, ValueError, RecursionError, UnicodeError):
            envelope = {"schema_version": 1, "command": "index", "data": None,
                        "diagnostics": [{"kind": "index", "message": "Limits CLI returned an invalid index response", "line": 1, "col": 1}]}
            index_bytes = 0
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
        line = doc.positions.lines[point["line"] - 1][:point["col"] - 1]
        start = len(line)
        while start:
            char = line[start - 1]
            # Rust's Unicode Alphabetic includes Other_Alphabetic marks and
            # enclosed/cased letters beyond Python's str.isalnum(). Including
            # combining marks conservatively keeps a typed identifier intact.
            if not (char.isalnum() or char.islower() or char.isupper() or char == "_"
                    or unicodedata.category(char) in ("Mn", "Mc")):
                break
            start -= 1
        prefix = line[start:]
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
        if self.shutdown:
            raise RequestError(-32600, "Server has shut down")
        if method == "shutdown":
            self.shutdown = True
            return None
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
            return list(islice(({"name": s["name"], "kind": 12 if s["kind"] == "function" else 13,
                                "location": {"uri": uri, "range": _range(doc.text, span, doc.positions)}}
                               for s in index.get("symbols", []) for span in s.get("declarations", [])), MAX_QUERY_RESULTS))
        point = doc.positions.from_lsp(params.get("position", {}))
        if method == "textDocument/completion":
            items = self._completion(doc, index, point)
            return {"isIncomplete": len(items) > MAX_QUERY_RESULTS, "items": items[:MAX_QUERY_RESULTS]}
        ids = self._symbol_ids(index, point)
        symbol_map = {s["id"]: s for s in index.get("symbols", [])}
        symbols = [symbol_map[ident] for ident in ids]
        if method == "textDocument/definition":
            return list(islice(({"uri": uri, "range": _range(doc.text, span, doc.positions)}
                                for s in symbols for span in s.get("declarations", [])), MAX_QUERY_RESULTS))
        if method == "textDocument/references":
            locations = ({"uri": uri, "range": _range(doc.text, r["range"], doc.positions)}
                         for r in index.get("references", []) if any(i in r.get("candidates", []) for i in ids))
            if _object(params.get("context", {})).get("includeDeclaration", False):
                declarations = ({"uri": uri, "range": _range(doc.text, span, doc.positions)}
                                for s in symbols for span in s.get("declarations", []))
                locations = chain(declarations, locations)
            return list(islice(locations, MAX_QUERY_RESULTS))
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
        except ResponseError as error:
            if request:
                self.emit({"jsonrpc": "2.0", "id": ident,
                           "error": {"code": -32603, "message": str(error)}})
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
