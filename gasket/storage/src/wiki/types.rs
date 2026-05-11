//! Wiki page types — core data structures for the wiki subsystem.
//!
//! These live with the persistence layer rather than in `gasket-types`
//! because wiki pages are a storage-tier concept, not a leaf-level message
//! type shared workspace-wide.

use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

// ── Frequency ──────────────────────────────────────────────────

/// Wiki page access frequency classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Frequency {
    Hot,
    Warm,
    Cold,
    #[default]
    Archived,
}

impl PartialOrd for Frequency {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Frequency {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.rank().cmp(&other.rank())
    }
}

impl Frequency {
    pub fn rank(self) -> u8 {
        match self {
            Frequency::Hot => 3,
            Frequency::Warm => 2,
            Frequency::Cold => 1,
            Frequency::Archived => 0,
        }
    }

    pub fn from_str_lossy(s: &str) -> Frequency {
        match s.to_lowercase().as_str() {
            "hot" => Frequency::Hot,
            "warm" => Frequency::Warm,
            "cold" => Frequency::Cold,
            _ => Frequency::Archived,
        }
    }
}

impl fmt::Display for Frequency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Frequency::Hot => write!(f, "hot"),
            Frequency::Warm => write!(f, "warm"),
            Frequency::Cold => write!(f, "cold"),
            Frequency::Archived => write!(f, "archived"),
        }
    }
}

impl FromStr for Frequency {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "hot" => Ok(Frequency::Hot),
            "warm" => Ok(Frequency::Warm),
            "cold" => Ok(Frequency::Cold),
            "archived" => Ok(Frequency::Archived),
            _ => Err(format!("unknown frequency: {}", s)),
        }
    }
}

// ── PageType ───────────────────────────────────────────────────

/// Page type classification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PageType {
    Entity,
    Topic,
    Source,
    Sop,
}

impl PageType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Entity => "entity",
            Self::Topic => "topic",
            Self::Source => "source",
            Self::Sop => "sop",
        }
    }

    pub fn directory(&self) -> &'static str {
        match self {
            Self::Entity => "entities",
            Self::Topic => "topics",
            Self::Source => "sources",
            Self::Sop => "sops",
        }
    }
}

impl FromStr for PageType {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "entity" => Ok(Self::Entity),
            "topic" => Ok(Self::Topic),
            "source" => Ok(Self::Source),
            "sop" => Ok(Self::Sop),
            _ => Err(()),
        }
    }
}

// ── PageFilter ─────────────────────────────────────────────────

/// Filter for listing wiki pages.
#[derive(Debug, Clone, Default)]
pub struct PageFilter {
    pub page_type: Option<PageType>,
    pub category: Option<String>,
    pub tags: Vec<String>,
}

// ── PageSummary ────────────────────────────────────────────────

/// Lightweight page summary (no content).
#[derive(Debug, Clone)]
pub struct PageSummary {
    pub path: String,
    pub title: String,
    pub page_type: PageType,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub updated: DateTime<Utc>,
    pub summary: Option<String>,
    pub confidence: f64,
    pub frequency: Frequency,
    pub access_count: u64,
    pub last_accessed: Option<DateTime<Utc>>,
    pub content_length: u64,
    pub file_mtime: i64,
}

// ── WikiPage ───────────────────────────────────────────────────

/// A wiki page. Markdown files on disk are the SSOT.
/// SQLite and Tantivy are derived projections (cache + index).
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Markdown body content.
    pub content: String,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    pub source_count: u32,
    pub confidence: f64,
    // Machine runtime state (never written to Markdown):
    #[serde(skip)]
    pub frequency: Frequency,
    #[serde(skip)]
    pub access_count: u64,
    #[serde(skip)]
    pub last_accessed: Option<DateTime<Utc>>,
    #[serde(skip)]
    pub file_mtime: i64,
}

impl WikiPage {
    pub fn new(path: String, title: String, page_type: PageType, content: String) -> Self {
        let now = Utc::now();
        Self {
            path,
            title,
            page_type,
            category: None,
            tags: vec![],
            summary: None,
            content,
            created: now,
            updated: now,
            source_count: 0,
            confidence: 1.0,
            frequency: Frequency::default(),
            access_count: 0,
            last_accessed: None,
            file_mtime: 0,
        }
    }

    pub fn make_path(parts: &[&str]) -> String {
        parts.join("/")
    }

    /// Serialize to Markdown with YAML frontmatter.
    pub fn to_markdown(&self) -> String {
        let mut md = String::from("---\n");
        md.push_str(&format!("title: {:?}\n", self.title));
        md.push_str(&format!("type: {}\n", self.page_type.as_str()));
        if let Some(ref cat) = self.category {
            md.push_str(&format!("category: {}\n", cat));
        }
        if !self.tags.is_empty() {
            md.push_str("tags:\n");
            for tag in &self.tags {
                md.push_str(&format!("  - {}\n", tag));
            }
        }
        if let Some(ref s) = self.summary {
            md.push_str(&format!("summary: {}\n", s));
        }
        md.push_str("---\n\n");
        md.push_str(&self.content);
        md
    }

    /// Parse from Markdown with YAML frontmatter.
    pub fn from_markdown(path: String, markdown: &str) -> anyhow::Result<Self> {
        let markdown = markdown.trim_start();
        if !markdown.starts_with("---") {
            anyhow::bail!("Missing YAML frontmatter");
        }
        let rest = &markdown[3..];
        let end = rest
            .find("\n---")
            .ok_or_else(|| anyhow::anyhow!("Unclosed frontmatter"))?;
        let yaml = &rest[..end];
        let body = rest[end + 4..].trim();

        #[derive(Deserialize)]
        struct FrontMatter {
            title: String,
            #[serde(rename = "type")]
            page_type: String,
            category: Option<String>,
            #[serde(default)]
            tags: Vec<String>,
            summary: Option<String>,
        }

        let fm: FrontMatter = serde_yaml::from_str(yaml)?;
        let page_type: PageType = fm
            .page_type
            .parse()
            .map_err(|_| anyhow::anyhow!("Unknown page type: {}", fm.page_type))?;

        let summary = fm.summary.or_else(|| {
            if body.len() > 100 {
                Some(format!("{}...", &body[..100]))
            } else if !body.is_empty() {
                Some(body.to_string())
            } else {
                None
            }
        });

        Ok(Self {
            path,
            title: fm.title,
            page_type,
            category: fm.category,
            tags: fm.tags,
            summary,
            content: body.to_string(),
            created: Utc::now(),
            updated: Utc::now(),
            source_count: 0,
            confidence: 1.0,
            frequency: Frequency::default(),
            access_count: 0,
            last_accessed: None,
            file_mtime: 0,
        })
    }
}

// ── slugify ────────────────────────────────────────────────────

/// Convert a title to a URL-safe slug.
pub fn slugify(s: &str) -> String {
    let re = Regex::new(r"[^a-zA-Z0-9一-鿿]+").unwrap();
    let slug = re.replace_all(s, "-").to_string();
    let re_trim = Regex::new(r"^-+|-+$").unwrap();
    re_trim
        .replace_all(
            &slug.split('-').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("-"),
            "",
        )
        .to_string()
        .to_lowercase()
}
