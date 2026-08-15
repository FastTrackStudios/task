#!/usr/bin/env python3
"""Minimal demo language server for exercising the editor-lsp
pipeline without installing a real one.

Speaks just enough LSP: initialize/initialized, didOpen,
incremental didChange, shutdown/exit. Publishes version-stamped
diagnostics flagging every `TODO` (warning) and `FIXME` (error).

Run the playground against it:

    EDITOR_LSP_CMD="python3 tools/demo_ls.py" cargo run -p playground

Positions assume the demo doc is edited in ASCII regions (UTF-16
column == byte column); good enough for a smoke-test server.
"""

import json
import re
import sys


def read_message(stdin):
    length = None
    while True:
        line = stdin.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":")[1])
    if length is None:
        return None
    return json.loads(stdin.read(length))


def send(msg):
    data = json.dumps(msg).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(data))
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()


def offsets(text):
    """Line start offsets for position math."""
    starts = [0]
    for i, ch in enumerate(text):
        if ch == "\n":
            starts.append(i + 1)
    return starts


def to_offset(text, pos):
    starts = offsets(text)
    line = min(pos["line"], len(starts) - 1)
    return min(starts[line] + pos["character"], len(text))


def apply_change(text, change):
    if "range" not in change:
        return change["text"]
    start = to_offset(text, change["range"]["start"])
    end = to_offset(text, change["range"]["end"])
    return text[:start] + change["text"] + text[end:]


def diagnostics_for(text):
    diags = []
    starts = offsets(text)

    def line_col(off):
        line = 0
        for i, s in enumerate(starts):
            if s <= off:
                line = i
            else:
                break
        return line, off - starts[line]

    for pattern, severity, msg in (
        (r"\bFIXME\b", 1, "demo-ls: FIXME found"),
        (r"\bTODO\b", 2, "demo-ls: TODO found"),
    ):
        for m in re.finditer(pattern, text):
            sl, sc = line_col(m.start())
            el, ec = line_col(m.end())
            diags.append({
                "range": {
                    "start": {"line": sl, "character": sc},
                    "end": {"line": el, "character": ec},
                },
                "severity": severity,
                "source": "demo-ls",
                "message": msg,
            })
    return diags


def publish(uri, text, version):
    send({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": {
            "uri": uri,
            "version": version,
            "diagnostics": diagnostics_for(text),
        },
    })


def main():
    stdin = sys.stdin.buffer
    docs = {}  # uri -> (text, version)
    while True:
        msg = read_message(stdin)
        if msg is None:
            return
        method = msg.get("method")
        if method == "initialize":
            send({
                "jsonrpc": "2.0",
                "id": msg["id"],
                "result": {
                    "capabilities": {
                        "textDocumentSync": {"openClose": True, "change": 2},
                    },
                    "serverInfo": {"name": "demo-ls"},
                },
            })
        elif method == "shutdown":
            send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
        elif method == "exit":
            return
        elif method == "textDocument/didOpen":
            td = msg["params"]["textDocument"]
            docs[td["uri"]] = (td["text"], td["version"])
            publish(td["uri"], td["text"], td["version"])
        elif method == "textDocument/didChange":
            td = msg["params"]["textDocument"]
            text, _ = docs.get(td["uri"], ("", 0))
            for change in msg["params"]["contentChanges"]:
                text = apply_change(text, change)
            docs[td["uri"]] = (text, td["version"])
            publish(td["uri"], text, td["version"])
        elif method is not None and "id" in msg:
            # Unknown request — answer so the client never hangs.
            send({
                "jsonrpc": "2.0",
                "id": msg["id"],
                "error": {"code": -32601, "message": "demo-ls: not implemented"},
            })


if __name__ == "__main__":
    main()
