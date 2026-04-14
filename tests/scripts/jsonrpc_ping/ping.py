#!/usr/bin/env python3
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
    init_msg = recv()
    assert init_msg is not None, "No initialize message received"
    assert init_msg.get("method") == "initialize"
    params = init_msg.get("params", {})
    name = params.get("name", "world")

    # Make a callback to engine (will fail since no provider in tests)
    send({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "llm/chat",
        "params": {"model": "test", "messages": [{"role": "user", "content": "hi"}]}
    })
    llm_response = recv()

    # Return final result
    send({
        "jsonrpc": "2.0",
        "id": 0,
        "result": {
            "greeting": f"Hello, {name}!",
            "llm_called": llm_response is not None and "error" not in (llm_response or {})
        }
    })

if __name__ == "__main__":
    main()
