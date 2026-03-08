## Context

Token tracking and cost calculation are important for users to:
1. Understand their LLM spending
2. Optimize prompt efficiency
3. Compare model cost-effectiveness
4. Stay within budget constraints

The system already uses `tiktoken-rs` for history truncation, providing a foundation for client-side token estimation.

## Goals / Non-Goals

**Goals:**
- Accurate token counting (API usage when available, tiktoken-rs as fallback)
- Flexible pricing configuration per provider
- Clear, non-intrusive output of token/cost information
- Session-level cumulative tracking

**Non-Goals:**
- Budget enforcement or spending limits (future feature)
- Real-time cost alerts (future feature)
- Multi-currency conversion (future feature)
- Historical cost tracking across sessions (future feature)

## Decisions

### Decision 1: Token Counting Strategy

**Approach:** Hybrid - API usage first, client-side estimation as fallback

**Rationale:**
- API usage is authoritative when provided
- Some providers don't return usage in streaming mode
- tiktoken-rs with cl100k_base works for most models (GPT-4, GPT-3.5, Claude via OpenRouter)
- Fallback to `len/4` if tiktoken fails

**Alternatives Considered:**
- Client-side counting only: Less accurate, redundant work
- API-only counting: Fails for providers that don't return usage

### Decision 2: Pricing Configuration Location

**Approach:** Per-provider configuration in YAML config file

**Rationale:**
- Users can mix providers (OpenAI, OpenRouter, local models)
- Different providers have different pricing tiers
- Config file is already the source of truth for provider settings

**Example:**
```yaml
providers:
  openai:
    api_key: sk-xxx
    price_input_per_million: 2.5
    price_output_per_million: 10.0
    currency: USD

  openrouter:
    api_key: sk-or-xxx
    price_input_per_million: 3.0
    price_output_per_million: 15.0
```

### Decision 3: Output Location

**Approach:** Output after each response and at session end

**Rationale:**
- Per-request output gives immediate feedback
- Session summary provides cumulative view
- Non-intrusive (after response, not during)

**Format:**
```
[Token Usage] Input: 1,234 | Output: 567 | Total: 1,801
[Cost] $0.0087 (at $2.50/M input, $10.00/M output)

[Session Summary]
  Requests: 5
  Total Tokens: 12,345 (Input: 8,000 | Output: 4,345)
  Total Cost: $0.063
```

### Decision 4: Streaming Token Counting

**Approach:** Accumulate tokens from stream chunks, calculate cost at end

**Rationale:**
- Streaming APIs may not include usage in each chunk
- Final chunk often contains usage summary
- Client-side accumulation works when API doesn't provide usage

## Risks / Trade-offs

| Risk | Mitigation |
|------|------------|
| Token estimation inaccuracy | Use API usage when available; tiktoken-rs is accurate for most models |
| Pricing changes over time | Users must update config; consider auto-fetching pricing in future |
| Performance overhead | Token counting is O(n) but fast; cost calculation is trivial |
| Cluttered output | Format cleanly; make output configurable (opt-out) |

## Migration Plan

No migration required - this is a new feature with backward-compatible config changes:
- Existing configs without pricing fields continue to work
- Pricing defaults to "N/A" when not configured
- Token counting works without any config changes

## Open Questions

1. Should we provide default pricing for known models?
   - Pro: Better UX, immediate value
   - Con: Pricing changes, maintenance burden

2. Should token/cost output be configurable per channel?
   - Pro: Flexibility for different use cases
   - Con: Added complexity

3. Should we track tokens per tool call vs. per conversation turn?
   - Pro: More granular cost attribution
   - Con: More complex tracking, potentially noisy output
