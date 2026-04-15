//! Token usage tracking and cost calculation
//!
//! Provides token counting and budget enforcement for parent and subagent coordination.

use serde::{Deserialize, Serialize};

/// Scaling factor for fixed-point cost storage (1 billion = 10^9).
///
/// This allows micro-cent precision without the dangers of concurrent f64 CAS.
pub const COST_SCALE: u64 = 1_000_000_000;

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

    /// Calculate cost as a fixed-point integer (scaled by [`COST_SCALE`]).
    pub fn calculate_cost_scaled(&self, input_tokens: usize, output_tokens: usize) -> u64 {
        let cost = self.calculate_cost(input_tokens, output_tokens);
        (cost * COST_SCALE as f64).round() as u64
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
    /// Total cost accumulated as fixed-point micro-cents (including subagents)
    total_cost: std::sync::atomic::AtomicU64,
    /// Number of LLM requests made (including subagents)
    request_count: std::sync::atomic::AtomicUsize,
    /// Optional budget limit in fixed-point (0 = unlimited)
    budget_limit: std::sync::atomic::AtomicU64,
    /// Currency for cost display (immutable after construction)
    currency: String,
}

impl TokenTracker {
    /// Create a new token tracker with optional budget limit.
    pub fn new(currency: &str, budget_limit: Option<f64>) -> Self {
        let scaled_limit = budget_limit
            .map(|b| (b * COST_SCALE as f64).round() as u64)
            .unwrap_or(0);
        Self {
            currency: currency.to_string(),
            budget_limit: std::sync::atomic::AtomicU64::new(scaled_limit),
            ..Default::default()
        }
    }

    /// Create a new token tracker without budget limit.
    pub fn unlimited(currency: &str) -> Self {
        Self::new(currency, None)
    }

    /// Accumulate token usage from a single request.
    ///
    /// Uses a single atomic `fetch_add` on a fixed-point integer.
    /// The floating-point `cost` is converted to a scaled `u64` before
    /// accumulation, eliminating the need for a CAS loop and avoiding
    /// the associativity problems of concurrent f64 addition.
    pub fn accumulate(&self, usage: &TokenUsage, cost: f64) {
        self.total_input_tokens
            .fetch_add(usage.input_tokens, std::sync::atomic::Ordering::Relaxed);
        self.total_output_tokens
            .fetch_add(usage.output_tokens, std::sync::atomic::Ordering::Relaxed);

        let cost_scaled = (cost * COST_SCALE as f64).round() as u64;
        self.total_cost
            .fetch_add(cost_scaled, std::sync::atomic::Ordering::Relaxed);

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
        let scaled = self.total_cost.load(std::sync::atomic::Ordering::Relaxed);
        scaled as f64 / COST_SCALE as f64
    }

