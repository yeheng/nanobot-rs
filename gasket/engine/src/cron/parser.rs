//! Cron job file parsing and serialization

use std::path::Path;

use anyhow::anyhow;

use super::types::{CronJob, CronJobFrontmatter};

/// Extract YAML frontmatter and body from markdown content.
pub(super) fn extract_frontmatter_raw(content: &str) -> anyhow::Result<(String, String)> {
    let content = content.trim_start();

    if !content.starts_with("---") {
        anyhow::bail!("Invalid markdown format: missing frontmatter start delimiter '---'");
    }

    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        anyhow::bail!("Invalid markdown format: missing frontmatter end delimiter '---'");
    }

    let mut close_idx = None;
    for (i, line) in lines.iter().enumerate().skip(1) {
        if line.trim() == "---" {
            close_idx = Some(i);
            break;
        }
    }

    let close_idx = close_idx.ok_or_else(|| anyhow::anyhow!("Invalid markdown format: missing frontmatter end delimiter '---'"))?;

    let yaml_lines = &lines[1..close_idx];
    let body_lines = &lines[close_idx + 1..];

    let yaml_str = yaml_lines.join("\n");
    let body = body_lines.join("\n").trim_start().to_string();

    Ok((yaml_str, body))
}

/// Parse markdown content into a CronJob.
pub(super) fn parse_markdown(content: &str, file_path: &Path) -> anyhow::Result<CronJob> {
    let (yaml_str, body) = extract_frontmatter_raw(content)?;

    let fm: CronJobFrontmatter = serde_yaml::from_str(&yaml_str)
        .map_err(|e| anyhow!("Failed to parse YAML frontmatter: {}", e))?;

    let (schedule, next_run) = CronJob::parse_schedule(&fm.cron);

    let id = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    Ok(CronJob {
        id: id.clone(),
        name: fm.name.unwrap_or(id),
        cron: fm.cron,
        message: body,
        channel: fm.channel,
        chat_id: fm.to,
        tool: fm.tool,
        tool_args: fm.tool_args,
        last_run: None,
        next_run,
        enabled: fm.enabled,
        file_path: file_path.to_path_buf(),
        schedule,
    })
}

/// Parse a single markdown cron job file from disk.
pub(super) fn parse_markdown_file(path: &Path) -> anyhow::Result<CronJob> {
    let content = std::fs::read_to_string(path)?;
    parse_markdown(&content, path)
}

/// Serialize a CronJob into markdown + YAML frontmatter format.
///
/// Uses serde_yaml for proper escaping of special characters.
pub(super) fn serialize_to_markdown(job: &CronJob) -> anyhow::Result<String> {
    let mut frontmatter = serde_yaml::Mapping::new();
    frontmatter.insert(
        serde_yaml::Value::String("name".into()),
        serde_yaml::Value::String(job.name.clone()),
    );
    frontmatter.insert(
        serde_yaml::Value::String("cron".into()),
        serde_yaml::Value::String(job.cron.clone()),
    );
    frontmatter.insert(
        serde_yaml::Value::String("channel".into()),
        serde_yaml::Value::String(job.channel.clone().unwrap_or_default()),
    );
    frontmatter.insert(
        serde_yaml::Value::String("to".into()),
        serde_yaml::Value::String(job.chat_id.clone().unwrap_or_default()),
    );
    frontmatter.insert(
        serde_yaml::Value::String("enabled".into()),
        serde_yaml::Value::String(job.enabled.to_string()),
    );

    if let Some(ref tool) = job.tool {
        frontmatter.insert(
            serde_yaml::Value::String("tool".into()),
            serde_yaml::Value::String(tool.clone()),
        );
    }
    if let Some(ref args) = job.tool_args {
        frontmatter.insert(
            serde_yaml::Value::String("tool_args".into()),
            serde_yaml::to_value(args)?,
        );
    }

    let yaml_str = serde_yaml::to_string(&frontmatter)?;
    Ok(format!("---\n{}---\n\n{}", yaml_str, job.message))
}
