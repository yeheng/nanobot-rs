# Workflow Usage Guide

## Overview

Workflow is a **multi-step LLM orchestration process defined in YAML**. It abstracts task patterns with clear stages and review/retry nodes—such as "research → plan → implement → review"—so you don't have to reconstruct them from scratch every time via prompting.

Gasket separates workflow "definition" from "execution" into two layers:

| Layer | Type | Responsibility |
|---|---|---|
| Manifest | `WorkflowManifest` | Data directly deserialized from YAML, syntax validation only |
| Workflow | `Workflow` | Structurally validated, indexed execution graph used at runtime |

This layering ensures issues like "does this step name exist?", "is DONE reachable?", or "was a field misspelled?" fail fast at load time. At runtime, the engine simply executes by index.

## File Location and Discovery

Workflow YAML files reside in the `workflows/` subdirectory of the workspace:

```
$GASKET_WORKSPACE/workflows/
├── dev.yaml
├── self-evolution.yaml
└── *.yaml | *.yml
```

At startup, Gasket scans this directory and dispatches each file to one of two registries based on its `mode` field:

```
mode: tool   →  Registered as a tool in ToolRegistry (invoked by the LLM as a tool)
mode: skill  →  Injected into the system prompt as a skill (the LLM executes steps autonomously)
```

Default is `mode: tool`. Both modes share the same YAML folder.

## YAML Field Reference

```yaml
name: "my_workflow"           # Tool name / skill name (must be unique from the LLM's perspective)
description: "..."             # Tool description; the LLM uses this to decide when to invoke it
mode: tool                     # tool | skill, default is tool
always: true                   # Only effective in skill mode; default true; whether to persist in system prompt
parameters:                    # JSON Schema parameter definition (same as tool parameters)
  type: object
  properties:
    task:
      type: string
      description: "What the user wants to do"
  required: ["task"]
output_template: |             # Optional; final output rendering template
  ## Result
  {{step_name}}
start_step: "first"            # Name of the starting step
steps:
  first:
    prompt: "Do {{input.task}}"
    model: "claude-sonnet-4-6" # Optional; overrides the model for this step
    next: "second"             # Name of the next step, or "DONE" to terminate
  second:
    prompt: "Review {{first}}"
    evaluate:                  # Mutually exclusive with next; branches based on verdict
      on_pass: "DONE"
      on_fail: "first"
      max_retries: 3
```

### Field Constraints

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `name` | string | Yes | — | Must not be empty |
| `description` | string | Yes | — | Must not be empty |
| `parameters` | JSON Schema | Yes | — | Standard JSON Schema |
| `start_step` | string | Yes | — | Must exist in `steps` |
| `steps` | map | Yes | — | At least one step |
| `mode` | enum | No | `tool` | Only `tool` / `skill`; typos (e.g. `skil`) cause an immediate error |
| `always` | bool | No | `true` | Only meaningful in skill mode |
| `output_template` | string | No | — | If unset, returns `{"context": {...}}` JSON |

**Strict schema**: The three structs (`WorkflowManifest`, `WorkflowStepDef`, `EvaluateConfigDef`) all enable `deny_unknown_fields`. Any unknown field (including deprecated `condition`) causes loading to fail. This is intentional—typos must fail early.

### Step Definition

Each step must have at least one of `next` or `evaluate`:

```yaml
steps:
  step_a:
    prompt: "..."
    next: "step_b"           # Unconditional jump
  step_b:
    prompt: "..."
    evaluate:                # Conditional jump, depends on verdict in output
      on_pass: "step_c"
      on_fail: "step_a"
      max_retries: 3
```

- Jump targets must be a name in `steps`, or the literal `"DONE"` to indicate termination
- All steps must be reachable from `start_step` (following `next` / `on_pass` / `on_fail` edges). **Orphan steps cause a loading error**
- Step count limit is 100 (a defensive safeguard against infinite loops)

## Two Execution Modes

### `mode: tool` (State Machine Execution)

The LLM treats the workflow as an ordinary tool call. After the engine receives the call:

1. Starts the state machine at `start_step`
2. Each step launches a sub-agent via `spawn_with_stream` to execute the prompt
3. The sub-agent's output is stored in `context_map`, keyed by step name
4. The next step is determined by `next` or `evaluate`
5. Upon reaching `DONE`, renders `output_template` (or returns JSON if not configured)

Sub-agent streaming events are forwarded to the frontend in real time, so the UI shows progress for each step.

Use cases:

