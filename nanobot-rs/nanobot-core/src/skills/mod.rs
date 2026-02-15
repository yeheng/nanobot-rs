//! Skills system for dynamic skill loading and management
//!
//! The skills system allows loading skills from Markdown files with YAML frontmatter.
//! Skills can have dependencies (binaries, environment variables) and can be loaded
//! progressively (always-load vs on-demand).

mod loader;
mod metadata;
mod registry;
mod skill;

pub use loader::{parse_skill_file, SkillsLoader};
pub use metadata::SkillMetadata;
pub use registry::SkillsRegistry;
pub use skill::Skill;
