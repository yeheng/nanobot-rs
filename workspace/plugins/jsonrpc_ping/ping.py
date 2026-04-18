#!/usr/bin/env python3
"""JSON-RPC plugin example: demonstrates bidirectional communication.

This script uses the JSON-RPC 2.0 protocol over stdio. It receives an
"initialize" request, optionally makes engine callbacks (e.g. llm/chat),
and returns the final result.

The while-loop keeps the process alive so Gasket can reuse it across
multiple tool invocations (daemon mode).
"""
import json
import sys


def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()


def recv():
    line = sys.stdin.readline()
    if not line:
        return None
    return json.loads(line.strip())


def main():
    while True:
        req = recv()
        if req is None:
            # stdin closed — Gasket is shutting down the daemon
            break

        method = req.get("method")
        req_id = req.get("id")

        if method == "initialize":
            params = req.get("params", {})
            name = params.get("name", "world")

            # Make a callback to the engine (llm/chat)
            send({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "llm/chat",
                "params": {
                    "model": "glm-5",
                    "messages": [{"role": "user", "content": "hi"}]
                }
            })
            llm_response = recv()

            llm_called = (
                llm_response is not None
                and "error" not in (llm_response or {})
            )

            # Reply to the initialize request with the final result
            send({
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {
                    "greeting": f"Hello, {name}!",
                    "llm_called": llm_called
                }
            })
        else:
            send({
                "jsonrpc": "2.0",
                "id": req_id,
                "error": {
                    "code": -32601,
                    "message": f"Unknown method: {method}"
                }
            })


if __name__ == "__main__":
    main()