- Processes with clear flow that must execute in strict order
- Steps that need different models (e.g. evaluation with a strong model, execution with a fast one)
- Review nodes with well-defined retry boundaries

### `mode: skill` (Prompt Injection)

The workflow is not registered as a tool, but converted to markdown and injected into the LLM's system prompt. The LLM executes autonomously during the normal conversation loop, **with no sub-agents and no state machine**.

Converted markdown structure:

```markdown
## Workflow: my_workflow

[description]

**Parameters**:
- `task`: What the user wants to do

**Execution Rules**:
1. Execute the following steps in order...
2. Context flows naturally through conversation history...
3. If a step is clearly unnecessary, skip it flexibly but inform the user.

### Execution Steps

#### 1. step_a
[prompt content; {{var}} is replaced with "[see above]"]

#### 2. step_b
[prompt content]

*Review Rules*:
- On pass proceed to: **DONE**
- On fail return to: **step_a** (max retries: 3)

**End Rule**: After all steps are complete, output the final result...
```

Use cases:

- The flow is a "constraint/suggestion" rather than a strict state machine
- You want to reuse the LLM's existing conversation context (no sub-agent overhead)
- Steps are flexible and the LLM may skip or adjust them based on the situation

`always: true` (default) keeps the skill permanently in the system prompt. If you have many skill workflows, be aware that system prompt size grows linearly—set `always: false` and load them on demand via other skill mechanisms.

## Templating and Context

### Template Syntax

`{{key}}` placeholders; allowed characters for keys: `[a-zA-Z0-9_./]`.

```yaml
prompt: |
  Task: {{input.task}}
  Previous result: {{research}}
  Failure reason: {{review.reason}}
```

Unknown keys are preserved as-is (not replaced with an empty string), so the LLM can see which variable was not filled.

### Context Key Naming Conventions

| Key format | Source |
|---|---|
| `input.<param_name>` | Parameters passed when the user invokes the tool |
| `<step_name>` | Final output of that step's sub-agent |
| `<step_name>.reason` | Reason parsed from an `evaluate` step (review nodes only) |

Example: in `dev.yaml`, `{{review.reason}}` is used in `output_template` to represent the last pass/fail reason from the review step.

### output_template

If `output_template` is configured, it is rendered as the final output after the workflow completes; otherwise, all context is serialized to JSON.

```yaml
output_template: |
  ## Result: PASS

  **Final review**: {{review.reason}}

  ### Generated code
  {{implement}}
```

## Review Node: verdict / pass_gate

An `evaluate` step requires the prompt to guide the LLM to output **JSON containing a verdict field**:

```json
{
  "verdict": "PASS",
  "reason": "All checks passed."
}
```

Parsing rules (in priority order):

1. **Preferred**: `verdict: "PASS" | "FAIL"` (case-insensitive)
2. **Compatibility fallback**: `pass_gate: true|false` or `validation_passed: true|false` (prints a deprecation warning)
3. If neither is found → error → treated as FAIL and counted toward retries

`reason` field:

- If `reason` exists (string) → use it directly
- If not → serialize the entire JSON object as a string for the reason (ensures loop-back steps get at least some context)

### LLM Output Tolerance

In practice, LLMs often output "JSON + explanatory text" or "\`\`\`json fences + JSON". The engine uses `serde_json::Deserializer` for streaming extraction:

```
1. Find the first `{`
2. Stream-deserialize from that position, stopping at the matching `}`
3. All subsequent text is ignored
```

So the following are all parseable:

```
{"verdict":"PASS","reason":"ok"}

Note: this run was clean.        ← trailing text, discarded
```

```
Here's my verdict:
{"verdict":"FAIL","reason":"x"}  ← leading text skipped by find('{')
```

```
```json
{"verdict":"PASS","reason":"ok"}
```                              ← markdown fences, discarded
```

### Retry and Exit

- `verdict: PASS` → jump to `on_pass`
- `verdict: FAIL` → jump to `on_fail`, and the failure counter for this node increments by 1
- Failure count > `max_retries` → the entire workflow errors out
- Counting is per "review node index", not cumulative across the whole workflow

## Complete Examples

Refer to the two built-in workflows in the workspace:

**`workspace/workflows/dev.yaml`** (skill mode): research → plan → implement → review loop

