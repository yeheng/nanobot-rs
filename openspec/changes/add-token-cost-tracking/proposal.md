# Change: Add Token Statistics and Cost Calculation

## Why

Users need visibility into LLM usage costs and token consumption for:
- Budget tracking and cost awareness
- Optimizing prompt efficiency
- Understanding token usage patterns across conversations
- Comparing cost-effectiveness of different models

Currently, the system has token counting via `tiktoken-rs` for history truncation, but does not:
1. Report token usage after each LLM response
2. Track costs based on model-specific pricing
3. Provide session-level token/cost summaries

## What Changes

- Add token usage tracking to `ChatResponse` to capture `usage` from API responses
- Add model pricing configuration to `ProviderConfig` with input/output price per million tokens
- Calculate and display cost estimates after each LLM request
- Output per-request token breakdown (input tokens, output tokens, total)
- Output session-level token and cost summary at conversation end
- Use `tiktoken-rs` for client-side token verification when API doesn't return usage

## Impact

- **Affected specs**:
  - `provider-config` - Add pricing configuration fields
  - `chat-response` - Add usage tracking fields
  - `agent-loop` - Add token/cost reporting after requests
  - `session-management` - Track cumulative token usage per session

- **Affected code**:
  - `nanobot-core/src/providers/base.rs` - Add `Usage` struct to `ChatResponse`
  - `nanobot-core/src/providers/common.rs` - Parse `usage` from API responses
  - `nanobot-core/src/config/schema.rs` - Add pricing config to `ProviderConfig`
  - `nanobot-core/src/agent/loop_.rs` - Output token/cost after requests
  - `nanobot-core/src/agent/stream.rs` - Stream token/cost events
  - `nanobot-core/Cargo.toml` - Already has `tiktoken-rs` dependency

- **New modules**:
  - `nanobot-core/src/token_tracker.rs` - Token counting and cost calculation logic

## Breaking Changes

None - this is a backward-compatible addition. Pricing configuration is optional. 中文和我交互。
