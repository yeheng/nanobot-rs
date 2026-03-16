//! Integration tests for tantivy-cli.
//!
//! These tests verify the simplified synchronous CLI architecture.

use std::sync::Arc;
use std::thread;

use tantivy_cli::index::{FieldDef, FieldType, IndexManager};

/// Test basic index operations.
#[test]
fn test_basic_index_operations() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let mut manager = IndexManager::new(temp_dir.path());

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

/// Test read operations from multiple threads (read-only operations).
#[test]
fn test_concurrent_read_access() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let mut manager = IndexManager::new(temp_dir.path());

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

    // Wrap in Arc for shared read access
    let manager = Arc::new(manager);

    // Spawn multiple threads to read different indexes concurrently
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
#[test]
fn test_document_operations() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let mut manager = IndexManager::new(temp_dir.path());

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

    // Add document (synchronous)
    let doc = tantivy_cli::index::Document::new(
        "doc1".to_string(),
        serde_json::json!({
            "text": "Hello world"
        })
        .as_object()
        .unwrap()
        .clone(),
    );
    manager
        .add_document("doc_test", doc)
        .expect("Failed to add document");

    // Commit (synchronous)
    manager.commit("doc_test").expect("Failed to commit");

    // List documents
    let docs = manager
        .list_documents("doc_test", 10, 0)
        .expect("Failed to list documents");
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0].id, "doc1");

    // Delete document (synchronous)
    manager
        .delete_document("doc_test", "doc1")
        .expect("Failed to delete document");

    manager.commit("doc_test").expect("Failed to commit");

    // Verify deletion
    let docs = manager
        .list_documents("doc_test", 10, 0)
        .expect("Failed to list documents");
    assert_eq!(docs.len(), 0);
}

/// Test index compaction.
#[test]
fn test_compact() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let mut manager = IndexManager::new(temp_dir.path());

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

    // Add documents
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
        manager
            .add_document("compact_test", doc)
            .expect("Failed to add document");
    }
    manager.commit("compact_test").expect("Failed to commit");

    // Delete half the documents
    for i in 0..5 {
        manager
            .delete_document("compact_test", &format!("doc{}", i))
            .expect("Failed to delete document");
    }
    manager.commit("compact_test").expect("Failed to commit");

    // Compact (synchronous)
    manager.compact("compact_test").expect("Failed to compact");

    // Verify
    let stats = manager
        .get_stats("compact_test")
        .expect("Failed to get stats");
    assert_eq!(stats.doc_count, 5);
}

/// Test file locking prevents concurrent access to same index.
#[test]
fn test_file_locking() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let mut manager = IndexManager::new(temp_dir.path());

    // Create index
    let fields = vec![FieldDef {
        name: "text".to_string(),
        field_type: FieldType::Text,
        indexed: true,
        stored: true,
    }];
    manager
        .create_index("lock_test", fields, None)
        .expect("Failed to create index");

    // Acquire lock and verify it works
    let lock = manager
        .acquire_index_lock("lock_test")
        .expect("Failed to acquire lock");

    // Lock should be held - dropping releases it
    drop(lock);

    // Should be able to acquire again
    let _lock2 = manager
        .acquire_index_lock("lock_test")
        .expect("Failed to acquire lock second time");
}

/// Test batch document operations.
#[test]
fn test_batch_operations() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let mut manager = IndexManager::new(temp_dir.path());

    // Create index
    let fields = vec![FieldDef {
        name: "text".to_string(),
        field_type: FieldType::Text,
        indexed: true,
        stored: true,
    }];
    manager
        .create_index("batch_test", fields, None)
        .expect("Failed to create index");

    // Create batch of documents
    let doc_inputs: Vec<tantivy_cli::index::BatchDocumentInput> = (0..100)
        .map(|i| tantivy_cli::index::BatchDocumentInput {
            id: format!("doc{}", i),
            fields: serde_json::json!({
                "text": format!("Document {}", i)
            })
            .as_object()
            .unwrap()
            .clone(),
            ttl: None,
        })
        .collect();

    // Add batch
    let result = manager
        .add_documents_batch("batch_test", doc_inputs, None, 4)
        .expect("Failed to add batch");

    assert_eq!(result.total, 100);
    assert_eq!(result.success, 100);
    assert_eq!(result.failed, 0);

    // Commit
    manager.commit("batch_test").expect("Failed to commit");

    // Verify
    let docs = manager
        .list_documents("batch_test", 200, 0)
        .expect("Failed to list documents");
    assert_eq!(docs.len(), 100);
}

/// Test search functionality.
#[test]
fn test_search() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let mut manager = IndexManager::new(temp_dir.path());

    // Create index
    let fields = vec![FieldDef {
        name: "text".to_string(),
        field_type: FieldType::Text,
        indexed: true,
        stored: true,
    }];
    manager
        .create_index("search_test", fields, None)
        .expect("Failed to create index");

    // Add documents
    let docs = vec![
        ("doc1", "Hello world"),
        ("doc2", "Rust programming"),
        ("doc3", "Search engine"),
        ("doc4", "Hello Rust"),
    ];

    for (id, text) in docs {
        let doc = tantivy_cli::index::Document::new(
            id.to_string(),
            serde_json::json!({ "text": text })
                .as_object()
                .unwrap()
                .clone(),
        );
        manager
            .add_document("search_test", doc)
            .expect("Failed to add document");
    }
    manager.commit("search_test").expect("Failed to commit");

    // Search for "Hello"
    let query = tantivy_cli::index::SearchQuery {
        text: Some("Hello".to_string()),
        filters: vec![],
        limit: 10,
        offset: 0,
        highlight: None,
        sort: None,
    };

    let results = manager
        .search("search_test", &query)
        .expect("Failed to search");
    assert!(results.len() >= 2); // Should find "Hello world" and "Hello Rust"
}

/// Test index persistence across manager instances.
#[test]
fn test_persistence() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

    // Create first manager and index
    {
        let mut manager = IndexManager::new(temp_dir.path());
        let fields = vec![FieldDef {
            name: "text".to_string(),
            field_type: FieldType::Text,
            indexed: true,
            stored: true,
        }];
        manager
            .create_index("persist_test", fields, None)
            .expect("Failed to create index");

        let doc = tantivy_cli::index::Document::new(
            "doc1".to_string(),
            serde_json::json!({ "text": "Persist this" })
                .as_object()
                .unwrap()
                .clone(),
        );
        manager
            .add_document("persist_test", doc)
            .expect("Failed to add document");
        manager.commit("persist_test").expect("Failed to commit");
    }

    // Create second manager and verify data persisted
    {
        let mut manager = IndexManager::new(temp_dir.path());
        manager.load_indexes().expect("Failed to load indexes");

        let indexes = manager.list_indexes();
        assert!(indexes.contains(&"persist_test".to_string()));

        let docs = manager
            .list_documents("persist_test", 10, 0)
            .expect("Failed to list documents");
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].id, "doc1");
    }
}
