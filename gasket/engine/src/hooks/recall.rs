//! HistoryRecallHook — history recall for the AfterHistory hook point.
//!
//! This module provides two implementations gated by the `embedding` feature:
//!
//! - **Without `embedding`**: Uses keyword matching against the session event store.
//! - **With `embedding`**: Uses semantic vector search via `gasket_embedding::RecallSearcher`.
//!
//! Both implementations inject relevant historical context as additional messages.

// ── Keyword-based implementation (no embedding feature) ─────────────

#[cfg(not(feature = "embedding"))]
mod keyword_impl {
    use std::collections::HashMap;
    use std::sync::Arc;

    use async_trait::async_trait;
    use gasket_providers::ChatMessage;
    use tracing::debug;

    use super::super::{HookAction, HookPoint, MutableContext, PipelineHook};
    use crate::error::AgentError;
    use gasket_storage::EventStore;
    use gasket_types::SessionKey;

    /// Returns true if the character is a CJK unified ideograph.
    fn is_cjk(c: char) -> bool {
        ('\u{4e00}'..='\u{9fff}').contains(&c)
    }

    /// Stop words filtered out during keyword extraction.
    const STOP_WORDS: &[&str] = &[
        "the",
        "a",
        "an",
        "is",
        "are",
        "was",
        "were",
        "be",
        "been",
        "being",
        "have",
        "has",
        "had",
        "do",
        "does",
        "did",
        "will",
        "would",
        "could",
        "should",
        "may",
        "might",
        "must",
        "shall",
        "can",
        "need",
        "dare",
        "ought",
        "used",
        "to",
        "of",
        "in",
        "for",
        "on",
        "with",
        "at",
        "by",
        "from",
        "as",
        "into",
        "through",
        "during",
        "before",
        "after",
        "above",
        "below",
        "between",
        "under",
        "again",
        "further",
        "then",
        "once",
        "here",
        "there",
        "when",
        "where",
        "why",
        "how",
        "all",
        "each",
        "few",
        "more",
        "most",
        "other",
        "some",
        "such",
        "no",
        "nor",
        "not",
        "only",
        "own",
        "same",
        "so",
        "than",
        "too",
        "very",
        "just",
        "and",
        "but",
        "if",
        "or",
        "because",
        "until",
        "while",
        "这",
        "那",
        "是",
        "的",
        "了",
        "在",
        "有",
        "和",
        "与",
        "或",
        "就",
        "都",
        "而",
        "及",
        "等",
        "对",
        "能",
        "会",
        "要",
        "把",
        "被",
        "给",
        "让",
        "向",
        "从",
        "到",
        "为",
        "于",
        "以",
        "个",
        "什么",
        "怎么",
        "为什么",
        "哪里",
        "谁",
        "多少",
        "吗",
        "呢",
        "吧",
        "啊",
        "哦",
        "嗯",
        "我",
        "你",
        "他",
        "她",
        "它",
        "我们",
        "你们",
        "他们",
        "自己",
    ];

    /// Hook that recalls relevant historical messages from the current session
    /// using keyword matching.
    ///
    /// Runs at `AfterHistory` and injects matching past events as context.
    pub struct HistoryRecallHook {
        event_store: Arc<EventStore>,
        /// Maximum number of historical messages to inject.
        top_k: usize,
        /// Minimum keyword length (in bytes) to be considered.
        min_keyword_len: usize,
        /// Max events to fetch per keyword.
        per_keyword_limit: i64,
    }

    impl HistoryRecallHook {
        /// Create a new recall hook with the given event store.
        pub fn new(event_store: Arc<EventStore>) -> Self {
            Self {
                event_store,
                top_k: 3,
                min_keyword_len: 2,
                per_keyword_limit: 20,
            }
        }

        /// Set how many recalled messages to inject (default: 3).
        pub fn with_top_k(mut self, k: usize) -> Self {
            self.top_k = k;
            self
        }

