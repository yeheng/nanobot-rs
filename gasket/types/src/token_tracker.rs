//! Token usage tracking and cost calculation
//!
//! Provides token counting and budget enforcement for parent and subagent coordination.

use serde::{Deserialize, Serialize};

/// Token usage information from an LLM response
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenUsage {
    /// Number of tokens in the input/prompt
    pub input_tokens: usize,
    /// Number of tokens in the output/completion
    pub output_tokens: usize,
    /// Total tokens (input + output)
    pub total_tokens: usize,
}

impl TokenUsage {
    /// Create a new TokenUsage from input and output token counts
    pub fn new(input_tokens: usize, output_tokens: usize) -> Self {
        Self {
            input_tokens,
            output_tokens,
            total_tokens: input_tokens + output_tokens,
        }
    }

    /// Create TokenUsage from API response fields
    pub fn from_api_fields(prompt_tokens: usize, completion_tokens: usize) -> Self {
        Self::new(prompt_tokens, completion_tokens)
    }
}

/// Pricing configuration for a model
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelPricing {
    /// Price per million input tokens (in USD or configured currency)
    pub price_input_per_million: f64,
    /// Price per million output tokens (in USD or configured currency)
    pub price_output_per_million: f64,
    /// Currency code (e.g., "USD", "CNY")
    pub currency: String,
}

impl ModelPricing {
    /// Create new pricing configuration
    pub fn new(
        price_input_per_million: f64,
        price_output_per_million: f64,
        currency: &str,
    ) -> Self {
        Self {
            price_input_per_million,
            price_output_per_million,
            currency: currency.to_string(),
        }
    }

    /// Calculate cost for given token counts
    pub fn calculate_cost(&self, input_tokens: usize, output_tokens: usize) -> f64 {
        let input_cost = (input_tokens as f64) * self.price_input_per_million / 1_000_000.0;
        let output_cost = (output_tokens as f64) * self.price_output_per_million / 1_000_000.0;
        input_cost + output_cost
    }
}

/// Token and cost tracking for a session
#[derive(Debug, Clone, Default)]
pub struct SessionTokenStats {
    /// Total input tokens across all requests
    pub total_input_tokens: usize,
    /// Total output tokens across all requests
    pub total_output_tokens: usize,
    /// Total cost accumulated
    pub total_cost: f64,
    /// Number of LLM requests made
    pub request_count: usize,
    /// Currency for cost display
    pub currency: String,
}

impl SessionTokenStats {
    /// Create a new session stats tracker
    pub fn new(currency: &str) -> Self {
        Self {
            currency: currency.to_string(),
            ..Default::default()
        }
    }

    /// Add token usage from a single request
    pub fn add_usage(&mut self, usage: &TokenUsage, cost: f64) {
        self.total_input_tokens += usage.input_tokens;
        self.total_output_tokens += usage.output_tokens;
        self.total_cost += cost;
        self.request_count += 1;
    }

    /// Get total tokens (input + output)
    pub fn total_tokens(&self) -> usize {
        self.total_input_tokens + self.total_output_tokens
    }

    /// Get average tokens per request
    pub fn avg_tokens_per_request(&self) -> f64 {
        if self.request_count == 0 {
            return 0.0;
        }
        self.total_tokens() as f64 / self.request_count as f64
    }

    /// Format a summary string for display
    pub fn format_summary(&self) -> String {
        let currency_symbol = if self.currency == "CNY" { "¥" } else { "$" };
        format!(
            "\n[Session Summary]\n  \
             Requests: {}\n  \
             Total Tokens: {} (Input: {} | Output: {})\n  \
             Total Cost: {}{:.4}",
            self.request_count,
            self.total_tokens(),
            self.total_input_tokens,
            self.total_output_tokens,
            currency_symbol,
            self.total_cost
        )
    }
}

/// Shared token tracker for budget enforcement across parent and subagents.
///
/// Uses Arc for shared ownership - parent and subagents all accumulate
/// to the same tracker, enabling unified budget enforcement.
#[derive(Debug, Default)]
pub struct TokenTracker {
    /// Total input tokens across all requests (including subagents)
    total_input_tokens: std::sync::atomic::AtomicUsize,
    /// Total output tokens across all requests (including subagents)
    total_output_tokens: std::sync::atomic::AtomicUsize,
    /// Total cost accumulated (including subagents)
    total_cost: std::sync::atomic::AtomicU64,
    /// Number of LLM requests made (including subagents)
    request_count: std::sync::atomic::AtomicUsize,
    /// Optional budget limit (0 = unlimited)
    budget_limit: std::sync::atomic::AtomicU64,
    /// Currency for cost display (immutable after construction)
    currency: String,
}

