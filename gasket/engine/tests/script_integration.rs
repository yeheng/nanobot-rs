use gasket_engine::tools::script::discover_scripts_in_dir;
use gasket_engine::tools::{Tool, ToolContext};
use std::path::PathBuf;

fn test_scripts_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests")
        .join("scripts")
}

#[tokio::test]
async fn test_simple_echo_tool() {
    let tools = discover_scripts_in_dir(&test_scripts_dir()).unwrap();
    let echo_tool = tools
        .iter()
        .find(|t| t.name() == "test_echo")
        .expect("test_echo not found");
    let args = serde_json::json!({"message": "hello world"});
    let result = echo_tool.execute(args, &ToolContext::default()).await;
    assert!(result.is_ok(), "Simple echo failed: {:?}", result);
    let parsed: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
    assert_eq!(parsed["result"]["echo"], "hello world");
    assert_eq!(parsed["result"]["status"], "ok");
}

#[tokio::test]
async fn test_jsonrpc_ping_tool() {
    let tools = discover_scripts_in_dir(&test_scripts_dir()).unwrap();
    let ping_tool = tools
        .iter()
        .find(|t| t.name() == "test_ping")
        .expect("test_ping not found");
    let args = serde_json::json!({"name": "Alice"});
    let result = ping_tool.execute(args, &ToolContext::default()).await;
    assert!(result.is_ok(), "JsonRpc ping failed: {:?}", result);
    let parsed: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
    assert_eq!(parsed["result"]["greeting"], "Hello, Alice!");
    assert_eq!(parsed["result"]["llm_called"], false);
}

#[test]
fn test_discover_finds_both_tools() {
    let tools = discover_scripts_in_dir(&test_scripts_dir()).unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
    assert!(names.contains(&"test_echo"), "Missing test_echo");
    assert!(names.contains(&"test_ping"), "Missing test_ping");
}
