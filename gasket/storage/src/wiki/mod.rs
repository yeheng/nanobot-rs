pub mod log_store;
pub mod page_store;
pub mod relation_store;
pub mod source_store;
pub mod tables;

pub use log_store::WikiLogStore;
pub use page_store::WikiPageStore;
pub use relation_store::WikiRelationStore;
pub use source_store::WikiSourceStore;
pub use tables::create_wiki_tables;
