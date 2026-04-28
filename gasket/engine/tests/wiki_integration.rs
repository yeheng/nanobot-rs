//! Wiki integration tests — full cycle: ingest → query → lint.

use std::sync::Arc;

use gasket_engine::wiki::{
    slugify, PageFilter, PageIndex, PageStore, PageType, WikiLinter, WikiPage,
};
use gasket_storage::wiki::TantivyPageIndex;
use tempfile::TempDir;

async fn setup_store() -> (PageStore, TempDir) {
    // Ensure global broker is initialized (idempotent across parallel tests).
    gasket_engine::broker::init_broker(gasket_engine::broker::MemoryBroker::default());

    let pool = sqlx::SqlitePool::connect_lazy("sqlite::memory:").unwrap();
    gasket_engine::create_wiki_tables(&pool).await.unwrap();

    let dir = TempDir::new().unwrap();
    let store = PageStore::new(pool, dir.path().to_path_buf());
    store.init_dirs().await.unwrap();
    (store, dir)
}

fn make_page(path: &str, title: &str, content: &str, tags: Vec<&str>) -> WikiPage {
    let mut page = WikiPage::new(
        path.to_string(),
        title.to_string(),
        PageType::Topic,
        content.to_string(),
    );
    page.tags = tags.into_iter().map(|s| s.to_string()).collect();
    page
}

#[tokio::test]
async fn test_full_ingest_query_lint_cycle() {
    let (store, dir) = setup_store().await;

    // Ingest pages
    let rust_page = make_page(
        "topics/rust",
        "Rust Programming",
        "Rust is a systems programming language focused on safety, speed, and concurrency. \
         See [[topics/async]] for async programming details.",
        vec!["rust", "systems"],
    );
    let async_page = make_page(
        "topics/async",
        "Async Programming",
        "Async/await in Rust uses the tokio runtime for efficient I/O.",
        vec!["rust", "async"],
    );
    let short_page = make_page("topics/stub", "Stub Page", "Short.", vec![]);

    store.write(&rust_page).await.unwrap();
    store.write(&async_page).await.unwrap();
    store.write(&short_page).await.unwrap();

    // Query via Tantivy
    let tantivy_dir = dir.path().join(".tantivy");
    let tantivy = TantivyPageIndex::open(tantivy_dir).unwrap();
    let index = PageIndex::new(Arc::new(tantivy));

    // Rebuild index from store
    let rebuilt = index.rebuild(&store).await.unwrap();
    assert_eq!(rebuilt, 3);

    // Search for rust
    let hits = index.search_raw("rust programming", 10).await.unwrap();
    assert!(!hits.is_empty());
    assert_eq!(hits[0].path, "topics/rust");

    // Search for async
    let hits = index.search_raw("async tokio", 10).await.unwrap();
    assert!(!hits.is_empty());
    assert_eq!(hits[0].path, "topics/async");

    // Run lint
    let linter = WikiLinter::new(Arc::new(store));
    let report = linter.lint().await.unwrap();

    // Should find: stub page (short content), and at least one structural issue
    assert!(report.pages_checked >= 3);
    assert!(report.total_issues() > 0);

    // Should detect the stub
    let stubs: Vec<_> = report
        .structural
        .iter()
        .filter(|i| i.description.contains("chars of content"))
        .collect();
    assert!(!stubs.is_empty(), "Should detect stub pages");
}

#[tokio::test]
async fn test_write_read_roundtrip() {
    let (store, _dir) = setup_store().await;

    let mut page = WikiPage::new(
        "entities/gasket".to_string(),
        "Gasket Project".to_string(),
        PageType::Entity,
        "A Rust agent framework with wiki-first knowledge management.".to_string(),
    );
    page.tags = vec!["rust".to_string(), "agent".to_string()];
    page.category = Some("framework".to_string());

    store.write(&page).await.unwrap();

    let loaded = store.read("entities/gasket").await.unwrap();
    assert_eq!(loaded.title, "Gasket Project");
    assert_eq!(loaded.page_type, PageType::Entity);
    assert_eq!(loaded.tags, vec!["rust", "agent"]);
    assert_eq!(loaded.category, Some("framework".to_string()));
    assert!(loaded.content.contains("wiki-first"));
}

#[tokio::test]
async fn test_page_type_filtering() {
    let (store, _dir) = setup_store().await;

    let entity = WikiPage::new(
        "entities/a".into(),
        "Entity".into(),
        PageType::Entity,
        "Entity content.".into(),
    );
    let topic = WikiPage::new(
        "topics/b".into(),
        "Topic".into(),
        PageType::Topic,
        "Topic content.".into(),
    );
    let source = WikiPage::new(
        "sources/c".into(),
        "Source".into(),
        PageType::Source,
        "Source content.".into(),
    );

    store.write(&entity).await.unwrap();
    store.write(&topic).await.unwrap();
    store.write(&source).await.unwrap();

    let all = store.list(PageFilter::default()).await.unwrap();
    assert_eq!(all.len(), 3);

    let entities = store
        .list(PageFilter {
            page_type: Some(PageType::Entity),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(entities.len(), 1);
    assert_eq!(entities[0].page_type, PageType::Entity);
}

#[tokio::test]
async fn test_delete_page() {
    let (store, _dir) = setup_store().await;

    let page = make_page(
        "topics/delete-me",
        "Delete Me",
        "Content to delete.",
        vec![],
    );
    store.write(&page).await.unwrap();
    assert!(store.exists("topics/delete-me").await.unwrap());

    store.delete("topics/delete-me").await.unwrap();
    assert!(!store.exists("topics/delete-me").await.unwrap());
}

#[tokio::test]
async fn test_slugify_consistency() {
    assert_eq!(slugify("Hello World"), "hello-world");
    assert_eq!(slugify("Rust & LLM Agents"), "rust-llm-agents");
    assert_eq!(slugify("test/page"), "test-page");
    assert_eq!(slugify("UPPER CASE"), "upper-case");
}
