## 1. Core Implementation

- [x] 1.1 Create `nanobot-core/src/token_tracker.rs` module with:
  - `TokenUsage` struct (input_tokens, output_tokens, total_tokens)
  - `CostCalculator` struct with pricing configuration
  - `calculate_cost(input_tokens, output_tokens, pricing)` function
  - `estimate_tokens(text)` function using tiktoken-rs

- [x] 1.2 Add `Usage` struct to `nanobot-core/src/providers/base.rs`:
  - Add `pub usage: Option<Usage>` field to `ChatResponse`
  - Update deserialization to parse `usage` from API responses

- [x] 1.3 Update `nanobot-core/src/providers/common.rs`:
  - Add `usage` field to `OpenAICompatibleResponse`
  - Parse and forward usage in `chat()` method
  - Handle streaming responses (usage typically comes in final chunk)

- [x] 1.4 Update `nanobot-core/src/config/schema.rs`:
  - Add `price_input_per_million: Option<f64>` to `ProviderConfig`
  - Add `price_output_per_million: Option<f64>` to `ProviderConfig`
  - Add `currency: Option<String>` to `ProviderConfig` (default: "USD")

## 2. Agent Loop Integration

- [x] 2.1 Update `nanobot-core/src/agent/loop_.rs`:
  - Add token/cost logging after each LLM response
  - Track cumulative tokens across tool call iterations
  - Output summary at end of `process_direct_with_callback`

- [x] 2.2 Update `nanobot-core/src/agent/stream.rs`:
  - Add `StreamEvent::TokenStats { usage, cost }` variant
  - Emit token stats event when stream completes

## 3. CLI Output

- [x] 3.1 Update CLI to display token/cost information:
  - Format output nicely with aligned columns
  - Show per-request breakdown
  - Show session cumulative totals

- [x] 3.2 Add `/stats` command to show current session statistics on demand

## 4. Testing

- [x] 4.1 Unit tests for `CostCalculator`:
  - Test cost calculation with various pricing tiers
  - Test edge cases (zero tokens, missing pricing)

- [x] 4.2 Unit tests for `TokenUsage`:
  - Test parsing from API responses
  - Test estimation fallback

- [ ] 4.3 Integration tests:
  - Verify token counting matches expected values
  - Verify cost calculation accuracy

## 5. Documentation

- [ ] 5.1 Update configuration documentation with pricing examples:
  - Show how to configure pricing for common providers
  - Include default pricing reference table

- [ ] 5.2 Add example output screenshots to README

## Dependencies

- Task 1 must complete before Task 2
- Task 2 must complete before Task 3
- Tests (Task 4) can be written alongside implementation
