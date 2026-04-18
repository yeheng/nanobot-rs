#!/usr/bin/env python3
"""Simple plugin example: echoes back the input message.

This demonstrates the Simple protocol — JSON in via stdin, JSON out via stdout.
"""
import json
import sys

args = json.load(sys.stdin)
result = {"echo": args.get("message", ""), "status": "ok"}
print(json.dumps(result))
