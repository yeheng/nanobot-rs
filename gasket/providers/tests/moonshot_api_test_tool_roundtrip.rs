#[cfg(feature = "provider-moonshot")]
#[tokio::test]
async fn test_moonshot_coding_api_tool_roundtrip() {
    use futures_util::StreamExt;
    use gasket_providers::{
        ChatMessage, ChatRequest, ChatStreamChunk, LlmProvider, MoonshotProvider, ThinkingConfig,
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

    // First turn
    let request1 = ChatRequest {
        model: "Kimi-k2.6".to_string(),
        messages: vec![ChatMessage::user("搜索 general agent github")],
        tools: Some(tools.clone()),
        temperature: Some(0.7),
        max_tokens: Some(200),
        thinking: Some(ThinkingConfig::enabled()),
    };

    println!("Turn 1: streaming...");
    let mut stream = provider.chat_stream(request1).await.unwrap();
    let mut pending: Vec<Option<(String, String, String)>> = Vec::new();
    let mut reasoning_content = String::new();
    let mut chunks = 0;
    while let Some(result) = stream.next().await {
        let chunk: ChatStreamChunk = result.unwrap();
        chunks += 1;
        if let Some(r) = &chunk.delta.reasoning_content {
            reasoning_content.push_str(r);
        }
        for tc in &chunk.delta.tool_calls {
            if tc.index >= pending.len() {
                pending.resize_with(tc.index + 1, || None);
            }
            let entry = pending[tc.index]
                .get_or_insert_with(|| (String::new(), String::new(), String::new()));
            if let Some(id) = &tc.id {
                entry.0 = id.clone();
            }
            if let Some(name) = &tc.function_name {
                entry.1 = name.clone();
            }
            if let Some(args) = &tc.function_arguments {
                entry.2.push_str(args);
            }
        }
    }
    let tool_calls: Vec<_> = pending
        .into_iter()
        .flatten()
        .map(|(id, name, args)| {
            gasket_providers::ToolCall::new(
                id,
                name,
                serde_json::from_str(&args).unwrap_or_else(|_| serde_json::json!({})),
            )
        })
        .collect();
    println!(
        "Turn 1 done. chunks={} reasoning_len={} tool_calls={:?}",
        chunks,
        reasoning_content.len(),
        tool_calls
    );
    assert!(!tool_calls.is_empty(), "Expected tool calls in turn 1");

    // Second turn: send tool result back WITH reasoning_content preserved
    let reasoning_opt = if reasoning_content.is_empty() {
        None
    } else {
        Some(reasoning_content)
    };
    let messages = vec![
        ChatMessage::user("搜索 general agent github"),
        ChatMessage::assistant_with_tools(
            None,
            tool_calls.clone(),
            reasoning_opt,
        ),
        ChatMessage::tool_result(
            &tool_calls[0].id,
            &tool_calls[0].function.name,
            "1. **General Agent**\n   A general agent framework.\n   URL: https://github.com/example/general-agent\n\n",
        ),
    ];

    let request2 = ChatRequest {
        model: "Kimi-k2.6".to_string(),
        messages,
        tools: Some(tools.clone()),
        temperature: Some(0.7),
        max_tokens: Some(200),
        thinking: Some(ThinkingConfig::enabled()),
    };

    println!("Turn 2: sending request with tool result...");
    let mut stream = provider.chat_stream(request2).await.unwrap();
    let mut chunks = 0;
    let mut text = String::new();
    while let Some(result) = stream.next().await {
        let chunk = result.unwrap();
        chunks += 1;
        if let Some(t) = &chunk.delta.content {
            text.push_str(t);
        }
    }
    println!("Turn 2 done. chunks={} text={}", chunks, text);
    println!(
        "Turn 2 completed successfully with {} chunks (no 400 error)",
        chunks
    );
}
