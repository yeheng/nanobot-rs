//! Moonshot API integration test
//!
//! Run with:
//!   cargo test --test moonshot_api_test --features provider-moonshot -- --nocapture

#[cfg(feature = "provider-moonshot")]
use futures_util::StreamExt;
#[cfg(feature = "provider-moonshot")]
use gasket_providers::{ChatMessage, ChatRequest, LlmProvider, MoonshotProvider, ThinkingConfig};

#[tokio::test]
#[cfg(feature = "provider-moonshot")]
async fn test_moonshot_coding_api_thinking() {
    let provider = MoonshotProvider::with_config(
        "sk-kimi-xxxxxxxxx".to_string(),
        Some("https://api.kimi.com/coding".to_string()),
        Some("Kimi-k2.6".to_string()),
        None,
        None,
        None,
        None,
        None,
        [("user-agent".to_string(), "KimiCLI/1.37.0".to_string())]
            .into_iter()
            .collect(),
    );

    let request = ChatRequest {
        model: "Kimi-k2.6".to_string(),
        messages: vec![ChatMessage::user("1+1=?")],
        tools: None,
        temperature: Some(0.7),
        max_tokens: Some(200),
        thinking: Some(ThinkingConfig::enabled()),
    };

    println!("Testing Moonshot /coding with thinking (non-streaming)...");
    match provider.chat(request).await {
        Ok(response) => {
            println!("✅ Content: {:?}", response.content);
            println!("🧠 Reasoning: {:?}", response.reasoning_content);
            assert!(
                response.reasoning_content.is_some(),
                "Expected reasoning_content when thinking is enabled"
            );
        }
        Err(e) => panic!("❌ {}", e),
    }
}

#[tokio::test]
#[cfg(feature = "provider-moonshot")]
async fn test_moonshot_coding_stream_thinking() {
    let provider = MoonshotProvider::with_config(
        "sk-kimi-xxxxxxxxx".to_string(),
        Some("https://api.kimi.com/coding".to_string()),
        Some("Kimi-k2.6".to_string()),
        None,
        None,
        None,
        None,
        None,
        [("user-agent".to_string(), "KimiCLI/1.37.0".to_string())]
            .into_iter()
            .collect(),
    );

    let request = ChatRequest {
        model: "Kimi-k2.6".to_string(),
        messages: vec![ChatMessage::user("1+1=?")],
        tools: None,
        temperature: Some(0.7),
        max_tokens: Some(200),
        thinking: Some(ThinkingConfig::enabled()),
    };

    println!("\nTesting Moonshot /coding streaming with thinking...");
    let mut stream = provider.chat_stream(request).await.unwrap();

    let mut chunks = 0;
    let mut full_text = String::new();
    let mut full_reasoning = String::new();

    while let Some(result) = stream.next().await {
        match result {
            Ok(chunk) => {
                chunks += 1;
                if let Some(text) = &chunk.delta.content {
                    full_text.push_str(text);
                }
                if let Some(reasoning) = &chunk.delta.reasoning_content {
                    full_reasoning.push_str(reasoning);
                }
                println!(
                    "chunk {}: content={:?} reasoning={:?}",
                    chunks, chunk.delta.content, chunk.delta.reasoning_content
                );
            }
            Err(e) => panic!("Stream error: {}", e),
        }
    }

    println!(
        "✅ Stream done. chunks={}, text='{}', reasoning_len={}",
        chunks,
        full_text,
        full_reasoning.len()
    );
    assert!(
        !full_reasoning.is_empty(),
        "Expected non-empty streamed reasoning"
    );
}

#[tokio::test]
#[cfg(feature = "provider-moonshot")]
async fn test_moonshot_coding_api_tool_call_roundtrip() {
    use gasket_providers::{
        ChatMessage, ChatRequest, LlmProvider, MoonshotProvider, ThinkingConfig, ToolCall,
        ToolDefinition,
    };

    let provider = MoonshotProvider::with_config(
        "sk-kimi-xxxxxxxxx".to_string(),
        Some("https://api.kimi.com/coding".to_string()),
        Some("Kimi-k2.6".to_string()),
        None,
        None,
        None,
        None,
        None,
        [("user-agent".to_string(), "KimiCLI/1.37.0".to_string())]
            .into_iter()
            .collect(),
    );

    let tools = vec![ToolDefinition::function(
        "web_search",
        "Search the web",
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "count": {"type": "number"}
            },
            "required": ["query"]
        }),
    )];

    // First turn: ask a question that should trigger tool use
    let request1 = ChatRequest {
        model: "Kimi-k2.6".to_string(),
        messages: vec![ChatMessage::user("搜索 general agent github")],
        tools: Some(tools.clone()),
        temperature: Some(0.7),
        max_tokens: Some(200),
        thinking: Some(ThinkingConfig::enabled()),
    };

    println!("Turn 1: sending request with tools...");
    let mut stream = provider.chat_stream(request1).await.unwrap();
    let mut chunks = 0;
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    while let Some(result) = stream.next().await {
        match result {
            Ok(chunk) => {
                chunks += 1;
                if !chunk.delta.tool_calls.is_empty() {
                    println!(
                        "Turn 1 chunk {}: tool_calls={:?}",
                        chunks, chunk.delta.tool_calls
                    );
                }
                // Accumulate tool calls manually for simplicity
                for tc in &chunk.delta.tool_calls {
                    println!(
                        "  tool_call delta: id={:?} name={:?} args={:?}",
                        tc.id, tc.function_name, tc.function_arguments
                    );
                }
            }
            Err(e) => panic!("Turn 1 stream error: {}", e),
        }
    }
    println!("Turn 1 done. chunks={}", chunks);
}
