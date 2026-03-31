//! Multi-dimensional history retrieval system.

use chrono::{DateTime, Utc};
use gasket_types::SessionEvent;

/// 历史检索器
pub struct HistoryRetriever {
    // Will be connected to EventStore later
}

/// 检索查询条件
#[derive(Debug, Clone, Default)]
pub struct HistoryQuery {
    /// 会话标识
    pub session_key: String,

    /// 分支过滤 (None = 当前分支)
    pub branch: Option<String>,

    /// 时间范围
    pub time_range: Option<TimeRange>,

    pub event_types: Vec<String>,

    /// 语义搜索
    pub semantic_query: Option<SemanticQuery>,

    /// 工具使用过滤
    pub tools_filter: Vec<String>,

    /// 分页
    pub offset: usize,
    pub limit: usize,

    /// 排序
    pub order: QueryOrder,
}

impl HistoryQuery {
    /// 创建查询构造器
    pub fn builder(session_key: impl Into<String>) -> HistoryQueryBuilder {
        HistoryQueryBuilder::new(session_key)
    }
}

/// 查询构造器 (流式 API)
pub struct HistoryQueryBuilder {
    query: HistoryQuery,
}

impl HistoryQueryBuilder {
    pub fn new(session_key: impl Into<String>) -> Self {
        Self {
            query: HistoryQuery {
                session_key: session_key.into(),
                limit: 50,
                ..Default::default()
            },
        }
    }

    pub fn branch(mut self, branch: impl Into<String>) -> Self {
        self.query.branch = Some(branch.into());
        self
    }

    pub fn time_range(mut self, start: DateTime<Utc>, end: DateTime<Utc>) -> Self {
        self.query.time_range = Some(TimeRange { start, end });
        self
    }

    pub fn event_types(mut self, types: Vec<String>) -> Self {
        self.query.event_types = types;
        self
    }

    pub fn semantic_text(mut self, text: impl Into<String>) -> Self {
        self.query.semantic_query = Some(SemanticQuery::Text(text.into()));
        self
    }

    pub fn semantic_embedding(mut self, embedding: Vec<f32>) -> Self {
        self.query.semantic_query = Some(SemanticQuery::Embedding(embedding));
        self
    }

    pub fn tools(mut self, tools: Vec<String>) -> Self {
        self.query.tools_filter = tools;
        self
    }

    pub fn limit(mut self, limit: usize) -> Self {
        self.query.limit = limit;
        self
    }

    pub fn offset(mut self, offset: usize) -> Self {
        self.query.offset = offset;
        self
    }

    pub fn order(mut self, order: QueryOrder) -> Self {
        self.query.order = order;
        self
    }

    pub fn build(self) -> HistoryQuery {
        self.query
    }
}

#[derive(Debug, Clone)]
pub enum SemanticQuery {
    Text(String),
    Embedding(Vec<f32>),
}

#[derive(Debug, Clone, Default)]
pub enum QueryOrder {
    Chronological,
    #[default]
    ReverseChronological,
    Similarity,
}

#[derive(Debug, Clone)]
pub struct TimeRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// 检索结果
#[derive(Debug)]
pub struct HistoryResult {
    pub events: Vec<SessionEvent>,
    pub meta: ResultMeta,
}

#[derive(Debug, Default)]
pub struct ResultMeta {
    pub total_count: usize,
    pub has_more: bool,
    pub query_time_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_builder() {
        let query = HistoryQuery::builder("test:session")
            .branch("explore")
            .limit(10)
            .offset(5)
            .order(QueryOrder::ReverseChronological)
            .build();

        assert_eq!(query.session_key, "test:session");
        assert_eq!(query.branch, Some("explore".into()));
        assert_eq!(query.limit, 10);
        assert_eq!(query.offset, 5);
    }

    #[test]
    fn test_query_builder_with_event_types() {
        let query = HistoryQuery::builder("test:session")
            .event_types(vec![
                "user_message".to_string(),
                "assistant_message".to_string(),
            ])
            .build();

        assert_eq!(query.event_types.len(), 2);
    }
}
