//! Permission matrix for multi-agent delegation.
//!
//! Encodes which agent roles are allowed to delegate work to other roles.
//! The default matrix mirrors the 三省六部 hierarchy:
//!
//! ```text
//! taizi    → [zhongshu]
//! zhongshu → [menxia]
//! menxia   → [shangshu, zhongshu]   // can reject back to planning
//! shangshu → [li, hu, bing, xing, gong, dianzhong]
//! 六部     → [shangshu]              // report back to dispatch only
//! ```

use std::collections::{HashMap, HashSet};

/// A directed graph of `caller → allowed_targets` that governs
/// which agent role may delegate to which other roles.
#[derive(Debug, Clone)]
pub struct PermissionMatrix {
    /// Map from caller role to the set of target roles it may invoke.
    allowed: HashMap<String, HashSet<String>>,
}

impl PermissionMatrix {
    /// Build a matrix from an explicit map.
    pub fn new(allowed: HashMap<String, HashSet<String>>) -> Self {
        Self { allowed }
    }

    /// Build the default 三省六部 hierarchy.
    pub fn default_sangsheng() -> Self {
        let mut m: HashMap<String, HashSet<String>> = HashMap::new();

        // 太子 → 中书省
        m.insert("taizi".into(), ["zhongshu"].into_iter().map(Into::into).collect());

        // 中书省 → 门下省
        m.insert("zhongshu".into(), ["menxia"].into_iter().map(Into::into).collect());

        // 门下省 → 尚书省 (approve) or 中书省 (reject back)
        m.insert(
            "menxia".into(),
            ["shangshu", "zhongshu"].into_iter().map(Into::into).collect(),
        );

        // 尚书省 → 六部
        m.insert(
            "shangshu".into(),
            ["li", "hu", "bing", "xing", "gong", "dianzhong"]
                .into_iter()
                .map(Into::into)
                .collect(),
        );

        // 六部 → 尚书省 only (report back)
        for ministry in &["li", "hu", "bing", "xing", "gong", "dianzhong"] {
            m.insert(
                (*ministry).into(),
                ["shangshu"].into_iter().map(Into::into).collect(),
            );
        }

        Self { allowed: m }
    }

    /// Check whether `caller` is allowed to delegate to `target`.
    pub fn is_allowed(&self, caller: &str, target: &str) -> bool {
        self.allowed
            .get(caller)
            .is_some_and(|targets| targets.contains(target))
    }

    /// Return the set of targets a caller may invoke.
    pub fn targets_for(&self, caller: &str) -> Option<&HashSet<String>> {
        self.allowed.get(caller)
    }

    /// Merge additional permissions on top of the existing matrix.
    /// Useful for extending the default with user-supplied config.
    pub fn merge(&mut self, extra: HashMap<String, HashSet<String>>) {
        for (role, targets) in extra {
            self.allowed.entry(role).or_default().extend(targets);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_hierarchy() {
        let pm = PermissionMatrix::default_sangsheng();

        // Forward flow
        assert!(pm.is_allowed("taizi", "zhongshu"));
        assert!(pm.is_allowed("zhongshu", "menxia"));
        assert!(pm.is_allowed("menxia", "shangshu"));
        assert!(pm.is_allowed("shangshu", "gong"));
        assert!(pm.is_allowed("shangshu", "li"));

        // Reject path
        assert!(pm.is_allowed("menxia", "zhongshu"));

        // Report back
        assert!(pm.is_allowed("gong", "shangshu"));
        assert!(pm.is_allowed("hu", "shangshu"));
    }

    #[test]
    fn test_disallowed() {
        let pm = PermissionMatrix::default_sangsheng();

        // Skip levels
        assert!(!pm.is_allowed("taizi", "shangshu"));
        assert!(!pm.is_allowed("taizi", "gong"));

        // Reverse flow (except menxia→zhongshu)
        assert!(!pm.is_allowed("shangshu", "menxia"));
        assert!(!pm.is_allowed("zhongshu", "taizi"));

        // Ministry cross-talk
        assert!(!pm.is_allowed("gong", "li"));
    }

    #[test]
    fn test_merge_extends() {
        let mut pm = PermissionMatrix::default_sangsheng();
        let mut extra = HashMap::new();
        extra.insert(
            "taizi".into(),
            ["menxia"].into_iter().map(Into::into).collect(),
        );
        pm.merge(extra);

        // Original still works
        assert!(pm.is_allowed("taizi", "zhongshu"));
        // New permission added
        assert!(pm.is_allowed("taizi", "menxia"));
    }

    #[test]
    fn test_custom_matrix() {
        let mut m = HashMap::new();
        m.insert("a".into(), ["b", "c"].into_iter().map(Into::into).collect());

        let pm = PermissionMatrix::new(m);
        assert!(pm.is_allowed("a", "b"));
        assert!(pm.is_allowed("a", "c"));
        assert!(!pm.is_allowed("b", "a"));
    }
}