        /// Extract keywords from user input, filtering out stop words and short tokens.
        fn extract_keywords(&self, text: &str) -> Vec<String> {
            let mut keywords = Vec::new();
            let chars: Vec<char> = text.chars().collect();
            let mut i = 0;

            while i < chars.len() {
                if is_cjk(chars[i]) {
                    let start = i;
                    while i < chars.len() && is_cjk(chars[i]) {
                        i += 1;
                    }
                    let seq: String = chars[start..i].iter().collect();
                    if seq.len() >= self.min_keyword_len && !STOP_WORDS.contains(&seq.as_str()) {
                        keywords.push(seq);
                    }
                    continue;
                }

                if chars[i].is_ascii_alphanumeric() {
                    let start = i;
                    while i < chars.len() && chars[i].is_ascii_alphanumeric() {
                        i += 1;
                    }
                    let word: String = chars[start..i].iter().collect::<String>().to_lowercase();
                    if word.len() >= self.min_keyword_len
                        && word.chars().any(|c| c.is_ascii_alphabetic())
                        && !STOP_WORDS.contains(&word.as_str())
                    {
                        keywords.push(word);
                    }
                    continue;
                }

                i += 1;
            }

            let mut seen = std::collections::HashSet::new();
            keywords
                .into_iter()
                .filter(|k| seen.insert(k.clone()))
                .collect()
        }

        /// Search history for events matching the given keywords.
        async fn recall(
            &self,
            session_key: &SessionKey,
            keywords: &[String],
        ) -> Result<Vec<String>, AgentError> {
            if keywords.is_empty() {
                return Ok(Vec::new());
            }

            let mut scores: HashMap<String, (String, usize)> = HashMap::new();

            for kw in keywords {
                let events = self
                    .event_store
                    .search_session_events(session_key, kw, self.per_keyword_limit)
                    .await
                    .map_err(|e| {
                        AgentError::SessionError(format!("History recall search failed: {}", e))
                    })?;

                for event in events {
                    let entry = scores.entry(event.id.to_string()).or_insert_with(|| {
                        let role = match event.event_type {
                            gasket_types::EventType::UserMessage => "user",
                            gasket_types::EventType::AssistantMessage => "assistant",
                            _ => "system",
                        };
                        (format!("[{}]: {}", role, event.content), 0)
                    });
                    entry.1 += 1;
                }
            }

            let mut scored: Vec<_> = scores.into_values().collect();
            scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

            Ok(scored
                .into_iter()
                .take(self.top_k)
                .map(|(text, _)| text)
                .collect())
        }
    }

    #[async_trait]
    impl PipelineHook for HistoryRecallHook {
        fn name(&self) -> &str {
            "history_recall"
        }

        fn point(&self) -> HookPoint {
            HookPoint::AfterHistory
        }

        async fn run(&self, ctx: &mut MutableContext<'_>) -> Result<HookAction, AgentError> {
            let user_input = match ctx.user_input {
                Some(text) => text,
                None => return Ok(HookAction::Continue),
            };

            let keywords = self.extract_keywords(user_input);
            if keywords.is_empty() {
                return Ok(HookAction::Continue);
            }

            let session_key = SessionKey::parse(ctx.session_key).unwrap_or_else(|| {
                SessionKey::new(gasket_types::ChannelType::Cli, ctx.session_key)
            });

            let recalled = match self.recall(&session_key, &keywords).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("[{}] Recall failed: {}", self.name(), e);
                    return Ok(HookAction::Continue);
                }
            };

            if recalled.is_empty() {
                debug!(
                    "[{}] No relevant history found for keywords: {:?}",
                    self.name(),
                    keywords
                );
                return Ok(HookAction::Continue);
            }

            let injection = format!(
                "[SYSTEM: 以下是从历史对话中召回的相关内容，供你参考]\n\n{}",
                recalled.join("\n\n---\n\n")
            );
            ctx.messages.push(ChatMessage::user(injection));

            debug!(
                "[{}] Injected {} recalled messages for session {}",
                self.name(),
                recalled.len(),
                session_key
            );

            Ok(HookAction::Continue)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn make_hook() -> HistoryRecallHook {
            let pool = sqlx::SqlitePool::connect_lazy(":memory:").unwrap();
            HistoryRecallHook::new(Arc::new(EventStore::new(pool)))
        }

        #[tokio::test]
        async fn test_extract_keywords_english() {
            let hook = make_hook();
            let kw = hook.extract_keywords("How do I build a Rust web server?");
            assert!(kw.contains(&"rust".to_string()));
            assert!(kw.contains(&"build".to_string()));
            assert!(kw.contains(&"web".to_string()));
            assert!(kw.contains(&"server".to_string()));
            assert!(!kw.contains(&"how".to_string()));
            assert!(!kw.contains(&"i".to_string()));
        }