    /// Get request count
    pub fn request_count(&self) -> usize {
        self.request_count
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Get budget limit (0 = unlimited)
    pub fn budget_limit(&self) -> f64 {
        let scaled = self.budget_limit.load(std::sync::atomic::Ordering::Relaxed);
        scaled as f64 / COST_SCALE as f64
    }

    /// Check if budget is exceeded
    pub fn is_budget_exceeded(&self) -> bool {
        let limit = self.budget_limit.load(std::sync::atomic::Ordering::Relaxed);
        limit > 0 && self.total_cost.load(std::sync::atomic::Ordering::Relaxed) > limit
    }

    /// Get remaining budget (returns None if unlimited)
    pub fn remaining_budget(&self) -> Option<f64> {
        let limit = self.budget_limit.load(std::sync::atomic::Ordering::Relaxed);
        if limit > 0 {
            let spent = self.total_cost.load(std::sync::atomic::Ordering::Relaxed);
            Some(scaled_as_f64(limit.saturating_sub(spent)))
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

fn scaled_as_f64(scaled: u64) -> f64 {
    scaled as f64 / COST_SCALE as f64
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_usage_creation() {
        let usage = TokenUsage::new(100, 50);
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.total_tokens, 150);
    }

    #[test]
    fn test_model_pricing_calculation() {
        let pricing = ModelPricing::new(1.0, 2.0, "USD");
        let cost = pricing.calculate_cost(1_000_000, 500_000);
        assert!((cost - 2.0).abs() < 0.0001);
    }

    #[test]
    fn test_model_pricing_zero_tokens() {
        let pricing = ModelPricing::new(1.0, 2.0, "USD");
        let cost = pricing.calculate_cost(0, 0);
        assert_eq!(cost, 0.0);
    }

    #[test]
    fn test_token_tracker_accumulate_usage() {
        let tracker = TokenTracker::unlimited("USD");
        let usage = TokenUsage::new(100, 50);
        tracker.accumulate(&usage, 0.05);

        assert_eq!(tracker.input_tokens(), 100);
        assert_eq!(tracker.output_tokens(), 50);
        assert_eq!(tracker.total_tokens(), 150);
        assert_eq!(tracker.request_count(), 1);
        assert!((tracker.total_cost() - 0.05).abs() < 0.0000001);
    }

    #[test]
    fn test_token_tracker_budget_enforcement() {
        let tracker = TokenTracker::new("USD", Some(0.1));
        let usage = TokenUsage::new(100, 50);
        tracker.accumulate(&usage, 0.05);

        assert!(!tracker.is_budget_exceeded());
        assert!(tracker.remaining_budget().is_some());
        assert!((tracker.remaining_budget().unwrap() - 0.05).abs() < 0.0000001);

        tracker.accumulate(&usage, 0.06);
        assert!(tracker.is_budget_exceeded());
    }

    #[test]
    fn test_token_tracker_no_pricing() {
        let tracker = TokenTracker::unlimited("USD");
        let usage = TokenUsage::new(100, 50);
        tracker.accumulate(&usage, 0.0);

        assert_eq!(tracker.total_cost(), 0.0);
        assert!(!tracker.is_budget_exceeded());
        assert!(tracker.remaining_budget().is_none());
    }

    #[test]
    fn test_calculate_cost_with_pricing() {
        let pricing = ModelPricing::new(10.0, 20.0, "USD");
        let usage = TokenUsage::new(1_000_000, 500_000);
        let cost = calculate_cost(&usage, Some(&pricing));
        assert!((cost - 20.0).abs() < 0.0001);
    }

    #[test]
    fn test_calculate_cost_without_pricing() {
        let usage = TokenUsage::new(1_000_000, 500_000);
        let cost = calculate_cost(&usage, None);
        assert_eq!(cost, 0.0);
    }

    #[test]
    fn test_format_token_usage() {
        let usage = TokenUsage::new(100, 50);
        let formatted = format_token_usage(&usage);
        assert_eq!(formatted, "Input: 100 | Output: 50 | Total: 150");
    }

    #[test]
    fn test_format_cost() {
        assert_eq!(format_cost(1.2345, "USD"), "$1.2345");
        assert_eq!(format_cost(1.2345, "CNY"), "¥1.2345");
    }

    #[test]
    fn test_session_stats_accumulation() {
        let mut stats = SessionTokenStats::new("USD");
        let usage = TokenUsage::new(100, 50);
        stats.add_usage(&usage, 0.05);

        assert_eq!(stats.total_input_tokens, 100);
        assert_eq!(stats.total_output_tokens, 50);
        assert_eq!(stats.request_count, 1);
        assert!((stats.total_cost - 0.05).abs() < 0.0000001);
    }

    #[test]
    fn test_session_stats_avg_tokens() {
        let mut stats = SessionTokenStats::new("USD");
        stats.add_usage(&TokenUsage::new(100, 50), 0.05);
        stats.add_usage(&TokenUsage::new(200, 100), 0.10);

        assert_eq!(stats.avg_tokens_per_request(), 225.0);
    }

    #[test]
    fn test_session_stats_format_summary() {
        let mut stats = SessionTokenStats::new("USD");
        stats.add_usage(&TokenUsage::new(100, 50), 0.05);
        let summary = stats.format_summary();
        assert!(summary.contains("Requests: 1"));
        assert!(summary.contains("Total Tokens: 150"));
        assert!(summary.contains("$0.0500"));
    }

    #[test]
    fn test_token_usage_from_api_fields() {
        let usage = TokenUsage::from_api_fields(1000, 500);
        assert_eq!(usage.input_tokens, 1000);
        assert_eq!(usage.output_tokens, 500);
        assert_eq!(usage.total_tokens, 1500);
    }

    #[test]
    fn test_model_pricing_calculation_cny() {
        let pricing = ModelPricing::new(1.0, 2.0, "CNY");
        assert_eq!(pricing.currency, "CNY");
        let cost = pricing.calculate_cost(1_000_000, 500_000);
        assert!((cost - 2.0).abs() < 0.0001);
    }

    #[test]
    fn test_fixed_point_precision() {
        // Verify that repeated small additions are exact with fixed-point
        let tracker = TokenTracker::unlimited("USD");
        for _ in 0..1_000 {
            tracker.accumulate(&TokenUsage::new(1, 0), 0.000_001);
        }
        // 1_000 * 0.000_001 = 0.001
        assert!((tracker.total_cost() - 0.001).abs() < 1e-12);
    }
}
