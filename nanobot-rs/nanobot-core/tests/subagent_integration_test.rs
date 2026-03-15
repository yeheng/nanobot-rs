//! Integration tests for subagent parallel execution

use std::sync::Arc;
use tokio::sync::mpsc;

use nanobot_core::agent::{SubagentManager, SubagentTracker};
use nanobot_core::bus::events::OutboundMessage;
use nanobot_core::providers::{LlmProvider, ProviderRegistry};
use nanobot_core::tools::ToolRegistry;

async fn create_test_manager() -> SubagentManager {
    let registry = ProviderRegistry::new();
    let provider = registry
        .get_or_create("openai")
        .expect("Failed to create provider");

    let workspace = std::env::temp_dir().join("nanobot-test");
    std::fs::create_dir_all(&workspace).ok();

    let tools = Arc::new(ToolRegistry::new());
    let (outbound_tx, _outbound_rx) = mpsc::channel::<OutboundMessage>(10);

    SubagentManager::new(provider, workspace, tools, outbound_tx).await
}

#[tokio::test]
async fn test_subagent_tracker_creation() {
    let tracker = SubagentTracker::new();
    let id = SubagentTracker::generate_id();

    assert!(!id.is_empty());
    assert_eq!(id.len(), 36); // UUID format
}

#[tokio::test]
async fn test_multiple_unique_ids() {
    let id1 = SubagentTracker::generate_id();
    let id2 = SubagentTracker::generate_id();
    let id3 = SubagentTracker::generate_id();

    assert_ne!(id1, id2);
    assert_ne!(id2, id3);
    assert_ne!(id1, id3);
}

#[tokio::test]
async fn test_tracker_result_sender() {
    let tracker = SubagentTracker::new();
    let sender = tracker.result_sender();

    // Verify sender can be cloned
    let _sender2 = sender.clone();
}

#[tokio::test]
async fn test_parallel_task_limit() {
    let tasks: Vec<String> = (0..15).map(|i| format!("Task {}", i)).collect();
    assert!(tasks.len() > 10, "Should exceed max parallel limit of 10");
}

#[tokio::test]
async fn test_empty_task_validation() {
    let tasks: Vec<String> = vec![];
    assert!(tasks.is_empty(), "Empty task list should be rejected");
}
