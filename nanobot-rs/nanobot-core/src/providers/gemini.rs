//! Google Gemini LLM provider

use crate::providers::{ChatRequest, ChatResponse, LlmProvider};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use tracing::debug;

/// Gemini provider using Google's Generative AI API
pub struct GeminiProvider {
    /// HTTP client
    client: Client,

    /// API key
    api_key: String,

    /// API base URL
    api_base: String,

    /// Default model
    default_model: String,
}

impl GeminiProvider {
    /// Create a new Gemini provider
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            api_base: "https://generativelanguage.googleapis.com/v1beta".to_string(),
            default_model: "gemini-pro".to_string(),
        }
    }

    /// Create with custom API base URL
    pub fn with_api_base(api_key: String, api_base: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            api_base,
            default_model: "gemini-pro".to_string(),
        }
    }

    /// Set default model
    pub fn with_model(mut self, model: String) -> Self {
        self.default_model = model;
        self
    }

    /// Convert ChatRequest to Gemini format
    fn build_gemini_request(&self, request: ChatRequest) -> Value {
        // Convert messages to Gemini format
        let mut contents = Vec::new();
        let mut system_instruction = None;

        for msg in request.messages {
            match msg.role.as_str() {
                "system" => {
                    system_instruction = Some(json!({
                        "parts": [{"text": msg.content.unwrap_or_default()}]
                    }));
                }
                "user" => {
                    contents.push(json!({
                        "role": "user",
                        "parts": [{"text": msg.content.unwrap_or_default()}]
                    }));
                }
                "assistant" => {
                    let mut parts = Vec::new();
                    if let Some(text) = &msg.content {
                        if !text.is_empty() {
                            parts.push(json!({"text": text}));
                        }
                    }
                    // Include function calls from assistant messages
                    if let Some(tool_calls) = &msg.tool_calls {
                        for tc in tool_calls {
                            parts.push(json!({
                                "functionCall": {
                                    "name": tc.function.name,
                                    "args": tc.function.arguments
                                }
                            }));
                        }
                    }
                    if !parts.is_empty() {
                        contents.push(json!({
                            "role": "model",
                            "parts": parts
                        }));
                    }
                }
                "tool" => {
                    // Gemini function call response format
                    let tool_name = msg.name.as_deref().unwrap_or("unknown");
                    let result_text = msg.content.unwrap_or_default();
                    contents.push(json!({
                        "role": "function",
                        "parts": [{
                            "functionResponse": {
                                "name": tool_name,
                                "response": {
                                    "content": result_text
                                }
                            }
                        }]
                    }));
                }
                _ => {}
            }
        }

        let mut body = json!({
            "contents": contents,
        });

        if let Some(system) = system_instruction {
            body["system_instruction"] = system;
        }

        // Add generation config
        let mut generation_config = json!({});

        if let Some(temp) = request.temperature {
            generation_config["temperature"] = json!(temp);
        }

        if let Some(tokens) = request.max_tokens {
            generation_config["maxOutputTokens"] = json!(tokens);
        }

        if generation_config
            .as_object()
            .map_or(false, |obj| !obj.is_empty())
        {
            body["generationConfig"] = generation_config;
        }

        // Add tool declarations if provided
        if let Some(tools) = &request.tools {
            if !tools.is_empty() {
                let function_declarations: Vec<Value> = tools
                    .iter()
                    .map(|t| {
                        json!({
                            "name": t.function.name,
                            "description": t.function.description,
                            "parameters": t.function.parameters
                        })
                    })
                    .collect();
                body["tools"] = json!([{
                    "function_declarations": function_declarations
                }]);
            }
        }

        debug!(
            "Gemini request: {}",
            serde_json::to_string_pretty(&body)
                .unwrap_or_else(|_| "<serialization error>".to_string())
        );
        body
    }

    /// Parse Gemini response
    fn parse_gemini_response(&self, response: Value) -> Result<ChatResponse> {
        debug!(
            "Gemini response: {}",
            serde_json::to_string_pretty(&response)
                .unwrap_or_else(|_| "<serialization error>".to_string())
        );

        // Check for errors
        if let Some(error) = response.get("error") {
            return Err(anyhow!("Gemini API error: {}", error));
        }

        // Extract candidates
        let candidates = response["candidates"]
            .as_array()
            .ok_or_else(|| anyhow!("No candidates in response"))?;

        if candidates.is_empty() {
            return Err(anyhow!("Empty candidates in response"));
        }

        let first_candidate = &candidates[0];
        let parts = first_candidate["content"]["parts"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        // Parse text content and function calls from parts
        let mut text_parts = Vec::new();
        let mut tool_calls = Vec::new();

        for (i, part) in parts.iter().enumerate() {
            if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                text_parts.push(text.to_string());
            }
            if let Some(fc) = part.get("functionCall") {
                let name = fc["name"].as_str().unwrap_or("").to_string();
                let args = fc.get("args").cloned().unwrap_or(json!({}));
                tool_calls.push(crate::providers::ToolCall::new(
                    format!("call_{}", i),
                    name,
                    args,
                ));
            }
        }

        let content = if text_parts.is_empty() {
            None
        } else {
            Some(text_parts.join(""))
        };

        Ok(ChatResponse {
            content,
            tool_calls,
            reasoning_content: None,
        })
    }
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    fn name(&self) -> &str {
        "gemini"
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse> {
        let model = if request.model.is_empty() {
            &self.default_model
        } else {
            &request.model
        };

        let url = format!("{}/models/{}:generateContent", self.api_base, model);

        let body = self.build_gemini_request(request);

        debug!("[gemini] POST {}", url);

        let response = self
            .client
            .post(&url)
            .header("x-goog-api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        debug!("[gemini] response status: {}", status);

        let response_text = response.text().await?;
        debug!("[gemini] response body:\n{}", response_text);

        if !status.is_success() {
            anyhow::bail!("Gemini API error: {} - {}", status, response_text);
        }

        let response_value: Value = serde_json::from_str(&response_text).map_err(|e| {
            anyhow!(
                "Gemini API response parse error: {} | body: {}",
                e,
                response_text
            )
        })?;

        self.parse_gemini_response(response_value)
    }
}

#[cfg(test)]
mod tests {
    use crate::providers::ChatMessage;

    use super::*;

    #[test]
    fn test_provider_creation() {
        let provider = GeminiProvider::new("test-key".to_string());
        assert_eq!(provider.name(), "gemini");
        assert_eq!(provider.default_model(), "gemini-pro");
    }

    #[test]
    fn test_custom_model() {
        let provider =
            GeminiProvider::new("test-key".to_string()).with_model("gemini-ultra".to_string());
        assert_eq!(provider.default_model(), "gemini-ultra");
    }

    #[test]
    fn test_build_gemini_request() {
        let provider = GeminiProvider::new("test-key".to_string());

        let request = ChatRequest {
            model: "gemini-pro".to_string(),
            messages: vec![
                ChatMessage::system("You are helpful"),
                ChatMessage::user("Hello"),
                ChatMessage::assistant("Hi there!"),
            ],
            tools: None,
            temperature: Some(0.7),
            max_tokens: Some(100),
        };

        let body = provider.build_gemini_request(request);

        // Check structure
        assert!(body.get("contents").is_some());
        assert!(body.get("system_instruction").is_some());
        assert!(body.get("generationConfig").is_some());

        // Check messages
        let contents = body["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 2); // user and assistant

        // Check user message role
        assert_eq!(contents[0]["role"], "user");

        // Check assistant message role (model in Gemini)
        assert_eq!(contents[1]["role"], "model");
    }
}
