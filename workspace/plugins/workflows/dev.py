#!/usr/bin/env python3
"""Dev workflow: Research → Plan → Implement → Review loop.

Orchestrates 4–8 subagents via subagent/spawn. Implements bounded retry
with strict-JSON verdict parsing; appends reviewer feedback to the plan
on failure rather than replacing it (preserves original goals).
"""
import json
import sys
from pathlib import Path
from typing import Tuple

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from gasket_sdk import GasketPlugin


REVIEW_PROMPT_SUFFIX = """

Output STRICT JSON only, no prose, no markdown fences:
{"verdict": "PASS" | "FAIL", "reason": "<one sentence>"}
"""


def parse_verdict(review_text: str) -> Tuple[str, str]:
    """Parse strict JSON verdict; tolerate ``` fences as fallback.

    Returns (verdict, reason) where verdict is exactly "PASS" or "FAIL".
    Unparseable input is treated as FAIL.
    """
    txt = review_text.strip()
    if txt.startswith("```"):
        # Strip ```json ... ``` fences if model misbehaves
        txt = txt.strip("`")
        if txt.lower().startswith("json"):
            txt = txt[4:]
        txt = txt.strip()
    try:
        obj = json.loads(txt)
    except json.JSONDecodeError:
        return "FAIL", f"reviewer output not parseable: {review_text[:200]}"
    verdict = str(obj.get("verdict", "FAIL")).upper()
    reason = str(obj.get("reason", ""))
    if verdict not in ("PASS", "FAIL"):
        verdict = "FAIL"
    return verdict, reason


def main() -> None:
    plugin = GasketPlugin()
    args = plugin.get_args()

    task = args["task"]
    max_iter = int(args.get("max_iterations", 3))
    reasoner = args.get("reasoner_model")  # None -> engine default
    coder = args.get("coder_model")

    # Phase 1: Research
    research = plugin.spawn_subagent(
        f"Research strictly relevant context for this task. Be concise.\n\n"
        f"Task: {task}",
        model=reasoner,
    )

    # Phase 2: Plan
    plan = plugin.spawn_subagent(
        f"Create concrete implementation steps based on research.\n\n"
        f"Task: {task}\n\nResearch:\n{research}",
        model=reasoner,
    )

    # Phase 3: Implement -> Review loop (best-effort)
    code = ""
    last_reason = ""
    passed = False
    iterations_used = 0
    for i in range(max_iter):
        iterations_used = i + 1
        code = plugin.spawn_subagent(
            f"Implement this plan. Output runnable code only.\n\n"
            f"Plan:\n{plan}\n\nPrevious attempt (may be empty):\n{code}",
            model=coder,
        )
        review = plugin.spawn_subagent(
            f"Review this code against the plan. Be strict.{REVIEW_PROMPT_SUFFIX}\n\n"
            f"Plan:\n{plan}\n\nCode:\n{code}",
            model=reasoner,
        )
        verdict, reason = parse_verdict(review)
        last_reason = reason
        if verdict == "PASS":
            passed = True
            break
        # Append reviewer feedback (do not replace plan)
        plan = f"{plan}\n\n[Reviewer feedback to address]:\n{reason}"

    plugin.return_result({
        "final_code": code,
        "passed": passed,
        "iterations_used": iterations_used,
        "last_review_reason": last_reason,
    })


if __name__ == "__main__":
    main()
