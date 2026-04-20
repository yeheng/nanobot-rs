use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Page type classification
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PageType {
    Entity,
    Topic,
    Source,
}

impl PageType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Entity => "entity",
            Self::Topic => "topic",
            Self::Source => "source",
        }
    }

}

impl std::str::FromStr for PageType {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "entity" => Ok(Self::Entity),
            "topic" => Ok(Self::Topic),
            "source" => Ok(Self::Source),
            _ => Err(()),
        }
    }
}

/// A wiki page. One struct. One constructor. No special cases.
/// SQLite is the single truth source. Disk files are derived cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WikiPage {
    /// Relative path under wiki root: "entities/projects/gasket"
    pub path: String,
    pub title: String,
    pub page_type: PageType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub content: String,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    #[serde(default)]
    pub source_count: u32,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
}

fn default_confidence() -> f64 {
    1.0
}

impl WikiPage {
    /// One constructor. All page types go through this.
    pub fn new(path: String, title: String, page_type: PageType, content: String) -> Self {
        let now = Utc::now();
        Self {
            path,
            title,
            page_type,
            content,
            category: None,
            tags: vec![],
            created: now,
            updated: now,
            source_count: 0,
            confidence: 1.0,
        }
    }

    /// Helper: build a path from parts: ["entities", "projects", "gasket"]
    pub fn make_path(parts: &[&str]) -> String {
        parts.join("/")
    }

    /// Convert to markdown for disk export (optional cache)
    pub fn to_markdown(&self) -> String {
        let mut out = String::from("---\n");
        out.push_str(&serde_yaml::to_string(&self).unwrap_or_default());
        out.push_str("---\n\n");
        out.push_str(&self.content);
        out
    }

    /// Parse from markdown (used only for migration / disk cache rebuild)
    pub fn from_markdown(path: String, markdown: &str) -> anyhow::Result<Self> {
        let content = markdown.trim_start();
        if !content.starts_with("---") {
            anyhow::bail!("missing frontmatter delimiter");
        }
        let rest = &content[3..];
        let end = rest
            .find("\n---")
            .ok_or_else(|| anyhow::anyhow!("unclosed frontmatter"))?;
        let yaml = &rest[..end];
        let body = rest[end + 4..].trim_start_matches('\n').trim_start();
        let mut page: WikiPage = serde_yaml::from_str(yaml)?;
        page.path = path;
        page.content = body.to_string();
        Ok(page)
    }
}

/// Summary for listing (no content — lightweight)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageSummary {
    pub path: String,
    pub title: String,
    pub page_type: PageType,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub updated: DateTime<Utc>,
    pub confidence: f64,
}

/// Filter for listing pages
#[derive(Debug, Clone, Default)]
pub struct PageFilter {
    pub page_type: Option<PageType>,
    pub category: Option<String>,
    pub tags: Vec<String>,
}

/// Slugify a string for use in paths
pub fn slugify(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}
