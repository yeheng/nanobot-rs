pub mod index;
pub mod log;
pub mod page;
pub mod store;

// Re-exports
pub use index::PageIndex;
pub use log::{LogEntry, WikiLog};
pub use page::{slugify, PageFilter, PageSummary, PageType, WikiPage};
pub use store::PageStore;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_page_new_entity() {
        let page = WikiPage::new(
            "entities/projects/gasket".to_string(),
            "Gasket Project".to_string(),
            PageType::Entity,
            "A Rust agent framework.".to_string(),
        );
        assert_eq!(page.path, "entities/projects/gasket");
        assert_eq!(page.page_type, PageType::Entity);
        assert_eq!(page.confidence, 1.0);
        assert!(page.tags.is_empty());
    }

    #[test]
    fn test_page_new_topic() {
        let page = WikiPage::new(
            "topics/rust-async".to_string(),
            "Rust Async".to_string(),
            PageType::Topic,
            "How async works in Rust.".to_string(),
        );
        assert_eq!(page.page_type, PageType::Topic);
    }

    #[test]
    fn test_page_markdown_roundtrip() {
        let mut page = WikiPage::new(
            "topics/test".to_string(),
            "Test Topic".to_string(),
            PageType::Topic,
            "Some content here.".to_string(),
        );
        page.tags = vec!["test".to_string()];
        let md = page.to_markdown();
        let parsed = WikiPage::from_markdown("topics/test".to_string(), &md).unwrap();
        assert_eq!(parsed.title, "Test Topic");
        assert_eq!(parsed.content, "Some content here.");
        assert_eq!(parsed.tags, vec!["test"]);
    }

    #[test]
    fn test_make_path() {
        assert_eq!(
            WikiPage::make_path(&["entities", "projects", "gasket"]),
            "entities/projects/gasket"
        );
        assert_eq!(WikiPage::make_path(&["topics", "rust-async"]), "topics/rust-async");
    }

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("Hello World"), "hello-world");
        assert_eq!(slugify("Rust & LLM"), "rust-llm");
        assert_eq!(slugify("  spaces  "), "spaces");
    }

    #[test]
    fn test_page_type_roundtrip() {
        assert_eq!(PageType::from_str("entity"), Some(PageType::Entity));
        assert_eq!(PageType::from_str("topic"), Some(PageType::Topic));
        assert_eq!(PageType::from_str("source"), Some(PageType::Source));
        assert_eq!(PageType::from_str("unknown"), None);
    }
}
