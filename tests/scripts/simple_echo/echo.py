#!/usr/bin/env python3
import json
import sys

data = json.load(sys.stdin)
result = {"echo": data.get("message", ""), "status": "ok"}
json.dump(result, sys.stdout)