```yaml
name: "dev_workflow"
description: "Research → Plan → Implement → Review loop for code generation"
mode: "skill"
output_template: |
  ## Dev Workflow Result: PASS
  - **Final Review**: {{review.reason}}
  ### Generated Code
  {{implement}}
parameters:
  type: object
  properties:
    task:
      type: string
      description: "What to build"
  required: ["task"]
start_step: "research"
steps:
  research:
    prompt: "Research context for: {{input.task}}"
    next: "plan"
  plan:
    prompt: "Plan the implementation. Research: {{research}}"
    next: "implement"
  implement:
    prompt: "Implement the plan: {{plan}}"
    next: "review"
  review:
    prompt: |
      Review the implementation: {{implement}}
      Output JSON: {"verdict":"PASS|FAIL", "reason":"..."}
    evaluate:
      on_pass: "DONE"
      on_fail: "implement"
      max_retries: 3
```

**`workspace/workflows/self-evolution.yaml`** (tool mode): execute → evaluate → diagnose → refine → validate → distill closed loop, demonstrating a topology where the `on_fail` branch is independently reachable.

## Best Practices

### 1. Keep Steps "Atomic"

One step should do one independently reviewable thing. If "research + planning" are combined, the review node cannot determine whether "the research was insufficient or the plan was wrong".

Bad:

```yaml
prepare:
  prompt: "Research and plan: {{input.task}}"
  next: "implement"
```

Good:

```yaml
research:
  prompt: "Research context for: {{input.task}}"
  next: "plan"
plan:
  prompt: "Plan based on research: {{research}}"
  next: "implement"
```

### 2. Review Nodes Must Explicitly Request JSON in the Prompt

Do not assume the LLM knows what schema you expect. Write the target JSON verbatim into the prompt:

```yaml
review:
  prompt: |
    Review the code: {{implement}}
    
    Output ONLY a JSON object in this exact format:
    {"verdict": "PASS" | "FAIL", "reason": "<one sentence>"}
```

The engine tolerates various LLM output formats (see above), but **a clear prompt is far more important than relying on tolerance**—it reduces retry count and saves tokens.

### 3. Do Not Set `max_retries` Above 3

Each retry is a full sub-agent call. Retrying 5 times means the same step can run up to 6 times. If the default 3 retries are still not enough, it usually means:

- The prompt has issues (most common)
- The task exceeds the current model's capabilities
- The review criteria are too strict

Fix the prompt or the review criteria rather than increasing `max_retries`.

### 4. Choose Models Per Step in Tool Mode

Use strong models (Opus/Sonnet) for `evaluate` nodes to ensure strict assessment; use faster models for `execute`/`refine` nodes. Example:

```yaml
execute:
  prompt: "..."
  model: "claude-sonnet-4-6"
  next: "evaluate"
evaluate:
  prompt: "..."
  model: "claude-opus-4-7"     # Strong model for review
  evaluate: { ... }
```

If the model field is omitted, the engine default is used.

### 5. Use `always: true` Sparingly in Skill Mode

Skill workflows stay in the system prompt and are injected into every conversation. If you have 5 skill workflows all with `always: true`, each conversation's system prompt grows by 5 markdown segments—long-context costs increase significantly.

Recommendations:

- General flows (dev, review) → `always: true`
- Specialized flows (project-type-specific) → `always: false`, load on demand via skill invocation

### 6. Write Human-Readable Markdown for `output_template`

The final output of a workflow is for humans, not the LLM. Returning raw `{"context": {...}}` JSON is poor UX.

```yaml
output_template: |
  ## ✅ Dev Workflow Complete
  
  ### Final Review
  {{review.reason}}
  
  ### Implementation
  {{implement}}
  
  ---
  *Generated in {{plan}} planning step → {{implement}} implementation*
```

### 7. Write Detailed Parameter Descriptions

`parameters.properties.<name>.description` is the key information the LLM uses to decide whether to invoke the tool and how to fill in arguments.

Bad:

```yaml
task:
  type: string
  description: "task"        # Equivalent to writing nothing
```

Good:

```yaml
task:
  type: string
  description: "Specific coding task to perform, e.g. 'Add login form validation to /auth/login route'"
```

## Common Pitfalls

### Pitfall 1: Forgetting That `evaluate` Steps Require JSON Output

```yaml
review:
  prompt: "Is the code good?"   # ← Did not request JSON format
  evaluate:
    on_pass: "DONE"
    on_fail: "implement"
```

The LLM returns prose, verdict parsing fails → treated as FAIL → retries 3 times → workflow errors out.

**Fix**: Explicitly request `{"verdict":"...", "reason":"..."}` in the prompt.

