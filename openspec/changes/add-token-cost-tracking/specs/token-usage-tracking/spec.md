## ADDED Requirements

### Requirement: Token Usage Tracking

The system SHALL track token usage for each LLM request, including:
- Input tokens (prompt tokens)
- Output tokens (completion tokens)
- Total tokens (input + output)

When the LLM API returns usage information, it SHALL be captured and stored. When the API does not return usage, the system SHALL estimate tokens using `tiktoken-rs` with the `cl100k_base` encoding.

#### Scenario: API returns usage information
- **WHEN** an LLM API response includes a `usage` field
- **THEN** the system SHALL extract and store `prompt_tokens`, `completion_tokens`, and `total_tokens`

#### Scenario: API does not return usage information
- **WHEN** an LLM API response does not include a `usage` field
- **THEN** the system SHALL estimate tokens using `tiktoken-rs` for the request content and response content

#### Scenario: Token estimation fallback
- **WHEN** `tiktoken-rs` fails to initialize
- **THEN** the system SHALL fall back to `text.len() / 4` as a conservative estimate

### Requirement: Model Pricing Configuration

Each provider configuration SHALL support optional pricing fields:
- `price_input_per_million` - Price per million input tokens (in USD or user's currency)
- `price_output_per_million` - Price per million output tokens (in USD or user's currency)
- `currency` - Currency code (default: "USD")

#### Scenario: Provider with pricing configured
- **WHEN** a provider has `price_input_per_million` and `price_output_per_million` configured
- **THEN** costs SHALL be calculated as: `(input_tokens * price_input_per_million / 1_000_000) + (output_tokens * price_output_per_million / 1_000_000)`

#### Scenario: Provider without pricing configured
- **WHEN** a provider does not have pricing configured
- **THEN** cost SHALL be displayed as "N/A" or "Unknown"

#### Scenario: Default pricing for known providers
- **WHEN** a user enables automatic pricing
- **THEN** the system MAY suggest default pricing for known models (e.g., GPT-4o, Claude Sonnet)

### Requirement: Per-Request Token/Cost Output

After each LLM request completes, the system SHALL output:
- Token breakdown (input, output, total)
- Cost estimate (if pricing is configured)
- Cumulative session tokens and cost

#### Scenario: Non-streaming response
- **WHEN** an LLM request completes in non-streaming mode
- **THEN** the system SHALL display token count and cost immediately after the response

#### Scenario: Streaming response
- **WHEN** an LLM request completes in streaming mode
- **THEN** the system SHALL display token count and cost after the stream ends

#### Scenario: Tool call iteration
- **WHEN** the agent makes multiple LLM calls due to tool use
- **THEN** each iteration's token usage SHALL be tracked separately and included in the cumulative total

### Requirement: Session-Level Token/Cost Summary

At the end of a conversation session, the system SHALL output:
- Total tokens consumed (input, output, combined)
- Total estimated cost
- Number of LLM requests made
- Average tokens per request

#### Scenario: CLI session end
- **WHEN** a user exits the CLI or starts a new session with `/new`
- **THEN** the system SHALL display a session summary with total tokens and cost

#### Scenario: API session end
- **WHEN** a conversation session ends via an external channel (Telegram, Slack, etc.)
- **THEN** the system MAY log session statistics (configurable per channel)

## MODIFIED Requirements

### Requirement: Chat Response Structure

The `ChatResponse` struct SHALL include an optional `Usage` field containing token counts.

#### Scenario: Parsing API response with usage
- **WHEN** deserializing an API response that includes `usage`
- **THEN** the `Usage` struct SHALL be populated with `prompt_tokens`, `completion_tokens`, and `total_tokens`

#### Scenario: Parsing API response without usage
- **WHEN** deserializing an API response without `usage`
- **THEN** the `Usage` field SHALL be `None`, triggering client-side estimation

### Requirement: Provider Configuration Schema

The `ProviderConfig` struct SHALL include optional pricing fields for cost calculation.

#### Scenario: Loading provider config with pricing
- **WHEN** loading configuration that includes pricing fields
- **THEN** the system SHALL parse and store pricing information for the provider

#### Scenario: Loading provider config without pricing
- **WHEN** loading configuration without pricing fields
- **THEN** the system SHALL treat the provider as having no pricing (costs displayed as "N/A")
