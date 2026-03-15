//! Token usage tracking and cost calculation
//!
//! Provides token counting using tiktoken-rs and cost estimation based on provider pricing.

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use tiktoken_rs::CoreBPE;
use tracing::warn;

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

/// Global cached BPE encoder (cl100k_base, covers GPT-4/GPT-3.5).
static ENCODER: OnceLock<Option<CoreBPE>> = OnceLock::new();

fn get_encoder() -> Option<&'static CoreBPE> {
    ENCODER
        .get_or_init(|| match tiktoken_rs::cl100k_base() {
            Ok(enc) => Some(enc),
            Err(e) => {
                warn!(
                    "Failed to init tiktoken cl100k_base encoder: {}. Falling back to len/4.",
                    e
                );
                None
            }
        })
        .as_ref()
}

/// Estimate tokens in text using tiktoken-rs.
///
/// Falls back to `text.len() / 4` if the encoder fails to initialize.
pub fn estimate_tokens(text: &str) -> usize {
    match get_encoder() {
        Some(enc) => enc.encode_with_special_tokens(text).len(),
        None => text.len() / 4,
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

/// Format token/cost info for a single request
pub fn format_request_stats(
    usage: &TokenUsage,
    cost: f64,
    currency: &str,
    pricing: Option<&ModelPricing>,
) -> String {
    let token_line = format!("[Token Usage] {}", format_token_usage(usage));

    let cost_line = if cost > 0.0 {
        let pricing_info = if let Some(p) = pricing {
            format!(
                " (at ${:.2}/M input, ${:.2}/M output)",
                p.price_input_per_million, p.price_output_per_million
            )
        } else {
            "".to_string()
        };
        format!("[Cost] {}{}", format_cost(cost, currency), pricing_info)
    } else {
        "[Cost] N/A (pricing not configured)".to_string()
    };

    format!("{}\n{}", token_line, cost_line)
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
    fn test_token_usage_from_api() {
        let usage = TokenUsage::from_api_fields(200, 100);
        assert_eq!(usage.input_tokens, 200);
        assert_eq!(usage.output_tokens, 100);
        assert_eq!(usage.total_tokens, 300);
    }

    #[test]
    fn test_model_pricing_calculation() {
        let pricing = ModelPricing::new(2.5, 10.0, "USD");
        let cost = pricing.calculate_cost(1000, 500);
        // (1000 * 2.5 / 1_000_000) + (500 * 10.0 / 1_000_000) = 0.0025 + 0.005 = 0.0075
        assert!((cost - 0.0075).abs() < 0.0001);
    }

    #[test]
    fn test_model_pricing_zero_tokens() {
        let pricing = ModelPricing::new(2.5, 10.0, "USD");
        let cost = pricing.calculate_cost(0, 0);
        assert_eq!(cost, 0.0);
    }

    #[test]
    fn test_session_stats_accumulation() {
        let mut stats = SessionTokenStats::new("USD");

        let usage1 = TokenUsage::new(100, 50);
        stats.add_usage(&usage1, 0.001);

        let usage2 = TokenUsage::new(200, 100);
        stats.add_usage(&usage2, 0.002);

        assert_eq!(stats.request_count, 2);
        assert_eq!(stats.total_input_tokens, 300);
        assert_eq!(stats.total_output_tokens, 150);
        assert_eq!(stats.total_tokens(), 450);
        assert!((stats.total_cost - 0.003).abs() < 0.0001);
    }

    #[test]
    fn test_session_stats_avg_tokens() {
        let mut stats = SessionTokenStats::new("USD");

        stats.add_usage(&TokenUsage::new(100, 50), 0.001);
        stats.add_usage(&TokenUsage::new(200, 100), 0.002);

        assert!((stats.avg_tokens_per_request() - 225.0).abs() < 0.01);
    }

    #[test]
    fn test_estimate_tokens() {
        // "hello world" should be a small number of tokens
        let tokens = estimate_tokens("hello world");
        assert!(tokens > 0);
        assert!(tokens < 10);

        // Empty string should be 0
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn test_calculate_cost_with_pricing() {
        let usage = TokenUsage::new(1000, 500);
        let pricing = ModelPricing::new(2.5, 10.0, "USD");

        let cost = calculate_cost(&usage, Some(&pricing));
        assert!((cost - 0.0075).abs() < 0.0001);
    }

    #[test]
    fn test_calculate_cost_without_pricing() {
        let usage = TokenUsage::new(1000, 500);
        let cost = calculate_cost(&usage, None);
        assert_eq!(cost, 0.0);
    }

    #[test]
    fn test_format_token_usage() {
        let usage = TokenUsage::new(1234, 567);
        let formatted = format_token_usage(&usage);
        assert!(formatted.contains("Input: 1234"));
        assert!(formatted.contains("Output: 567"));
        assert!(formatted.contains("Total: 1801"));
    }

    #[test]
    fn test_format_cost() {
        assert_eq!(format_cost(0.0123, "USD"), "$0.0123");
        assert_eq!(format_cost(0.0123, "CNY"), "¥0.0123");
    }

    #[test]
    fn test_session_stats_format_summary() {
        let mut stats = SessionTokenStats::new("USD");
        stats.add_usage(&TokenUsage::new(100, 50), 0.001);

        let summary = stats.format_summary();
        assert!(summary.contains("Requests: 1"));
        assert!(summary.contains("Total Tokens: 150"));
        assert!(summary.contains("Total Cost: $0.0010"));
    }
}
