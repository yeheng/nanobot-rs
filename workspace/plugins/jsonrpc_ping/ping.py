#!/usr/bin/env python3
"""JSON-RPC plugin example using gasket_sdk."""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from gasket_sdk import GasketPlugin


def main() -> None:
    plugin = GasketPlugin()
    args = plugin.get_args()
    name = args.get("name", "world")

    # Exercise the engine callback path
    chat = plugin.llm_chat(
        model="glm-5",
        messages=[{"role": "user", "content": "hi"}],
    )
    llm_called = bool(chat)

    plugin.return_result({"greeting": f"Hello, {name}!", "llm_called": llm_called})


if __name__ == "__main__":
    main()
