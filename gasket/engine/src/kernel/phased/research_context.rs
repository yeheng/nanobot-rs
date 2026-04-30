use gasket_providers::{ChatMessage, MessageRole};

pub struct WikiHit {
    pub title: String,
    pub path: String,
    pub score: f32,
    pub summary: String,
}

pub struct HistoryHit {
    pub role: String,
    pub content: String,
    pub timestamp: String,
}

pub struct ResearchContext;

impl ResearchContext {
    /// Build a search query from the last 3 user messages, joined by space.
    pub fn build_search_query(messages: &[ChatMessage]) -> String {
        let user_msgs: Vec<&str> = messages
            .iter()
            .rev()
            .filter(|m| m.role == MessageRole::User)
            .filter_map(|m| m.content.as_deref())
            .take(3)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        user_msgs.join(" ")
    }

    /// Format wiki hits and history hits into a human-readable block for
    /// injection into the agent's context.
    pub fn format_auto_search_results(
        wiki_hits: &[WikiHit],
        history_hits: &[HistoryHit],
    ) -> String {
        let mut parts = vec!["[Research Context — 自动检索]\n".to_string()];
        if wiki_hits.is_empty() && history_hits.is_empty() {
            parts.push("未找到相关的 Wiki 页面或历史记录。\n".to_string());
        } else {
            if !wiki_hits.is_empty() {
                parts.push(format!("## Wiki 相关页面 ({}条)\n", wiki_hits.len()));
                for hit in wiki_hits {
                    parts.push(format!(
                        "- {} ({:.2}): {}\n",
                        hit.title, hit.score, hit.summary
                    ));
                }
                parts.push("\n".to_string());
            }
            if !history_hits.is_empty() {
                parts.push(format!("## 历史相关记录 ({}条)\n", history_hits.len()));
                for hit in history_hits {
                    let preview = truncate_str(&hit.content, 100);
                    parts.push(format!(
                        "- [{}] {}: {}\n",
                        hit.timestamp, hit.role, preview
                    ));
                }
                parts.push("\n".to_string());
            }
        }
        parts.push(
            "你可以用 wiki_read 查看完整页面，或 history_search 调整搜索方向。\n\
             需要更多信息也可以直接问我。信息充分后调用 phase_transition 进入下一阶段。"
                .to_string(),
        );
        parts.join("")
    }
}

fn truncate_str(s: &str, max_chars: usize) -> &str {
    if s.chars().count() <= max_chars {
        s
    } else {
        let end = s
            .char_indices()
            .nth(max_chars)
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        &s[..end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gasket_providers::ChatMessage;

    #[test]
    fn test_build_search_query_single_message() {
        let messages = vec![ChatMessage::user("How does tokio work?")];
        let query = ResearchContext::build_search_query(&messages);
        assert_eq!(query, "How does tokio work?");
    }

    #[test]
    fn test_build_search_query_concatenates_recent() {
        let messages = vec![
            ChatMessage::user("Tell me about Rust"),
            ChatMessage::assistant("Rust is..."),
            ChatMessage::user("How about async?"),
        ];
        let query = ResearchContext::build_search_query(&messages);
        assert!(query.contains("Tell me about Rust"));
        assert!(query.contains("How about async?"));
    }

    #[test]
    fn test_build_search_query_limits_to_last_3() {
        let mut messages = vec![];
        for i in 0..5 {
            messages.push(ChatMessage::user(format!("Message {}", i)));
            messages.push(ChatMessage::assistant(format!("Reply {}", i)));
        }
        let query = ResearchContext::build_search_query(&messages);
        assert!(query.contains("Message 4"));
        assert!(query.contains("Message 3"));
        assert!(query.contains("Message 2"));
        assert!(!query.contains("Message 1"));
    }

    #[test]
    fn test_format_both_empty() {
        let formatted = ResearchContext::format_auto_search_results(&[], &[]);
        assert!(formatted.contains("未找到"));
    }

    #[test]
    fn test_format_wiki_hits() {
        let wiki = vec![WikiHit {
            title: "Tokio Runtime".into(),
            path: "topics/tokio".into(),
            score: 0.92,
            summary: "Async runtime".into(),
        }];
        let formatted = ResearchContext::format_auto_search_results(&wiki, &[]);
        assert!(formatted.contains("Tokio Runtime"));
        assert!(formatted.contains("0.92"));
    }

    #[test]
    fn test_format_history_hits() {
        let history = vec![HistoryHit {
            role: "user".into(),
            content: "How to use tokio?".into(),
            timestamp: "2026-04-29".into(),
        }];
        let formatted = ResearchContext::format_auto_search_results(&[], &history);
        assert!(formatted.contains("How to use tokio?"));
    }
}
