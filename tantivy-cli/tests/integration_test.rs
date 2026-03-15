//! Integration tests for tantivy-mcp lock refactoring.
//!
//! These tests verify that the new lock design using DashMap works correctly.

use std::sync::Arc;
use std::thread;

use tantivy_cli::index::{FieldDef, FieldType, IndexManager};
use tantivy_cli::maintenance::JobRegistry;

/// Test basic index operations.
#[test]
fn test_basic_index_operations() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let job_registry = Arc::new(JobRegistry::new());
    let manager = IndexManager::new(temp_dir.path(), job_registry);

    // Create index
    let fields = vec![
        FieldDef {
            name: "title".to_string(),
            field_type: FieldType::Text,
            indexed: true,
            stored: true,
        },
        FieldDef {
            name: "content".to_string(),
            field_type: FieldType::Text,
            indexed: true,
            stored: true,
        },
    ];

    let schema = manager
        .create_index("test_index", fields, None)
        .expect("Failed to create index");

    assert_eq!(schema.name, "test_index");
    assert_eq!(schema.fields.len(), 2);

    // List indexes
    let indexes = manager.list_indexes();
    assert!(indexes.contains(&"test_index".to_string()));

    // Get schema
    let retrieved_schema = manager
        .get_schema("test_index")
        .expect("Failed to get schema")
        .expect("Schema not found");
    assert_eq!(retrieved_schema.name, "test_index");

    // Get stats
    let stats = manager
        .get_stats("test_index")
        .expect("Failed to get stats");
    assert_eq!(stats.doc_count, 0);

    // Drop index
    manager
        .drop_index("test_index")
        .expect("Failed to drop index");

    let indexes = manager.list_indexes();
    assert!(!indexes.contains(&"test_index".to_string()));
}

/// Test concurrent access to different indexes.
#[test]
fn test_concurrent_index_access() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let manager = Arc::new(IndexManager::new(
        temp_dir.path(),
        Arc::new(JobRegistry::new()),
    ));

    // Create indexes first
    for i in 0..5 {
        let fields = vec![FieldDef {
            name: "field".to_string(),
            field_type: FieldType::Text,
            indexed: true,
            stored: true,
        }];
        manager
            .create_index(&format!("index_{}", i), fields, None)
            .expect("Failed to create index");
    }

    // Spawn multiple threads to access different indexes concurrently
    let handles: Vec<_> = (0..5)
        .map(|i| {
            let mgr = manager.clone();
            thread::spawn(move || {
                let index_name = format!("index_{}", i);
                // Read operation
                let schema = mgr.get_schema(&index_name).unwrap().unwrap();
                assert_eq!(schema.name, index_name);

                // Stats operation
                let stats = mgr.get_stats(&index_name).unwrap();
                assert_eq!(stats.name, index_name);
            })
        })
        .collect();

    // Wait for all threads
    for handle in handles {
        handle.join().expect("Thread panicked");
    }
}

/// Test document operations.
#[tokio::test(flavor = "multi_thread")]
async fn test_document_operations() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let job_registry = Arc::new(JobRegistry::new());
    let manager = IndexManager::new(temp_dir.path(), job_registry.clone());

    // Create index
    let fields = vec![FieldDef {
        name: "text".to_string(),
        field_type: FieldType::Text,
        indexed: true,
        stored: true,
    }];
    manager
        .create_index("doc_test", fields, None)
        .expect("Failed to create index");

    // Add document (returns JobId)
    let doc = tantivy_cli::index::Document::new(
        "doc1".to_string(),
        serde_json::json!({
            "text": "Hello world"
        })
        .as_object()
        .unwrap()
        .clone(),
    );
    let job_id = manager
        .add_document("doc_test", doc)
        .expect("Failed to add document");

    // Wait for job to complete
    wait_for_job(&job_registry, &job_id, std::time::Duration::from_secs(5));

    // Commit (returns JobId)
    let commit_job_id = manager.commit("doc_test").expect("Failed to commit");
    wait_for_job(
        &job_registry,
        &commit_job_id,
        std::time::Duration::from_secs(5),
    );

    // List documents
    let docs = manager
        .list_documents("doc_test", 10, 0)
        .expect("Failed to list documents");
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0].id, "doc1");

    // Delete document (returns JobId)
    let delete_job_id = manager
        .delete_document("doc_test", "doc1")
        .expect("Failed to delete document");
    wait_for_job(
        &job_registry,
        &delete_job_id,
        std::time::Duration::from_secs(5),
    );

    let commit_job_id = manager.commit("doc_test").expect("Failed to commit");
    wait_for_job(
        &job_registry,
        &commit_job_id,
        std::time::Duration::from_secs(5),
    );

    // Verify deletion
    let docs = manager
        .list_documents("doc_test", 10, 0)
        .expect("Failed to list documents");
    assert_eq!(docs.len(), 0);
}

/// Test index compaction.
#[tokio::test(flavor = "multi_thread")]
async fn test_compact() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let job_registry = Arc::new(JobRegistry::new());
    let manager = IndexManager::new(temp_dir.path(), job_registry.clone());

    // Create index
    let fields = vec![FieldDef {
        name: "text".to_string(),
        field_type: FieldType::Text,
        indexed: true,
        stored: true,
    }];
    manager
        .create_index("compact_test", fields, None)
        .expect("Failed to create index");

    // Add and delete some documents
    for i in 0..10 {
        let doc = tantivy_cli::index::Document::new(
            format!("doc{}", i),
            serde_json::json!({
                "text": format!("Document {}", i)
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        let job_id = manager
            .add_document("compact_test", doc)
            .expect("Failed to add document");
        wait_for_job(&job_registry, &job_id, std::time::Duration::from_secs(5));
    }
    let commit_job_id = manager.commit("compact_test").expect("Failed to commit");
    wait_for_job(
        &job_registry,
        &commit_job_id,
        std::time::Duration::from_secs(5),
    );

    // Delete half the documents
    for i in 0..5 {
        let job_id = manager
            .delete_document("compact_test", &format!("doc{}", i))
            .expect("Failed to delete document");
        wait_for_job(&job_registry, &job_id, std::time::Duration::from_secs(5));
    }
    let commit_job_id = manager.commit("compact_test").expect("Failed to commit");
    wait_for_job(
        &job_registry,
        &commit_job_id,
        std::time::Duration::from_secs(5),
    );

    // Compact (returns JobId)
    let compact_job_id = manager.compact("compact_test").expect("Failed to compact");
    wait_for_job(
        &job_registry,
        &compact_job_id,
        std::time::Duration::from_secs(10),
    );

    // Verify
    let stats = manager
        .get_stats("compact_test")
        .expect("Failed to get stats");
    assert_eq!(stats.doc_count, 5);
}

/// Helper function to wait for a job to complete.
fn wait_for_job(job_registry: &Arc<JobRegistry>, job_id: &str, timeout: std::time::Duration) {
    use tantivy_cli::maintenance::JobStatus;
    let start = std::time::Instant::now();
    loop {
        if let Some(job) = job_registry.get_job(job_id) {
            match job.status {
                JobStatus::Completed => return,
                JobStatus::Failed => panic!("Job {} failed: {:?}", job_id, job.error),
                _ => {}
            }
        }
        if start.elapsed() > timeout {
            panic!("Job {} timed out after {:?}", job_id, timeout);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
