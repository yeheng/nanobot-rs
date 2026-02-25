## ADDED Requirements

### Requirement: Accurate Token Counting via tiktoken-rs

The system SHALL use the `tiktoken-rs` crate with the `cl100k_base` encoding for token counting instead of the current `text.len() / 3` character-based heuristic. The `count_tokens()` function in `history_processor.rs` SHALL be replaced with a tiktoken-based implementation.

#### Scenario: Token counting for English text

- **WHEN** the system estimates tokens for English text
- **THEN** it SHALL return the exact BPE token count from `tiktoken-rs` using the `cl100k_base` encoding

#### Scenario: Token counting for CJK/mixed text

- **WHEN** the system estimates tokens for Chinese, Japanese, Korean, or mixed-language text
- **THEN** it SHALL return the exact BPE token count, which is significantly more accurate than `text.len() / 3` for non-Latin scripts

#### Scenario: tiktoken initialization failure

- **WHEN** the `tiktoken-rs` encoding fails to initialize
- **THEN** the system SHALL log a warning and fall back to `text.len() / 4` as a conservative estimate

### Requirement: Lazy Initialization

The tiktoken encoder SHALL be initialized lazily using `std::sync::OnceLock` and cached for the lifetime of the process. Subsequent calls to `count_tokens()` SHALL reuse the cached encoder with no initialization overhead.

#### Scenario: First token count call

- **WHEN** `count_tokens()` is called for the first time
- **THEN** the system SHALL initialize the `cl100k_base` encoder and cache it

#### Scenario: Subsequent token count calls

- **WHEN** `count_tokens()` is called after initialization
- **THEN** the system SHALL reuse the cached encoder without re-initialization
