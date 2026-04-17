//! Token usage tracking and cost calculation
//!
//! Provides token counting and budget enforcement for parent and subagent coordination.
//!
//! Cost tracking uses `parking_lot::Mutex<f64>` instead of fixed-point `AtomicU64`.
//! For a personal AI assistant with single-digit subagent concurrency, uncontended
//! mutex lock/unlock is nanosecond-scale — the CAS-loop and scaling overhead of
//! `AtomicU64` was pure mental tax with no measurable benefit.

use parking_lot::Mutex;
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
/// Uses `Arc` for shared ownership — parent and subagents all accumulate
/// to the same tracker, enabling unified budget enforcement.
///
/// Cost accumulation uses `parking_lot::Mutex<f64>` for simplicity.
/// Uncontended lock/unlock is ~2ns, negligible compared to LLM round-trips.
#[derive(Debug)]
pub struct TokenTracker {
    /// Total input tokens across all requests (including subagents)
    total_input_tokens: std::sync::atomic::AtomicUsize,
    /// Total output tokens across all requests (including subagents)
    total_output_tokens: std::sync::atomic::AtomicUsize,
    /// Total cost accumulated (including subagents) — direct f64, no scaling
    total_cost: Mutex<f64>,
    /// Number of LLM requests made (including subagents)
    request_count: std::sync::atomic::AtomicUsize,
    /// Optional budget limit (0.0 = unlimited, immutable after construction)
    budget_limit: f64,
    /// Currency for cost display (immutable after construction)
    currency: String,
}

impl Default for TokenTracker {
    fn default() -> Self {
        Self {
            total_input_tokens: std::sync::atomic::AtomicUsize::new(0),
            total_output_tokens: std::sync::atomic::AtomicUsize::new(0),
            total_cost: Mutex::new(0.0),
            request_count: std::sync::atomic::AtomicUsize::new(0),
            budget_limit: 0.0,
            currency: String::new(),
        }
    }
}

impl TokenTracker {
    /// Create a new token tracker with optional budget limit.
    pub fn new(currency: &str, budget_limit: Option<f64>) -> Self {
        Self {
            currency: currency.to_string(),
            budget_limit: budget_limit.unwrap_or(0.0),
            ..Default::default()
        }
    }

    /// Create a new token tracker without budget limit.
    pub fn unlimited(currency: &str) -> Self {
        Self::new(currency, None)
    }

    /// Accumulate token usage from a single request.
    ///
    /// Token counts use atomics (lock-free). Cost uses a `Mutex<f64>` —
    /// uncontended lock is ~2ns, dwarfed by any LLM round-trip.
    pub fn accumulate(&self, usage: &TokenUsage, cost: f64) {
        self.total_input_tokens
            .fetch_add(usage.input_tokens, std::sync::atomic::Ordering::Relaxed);
        self.total_output_tokens
            .fetch_add(usage.output_tokens, std::sync::atomic::Ordering::Relaxed);

        *self.total_cost.lock() += cost;

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
        *self.total_cost.lock()
    }

    /// Get request count
    pub fn request_count(&self) -> usize {
        self.request_count
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Get budget limit (0.0 = unlimited)
    pub fn budget_limit(&self) -> f64 {
        self.budget_limit
    }

    /// Check if budget is exceeded
    pub fn is_budget_exceeded(&self) -> bool {
        self.budget_limit > 0.0 && *self.total_cost.lock() > self.budget_limit
    }

    /// Get remaining budget (returns None if unlimited)
    pub fn remaining_budget(&self) -> Option<f64> {
        if self.budget_limit > 0.0 {
            let spent = *self.total_cost.lock();
            Some((self.budget_limit - spent).max(0.0))
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
        let budget_info = if self.budget_limit > 0.0 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

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
        assert!((tracker.total_cost() - 0.05).abs() < 1e-10);
    }

    #[test]
    fn test_token_tracker_budget_enforcement() {
        let tracker = TokenTracker::new("USD", Some(0.1));
        let usage = TokenUsage::new(100, 50);
        tracker.accumulate(&usage, 0.05);

        assert!(!tracker.is_budget_exceeded());
        assert!(tracker.remaining_budget().is_some());
        assert!((tracker.remaining_budget().unwrap() - 0.05).abs() < 1e-10);

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
        assert!((stats.total_cost - 0.05).abs() < 1e-10);
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
    fn test_repeated_accumulation_precision() {
        // Direct f64 accumulation — no fixed-point needed
        let tracker = TokenTracker::unlimited("USD");
        for _ in 0..1_000 {
            tracker.accumulate(&TokenUsage::new(1, 0), 0.000_001);
        }
        // 1_000 * 0.000_001 = 0.001
        assert!((tracker.total_cost() - 0.001).abs() < 1e-12);
    }

    /// Test Case: test_token_tracker_simplicity
    /// 100 concurrent tasks accumulating costs. Final total_cost must be exact.
    #[test]
    fn test_token_tracker_simplicity() {
        let tracker = Arc::new(TokenTracker::unlimited("USD"));
        let num_tasks = 100;
        let cost_per_task = 0.01;

        let handles: Vec<_> = (0..num_tasks)
            .map(|_| {
                let t = tracker.clone();
                thread::spawn(move || {
                    t.accumulate(&TokenUsage::new(100, 50), cost_per_task);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(tracker.request_count(), num_tasks);
        assert_eq!(tracker.input_tokens(), num_tasks * 100);
        assert_eq!(tracker.output_tokens(), num_tasks * 50);
        let expected = num_tasks as f64 * cost_per_task;
        assert!(
            (tracker.total_cost() - expected).abs() < 1e-10,
            "expected {}, got {}",
            expected,
            tracker.total_cost()
        );
    }
}
