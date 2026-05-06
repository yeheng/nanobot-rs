"""Minimal Gasket plugin SDK for JSON-RPC daemon plugins.

Wraps stdio JSON-RPC 2.0 boilerplate so plugins can focus on logic.
Single-threaded, request-response. No daemon loop — plugin is one-shot;
daemon reuse is handled by the Rust runner.
"""
import json
import sys
from typing import Any, Optional


class GasketPlugin:
    def __init__(self) -> None:
        self._next_id = 1
        self._args: Optional[dict] = None
        self._init_id: Any = None
        self._default_model: Optional[str] = None
        self._channel: Optional[str] = None
        self._chat_id: Optional[str] = None

    # -- lifecycle ----------------------------------------------------------
    def get_args(self) -> dict:
        """Block until the engine sends the initialize request."""
        if self._args is not None:
            return self._args
        req = self._recv()
        if req is None or req.get("method") != "initialize":
            raise RuntimeError(f"Expected initialize, got: {req}")
        self._init_id = req.get("id")
        params = req.get("params", {}) or {}
        # Extract engine metadata injected at initialization
        self._default_model = params.pop("_gasket_default_model", None)
        self._channel = params.pop("_gasket_channel", None)
        self._chat_id = params.pop("_gasket_chat_id", None)
        self._args = params
        return self._args

    def return_result(self, result: dict) -> None:
        """Reply to the initialize request with a successful result."""
        self._send({"jsonrpc": "2.0", "id": self._init_id, "result": result})

    def return_error(self, code: int, message: str) -> None:
        """Reply to the initialize request with an error."""
        self._send({
            "jsonrpc": "2.0",
            "id": self._init_id,
            "error": {"code": code, "message": message},
        })

    # -- engine callbacks ---------------------------------------------------
    def spawn_subagent(self, task: str, model: Optional[str] = None) -> str:
        """Spawn a subagent and block until it returns. Returns content string."""
        params: dict = {"task": task}
        if model is not None:
            params["model_id"] = model
        result = self._call("subagent/spawn", params)
        return result.get("content", "")

    def llm_chat(
        self, model: Optional[str] = None, messages: Optional[list] = None, **kwargs: Any
    ) -> dict:
        """Direct LLM chat completion via the engine."""
        if messages is None:
            raise RuntimeError("messages is required")
        resolved = model or self._default_model
        if not resolved:
            raise RuntimeError("model is required and no default model is configured")
        return self._call("llm/chat", {"model": resolved, "messages": messages, **kwargs})

    def send_message(self, channel: str, chat_id: str, content: str) -> dict:
        """Send a message to a specific channel/chat via the engine."""
        return self._call(
            "message/send",
            {"channel": channel, "chat_id": chat_id, "content": content},
        )

    # -- internals ----------------------------------------------------------
    def _call(self, method: str, params: dict) -> dict:
        rid = self._next_id
        self._next_id += 1
        self._send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
        resp = self._recv()
        if resp is None:
            raise RuntimeError(f"stdin closed while waiting for {method}")
        if "error" in resp:
            err = resp["error"]
            raise RuntimeError(
                f"{method} failed: {err.get('message')} (code {err.get('code')})"
            )
        return resp.get("result", {})

    @staticmethod
    def _send(msg: dict) -> None:
        sys.stdout.write(json.dumps(msg) + "\n")
        sys.stdout.flush()

    @staticmethod
    def _recv() -> Optional[dict]:
        line = sys.stdin.readline()
        if not line:
            return None
        return json.loads(line.strip())
