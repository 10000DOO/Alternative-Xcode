#!/usr/bin/env python3
"""stdio LSP proxy that wraps sourcekit-lsp for Zed.

Why this exists
---------------
sourcekit-lsp sends server->client "refresh" requests (e.g.
`workspace/semanticTokens/refresh`) with an empty-object body
`"params": {}`. Per the LSP spec those requests carry no params, and Zed
deserializes their params as the unit type `()`. An empty *map* is not unit,
so Zed rejects the request:

    ERROR [lsp] error deserializing workspace/semanticTokens/refresh request:
    Error("invalid type: map, expected unit")

When that refresh is dropped, Zed never re-requests semantic tokens after
the index becomes ready, so Objective-C/C/C++ files never receive the
compiler-accurate semantic highlighting (macros, classes, ivars, ...) and
fall back to the tree-sitter base layer forever.

This proxy sits between Zed and the real sourcekit-lsp and strips the empty
`params: {}` from those params-less refresh requests so Zed accepts them,
responds, and re-requests tokens. Everything else is forwarded byte-for-byte.

Zed launches this instead of sourcekit-lsp (see settings.json:
lsp.sourcekit-lsp.binary.path). Any CLI args Zed passes are forwarded to the
real server. The real binary is resolved from $SOURCEKIT_LSP_REAL, then the
Xcode default toolchain path, then `xcrun -f sourcekit-lsp`.
"""
import sys
import os
import json
import shutil
import subprocess
import threading

# Optional debug log: set SK_SHIM_DEBUG=<path> to capture the semantic-token
# legend and responses flowing between Zed and sourcekit-lsp. Off by default.
_DBG = os.environ.get("SK_SHIM_DEBUG")


def _dbg(obj):
    if not _DBG:
        return
    try:
        with open(_DBG, "a") as f:
            f.write(json.dumps(obj) + "\n")
    except Exception:
        pass

# Server->client requests that per spec take no params but which sourcekit-lsp
# emits with an empty `params: {}` that Zed's unit deserializer rejects.
_REWRITE_METHODS = {
    "workspace/semanticTokens/refresh",
    "workspace/inlayHint/refresh",
    "workspace/codeLens/refresh",
    "workspace/diagnostic/refresh",
    "workspace/codeActions/refresh",
}


def _resolve_real():
    env = os.environ.get("SOURCEKIT_LSP_REAL")
    if env and os.path.exists(env):
        return env
    default = ("/Applications/Xcode.app/Contents/Developer/Toolchains/"
               "XcodeDefault.xctoolchain/usr/bin/sourcekit-lsp")
    if os.path.exists(default):
        return default
    found = shutil.which("sourcekit-lsp")
    if found:
        return found
    try:
        out = subprocess.check_output(["xcrun", "-f", "sourcekit-lsp"])
        return out.decode().strip()
    except Exception:
        return default  # last resort; will fail loudly if missing


def _read_message(rs):
    """Read one framed LSP message. Returns (headers_bytes, body_bytes) or (None, None) at EOF."""
    headers = b""
    while b"\r\n\r\n" not in headers:
        c = rs.read(1)
        if not c:
            return None, None
        headers += c
    length = 0
    for line in headers.split(b"\r\n"):
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":", 1)[1].strip())
    body = b""
    while len(body) < length:
        chunk = rs.read(length - len(body))
        if not chunk:
            break
        body += chunk
    return headers, body


def _raw_pump(rs, ws):
    """Forward client->server bytes unchanged (read1 so small messages aren't buffered)."""
    while True:
        data = rs.read1(65536)
        if not data:
            break
        ws.write(data)
        ws.flush()
    try:
        ws.close()
    except Exception:
        pass


def _server_to_client(rs, ws):
    while True:
        headers, body = _read_message(rs)
        if headers is None:
            break
        try:
            msg = json.loads(body)
            if (msg.get("method") in _REWRITE_METHODS
                    and isinstance(msg.get("params"), dict)
                    and len(msg["params"]) == 0):
                del msg["params"]
                body = json.dumps(msg).encode("utf-8")
                headers = ("Content-Length: %d\r\n\r\n" % len(body)).encode("utf-8")
            if _DBG:
                if msg.get("method") == "client/registerCapability":
                    for reg in (msg.get("params") or {}).get("registrations", []):
                        if "semanticTokens" in (reg.get("method") or ""):
                            legend = (reg.get("registerOptions") or {}).get("legend", {})
                            _dbg({"kind": "legend",
                                  "tokenTypes": legend.get("tokenTypes"),
                                  "tokenModifiers": legend.get("tokenModifiers")})
                res = msg.get("result")
                if isinstance(res, dict) and isinstance(res.get("data"), list):
                    _dbg({"kind": "tokens", "id": msg.get("id"),
                          "n": len(res["data"]) // 5, "data": res["data"]})
        except Exception:
            pass  # never let a parse hiccup corrupt the stream
        ws.write(headers)
        ws.write(body)
        ws.flush()
    try:
        ws.close()
    except Exception:
        pass


def main():
    real = _resolve_real()
    child = subprocess.Popen([real] + sys.argv[1:],
                             stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=None)
    threading.Thread(target=_raw_pump, args=(sys.stdin.buffer, child.stdin), daemon=True).start()
    threading.Thread(target=_server_to_client, args=(child.stdout, sys.stdout.buffer), daemon=True).start()
    sys.exit(child.wait())


if __name__ == "__main__":
    main()