### Pitfall 2: YAML Typos Used to Be Silently Swallowed? Not Anymore

Older versions tolerated unknown fields in YAML, so `condtion: ...` (missing an `i`) would be silently discarded.

Now `deny_unknown_fields` is enforced, and loading fails immediately:

```
Failed to parse workflow YAML from ...: unknown field `condtion`, expected one of ...
```

### Pitfall 3: Steps Defined But Unreachable

```yaml
start_step: "a"
steps:
  a:
    prompt: "..."
    next: "DONE"
  b:                # ← Defined but no one points to it
    prompt: "..."
    next: "DONE"
```

Loading error:

```
Workflow has unreachable steps: ["b"]
```

**Fix**: Make `b` reachable via some `next` / `on_pass` / `on_fail`, or delete it.

### Pitfall 4: `mode: "skil"` Typo

`mode` is a strongly typed enum that only accepts `"tool"` and `"skill"`:

```
Failed to parse workflow YAML from ...: unknown variant `skil`, expected `tool` or `skill`
```

### Pitfall 5: Loop Without a `max_retries` Convergence Point

```yaml
a:
  prompt: "..."
  next: "b"
b:
  prompt: "..."
  next: "a"        # ← Unconditionally returns to a, hits MAX_WORKFLOW_STEPS (100) before exiting
```

A non-review node's `next` is an **unconditional jump** with no automatic stop. The 100-step limit is only a safety net; hitting it means 100 wasted sub-agent calls. Loop nodes must use `evaluate` with `max_retries`.

### Pitfall 6: Using `{{step_name}}` for a Nonexistent Step

```yaml
prompt: "Refine based on {{nonexistent}}"
```

`nonexistent` is not in the context_map, so the template preserves `{{nonexistent}}` as-is, and the LLM sees the literal text. **No error is raised, but LLM behavior becomes unpredictable**.

When writing prompts, verify the source of every `{{...}}` placeholder: it should be either `input.<param>` or the name of a previous step.

## Debugging and Troubleshooting

### Check Whether a Workflow Is Loaded

```bash
# Start gasket and watch the logs:
[INFO] Discovered workflow 'dev_workflow' from "..."
[INFO] Discovered workflow-skill 'dev_workflow' from "..."
```

If loading fails, a warning appears in the logs:

```
[WARN] Failed to validate workflow from "...": Workflow has unreachable steps: [...]
```

### Inspect Each Step's Execution

`run_step` outputs via `tracing::info!`:

```
[Workflow dev_workflow] Step 'research' spawning subagent
[Workflow dev_workflow] Step 'research' completed (tools_used: 3)
```

The `#[tracing::instrument(name = "tool.workflow")]` span wraps the entire execute call, so you can filter by workflow name in distributed tracing backends.

### Verdict Parse Failure

```
[WARN] [Workflow ...] Verdict parse failed for step 'review': No JSON object found in output
```

Check the sub-agent's final output (the `subagent_completed` streaming event). Usually the LLM did not output JSON as requested. Fix the prompt.

### Unit Tests

The tests module in `tools/workflow.rs` covers manifest parsing, verdict parsing, graph validation, and other core paths. When adding new workflow fields, add corresponding tests:

```bash
cargo test -p gasket-engine --lib workflow
```

## Relationship with Sub-Agents

In tool mode, each step launches an independent agent via `SubagentSpawner::spawn_with_stream`. This means:

- Each step has a **fresh conversation context**—the previous step's output is explicitly passed via prompt
- Each step has its own tool set (inherited from the parent context) and model selection
- Streaming events are forwarded to the frontend (`subagent_started` / `subagent_completed`)

In skill mode, there are no sub-agents—all steps execute within the parent agent's conversation loop, relying on conversation history to naturally carry context forward.

## When Not to Use Workflow

- **Single-step tasks**: Just let the LLM do it; workflow is extra overhead
- **Frequently changing flows**: YAML is not a good vehicle for prompt engineering; every change requires a restart
- **Complex data transformations between steps**: Template replacement is simple string substitution with no logic—use code for this
- **Parallelism**: The current workflow is a strictly sequential state machine; parallelism requires the `spawn_parallel` tool

## References

- Source code: `gasket/engine/src/tools/workflow.rs`
- Skill mode conversion: `gasket/engine/src/skills/workflow_skill.rs`
- Tool registration: `gasket/engine/src/tools/builder.rs`
- Built-in examples: `workspace/workflows/dev.yaml`, `workspace/workflows/self-evolution.yaml`