        #[tokio::test]
        async fn test_extract_keywords_chinese() {
            let hook = make_hook();
            let kw = hook.extract_keywords("如何在Rust中构建Web服务器？");
            assert!(kw.contains(&"rust".to_string()));
            assert!(kw.contains(&"web".to_string()));
            assert!(!kw.contains(&"如何".to_string()));
            assert!(!kw.contains(&"在".to_string()));
        }

        #[tokio::test]
        async fn test_extract_keywords_deduplicates() {
            let hook = make_hook();
            let kw = hook.extract_keywords("rust rust web web server");
            assert_eq!(kw.len(), 3);
            assert_eq!(kw.iter().filter(|&k| k == "rust").count(), 1);
        }
    }
}

// ── Semantic embedding-based implementation (with embedding feature) ──

#[cfg(feature = "embedding")]
mod embedding_impl {
    use std::sync::Arc;

    use async_trait::async_trait;
    use gasket_embedding::{RecallConfig, RecallSearcher};
    use gasket_providers::ChatMessage;
    use tracing::{debug, warn};

    use super::super::{HookAction, HookPoint, MutableContext, PipelineHook};
    use crate::error::AgentError;
    use gasket_storage::EventStore;
    use gasket_types::SessionKey;

    /// Hook that recalls relevant historical messages using semantic embedding search.
    ///
    /// Runs at `AfterHistory` and injects semantically similar past events as context.
    pub struct HistoryRecallHook {
        searcher: Arc<RecallSearcher>,
        config: RecallConfig,
        event_store: Arc<EventStore>,
    }

    impl HistoryRecallHook {
        /// Create a new semantic recall hook.
        pub fn new(
            searcher: Arc<RecallSearcher>,
            config: RecallConfig,
            event_store: Arc<EventStore>,
        ) -> Self {
            Self {
                searcher,
                config,
                event_store,
            }
        }
    }

    #[async_trait]
    impl PipelineHook for HistoryRecallHook {
        fn name(&self) -> &str {
            "history_recall"
        }

        fn point(&self) -> HookPoint {
            HookPoint::AfterHistory
        }

        async fn run(&self, ctx: &mut MutableContext<'_>) -> Result<HookAction, AgentError> {
            let user_input = match ctx.user_input {
                Some(text) => text,
                None => return Ok(HookAction::Continue),
            };

            let results = match self.searcher.recall(user_input, &self.config).await {
                Ok(r) => r,
                Err(e) => {
                    warn!("[{}] Semantic recall failed: {}", self.name(), e);
                    return Ok(HookAction::Continue);
                }
            };

            if results.is_empty() {
                debug!("[{}] No semantically relevant history found", self.name());
                return Ok(HookAction::Continue);
            }

            // Load full event content for matched IDs.
            let ids: Vec<uuid::Uuid> = results
                .iter()
                .filter_map(|(id, _)| uuid::Uuid::parse_str(id).ok())
                .collect();

            let events = self
                .event_store
                .get_events_by_ids_global(&ids)
                .await
                .unwrap_or_default();

            if events.is_empty() {
                return Ok(HookAction::Continue);
            }

            let session_key = SessionKey::parse(ctx.session_key).unwrap_or_else(|| {
                SessionKey::new(gasket_types::ChannelType::Cli, ctx.session_key)
            });

            let lines: Vec<String> = events
                .iter()
                .map(|e| {
                    let role = match e.event_type {
                        gasket_types::EventType::UserMessage => "user",
                        gasket_types::EventType::AssistantMessage => "assistant",
                        _ => "system",
                    };
                    format!("[{}]: {}", role, e.content)
                })
                .collect();

            let injection = format!(
                "[SYSTEM: 以下是从历史对话中召回的相关内容，供你参考]\n\n{}",
                lines.join("\n\n---\n\n")
            );
            ctx.messages.push(ChatMessage::user(injection));

            debug!(
                "[{}] Injected {} semantically recalled messages for session {}",
                self.name(),
                events.len(),
                session_key
            );

            Ok(HookAction::Continue)
        }
    }
}

// Re-export the active implementation's HistoryRecallHook.
#[cfg(not(feature = "embedding"))]
pub use keyword_impl::HistoryRecallHook;

#[cfg(feature = "embedding")]
pub use embedding_impl::HistoryRecallHook;