impl TokenTracker {
    /// Create a new token tracker with optional budget limit.
    pub fn new(currency: &str, budget_limit: Option<f64>) -> Self {
        Self {
            currency: currency.to_string(),
            budget_limit: std::sync::atomic::AtomicU64::new(
                budget_limit.map(|b| b.to_bits()).unwrap_or(0),
            ),
            ..Default::default()
        }
    }

    /// Create a new token tracker without budget limit.
    pub fn unlimited(currency: &str) -> Self {
        Self::new(currency, None)
    }

    /// Accumulate token usage from a single request.
    ///
    /// Uses a CAS (Compare-And-Swap) loop for atomic float addition.
    /// `fetch_add` on `f64::to_bits()` would perform integer addition on IEEE 754
    /// bit patterns, producing garbage results (e.g., 1.0 + 1.0 → ~1.8e308).
    pub fn accumulate(&self, usage: &TokenUsage, cost: f64) {
        self.total_input_tokens
            .fetch_add(usage.input_tokens, std::sync::atomic::Ordering::Relaxed);
        self.total_output_tokens
            .fetch_add(usage.output_tokens, std::sync::atomic::Ordering::Relaxed);
        // CAS loop: correctly add floating-point values atomically
        loop {
            let current_bits = self.total_cost.load(std::sync::atomic::Ordering::Relaxed);
            let current_cost = f64::from_bits(current_bits);
            let new_bits = (current_cost + cost).to_bits();
            match self.total_cost.compare_exchange_weak(
                current_bits,
                new_bits,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(_) => continue, // another thread modified it, retry
            }
        }
        self.request_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Get total input tokens
    pub fn input_tokens(&self) -> usize {
        self.total_input_tokens
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Get total output tokens
    pub fn output_tokens(&self) -> usize {
        self.total_output_tokens
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Get total tokens (input + output)
    pub fn total_tokens(&self) -> usize {
        self.input_tokens() + self.output_tokens()
    }

    /// Get total cost
    pub fn total_cost(&self) -> f64 {
        let bits = self.total_cost.load(std::sync::atomic::Ordering::Relaxed);
        f64::from_bits(bits)
    }

    /// Get request count
    pub fn request_count(&self) -> usize {
        self.request_count
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Get budget limit (0 = unlimited)
    pub fn budget_limit(&self) -> f64 {
        let bits = self.budget_limit.load(std::sync::atomic::Ordering::Relaxed);
        f64::from_bits(bits)
    }

    /// Check if budget is exceeded
    pub fn is_budget_exceeded(&self) -> bool {
        let limit = self.budget_limit();
        limit > 0.0 && self.total_cost() > limit
    }

    /// Get remaining budget (returns None if unlimited)
    pub fn remaining_budget(&self) -> Option<f64> {
        let limit = self.budget_limit();
        if limit > 0.0 {
            Some(limit - self.total_cost())
        } else {
            None
        }
    }

    /// Get currency
    pub fn currency(&self) -> &str {
        &self.currency
    }

    /// Format a summary string for display
    pub fn format_summary(&self) -> String {
        let currency = self.currency();
        let currency_symbol = if currency == "CNY" { "¥" } else { "$" };
        let budget_info = if self.budget_limit() > 0.0 {
            format!(
                " (Budget: {}{:.4}, Remaining: {}{:.4})",
                currency_symbol,
                self.budget_limit(),
                currency_symbol,
                self.remaining_budget().unwrap_or(0.0)
            )
        } else {
            String::new()
        };

        format!(
            "\n[Token Summary]\n  \
             Requests: {}\n  \
             Total Tokens: {} (Input: {} | Output: {})\n  \
             Total Cost: {}{:.4}{}",
            self.request_count(),
            self.total_tokens(),
            self.input_tokens(),
            self.output_tokens(),
            currency_symbol,
            self.total_cost(),
            budget_info
        )
    }

    /// Convert to SessionTokenStats for compatibility
    pub fn to_session_stats(&self) -> SessionTokenStats {
        SessionTokenStats {
            total_input_tokens: self.input_tokens(),
            total_output_tokens: self.output_tokens(),
            total_cost: self.total_cost(),
            request_count: self.request_count(),
            currency: self.currency().to_string(),
        }
    }
}

/// Calculate cost for token usage given optional pricing
pub fn calculate_cost(usage: &TokenUsage, pricing: Option<&ModelPricing>) -> f64 {
    match pricing {
        Some(p) => p.calculate_cost(usage.input_tokens, usage.output_tokens),
        None => 0.0,
    }
}

/// Format token usage for display
pub fn format_token_usage(usage: &TokenUsage) -> String {
    format!(
        "Input: {} | Output: {} | Total: {}",
        usage.input_tokens, usage.output_tokens, usage.total_tokens
    )
}

/// Format cost for display
pub fn format_cost(cost: f64, currency: &str) -> String {
    let symbol = if currency == "CNY" { "¥" } else { "$" };
    format!("{}{:.4}", symbol, cost)
}
