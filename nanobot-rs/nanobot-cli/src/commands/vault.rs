//! Vault commands implementation
//!
//! CLI commands for managing sensitive data in the vault.

use anyhow::{Context, Result};
use colored::Colorize;
use dialoguer::{Input, Password};

use nanobot_core::vault::VaultStore;

/// List all vault entries (values excluded for security)
pub async fn cmd_vault_list() -> Result<()> {
    println!("{}\n", "Vault Entries".bold());

    let store = VaultStore::new().context("Failed to open vault store")?;

    let keys = store.list_keys();

    if keys.is_empty() {
        println!("No vault entries found.");
        println!("\nUse 'nanobot vault set <key>' to create a new entry.");
        return Ok(());
    }

    println!("{:<20} {:<40} {:<20}", "Key", "Description", "Created");
    println!("{}", "─".repeat(80));

    for meta in keys {
        let desc = meta
            .description
            .as_deref()
            .unwrap_or("-")
            .chars()
            .take(38)
            .collect::<String>();
        let created = meta.created_at.format("%Y-%m-%d %H:%M").to_string();

        println!(
            "{:<20} {:<40} {:<20}",
            meta.key.cyan(),
            desc.dimmed(),
            created.dimmed()
        );
    }

    println!();
    println!("Total: {} entries", store.len());
    println!(
        "\n{}",
        "Tip: Use {{vault:key}} in your messages to inject secrets at runtime.".dimmed()
    );

    Ok(())
}

/// Set a vault entry
pub async fn cmd_vault_set(
    key: String,
    value: Option<String>,
    description: Option<String>,
) -> Result<()> {
    let store = VaultStore::new().context("Failed to open vault store")?;

    // Validate key format first
    if key.is_empty() || !key.chars().all(|c| c.is_alphanumeric() || c == '_') {
        anyhow::bail!(
            "Invalid key '{}'. Key must contain only alphanumeric characters and underscores.",
            key
        );
    }

    // Check if updating existing entry
    let is_update = store.exists(&key);

    // Get value interactively if not provided
    let final_value = match value {
        Some(v) => v,
        None => {
            // Use Password prompt to hide input
            Password::new()
                .with_prompt(format!("Enter value for '{}'", key))
                .interact()
                .context("Failed to read value")?
        }
    };

    // Get description interactively if not provided
    let final_desc = match description {
        Some(d) => Some(d),
        None => {
            let default_desc = if is_update {
                store
                    .list_keys()
                    .iter()
                    .find(|m| m.key == key)
                    .and_then(|m| m.description.clone())
            } else {
                None
            };

            let input: String = Input::new()
                .with_prompt("Description (optional)")
                .default(default_desc.unwrap_or_default())
                .allow_empty(true)
                .interact()
                .context("Failed to read description")?;

            if input.is_empty() {
                None
            } else {
                Some(input)
            }
        }
    };

    store
        .set(&key, &final_value, final_desc.as_deref())
        .context("Failed to set vault entry")?;

    if is_update {
        println!("{} Updated vault entry: {}", "✓".green(), key.bold());
    } else {
        println!("{} Created vault entry: {}", "✓".green(), key.bold());
    }

    println!();
    println!("Usage in messages: {{vault:{}}}", key.cyan());
    println!("Storage location: ~/.nanobot/vault/secrets.json");

    Ok(())
}

/// Get a vault entry value
pub async fn cmd_vault_get(key: String) -> Result<()> {
    let store = VaultStore::new().context("Failed to open vault store")?;

    let value = store.get(&key);

    match value {
        Some(v) => {
            println!("{}", v);
        }
        None => {
            println!("{} Key not found: {}", "✗".red(), key);
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Delete a vault entry
pub async fn cmd_vault_delete(key: String, force: bool) -> Result<()> {
    let store = VaultStore::new().context("Failed to open vault store")?;

    if !store.exists(&key) {
        println!("{} Key not found: {}", "✗".red(), key);
        return Ok(());
    }

    // Confirm deletion unless --force is specified
    if !force {
        println!(
            "{} Are you sure you want to delete '{}'?",
            "⚠".yellow(),
            key.bold()
        );

        let confirm: bool = dialoguer::Confirm::new()
            .with_prompt("Delete this entry?")
            .default(false)
            .interact()
            .context("Failed to read confirmation")?;

        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let deleted = store.delete(&key).context("Failed to delete vault entry")?;

    if deleted {
        println!("{} Deleted vault entry: {}", "✓".green(), key.bold());
    }

    Ok(())
}

/// Show detailed info for a vault entry (value excluded)
pub async fn cmd_vault_show(key: String, show_value: bool) -> Result<()> {
    let store = VaultStore::new().context("Failed to open vault store")?;

    let keys = store.list_keys();
    let meta = keys.iter().find(|m| m.key == key);

    match meta {
        Some(m) => {
            println!("{}", m.key.cyan().bold());
            println!();

            if let Some(desc) = &m.description {
                println!("  Description: {}", desc);
            } else {
                println!("  Description: {}", "-".dimmed());
            }

            println!(
                "  Created:     {}",
                m.created_at.format("%Y-%m-%d %H:%M UTC")
            );
            println!(
                "  Last used:   {}",
                m.last_used
                    .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
                    .unwrap_or_else(|| "Never".to_string())
            );

            if show_value {
                if let Some(value) = store.get(&key) {
                    println!();
                    println!("  Value:");
                    println!("    {}", value.yellow());
                }
            } else {
                println!();
                println!(
                    "  {}",
                    "Use --show-value to display the secret value.".dimmed()
                );
            }

            println!();
            println!("  Usage: {{vault:{}}}", key.cyan());
        }
        None => {
            println!("{} Key not found: {}", "✗".red(), key);
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Import vault entries from a JSON file
pub async fn cmd_vault_import(file_path: String, merge: bool) -> Result<()> {
    let store = VaultStore::new().context("Failed to open vault store")?;

    let content = std::fs::read_to_string(&file_path).context("Failed to read import file")?;

    let imported: std::collections::HashMap<String, nanobot_core::vault::VaultEntry> =
        serde_json::from_str(&content).context("Failed to parse JSON file")?;

    if imported.is_empty() {
        println!("No entries found in import file.");
        return Ok(());
    }

    let existing_count = store.len();

    if !merge && existing_count > 0 {
        println!(
            "{} Vault already contains {} entries.",
            "⚠".yellow(),
            existing_count
        );
        let confirm: bool = dialoguer::Confirm::new()
            .with_prompt("Continue importing? Existing entries with same keys will be overwritten.")
            .default(false)
            .interact()
            .context("Failed to read confirmation")?;

        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let mut imported_count = 0;
    let mut skipped_count = 0;

    for (_, entry) in imported {
        if !merge || !store.exists(&entry.key) {
            store
                .set(&entry.key, &entry.value, entry.description.as_deref())
                .context("Failed to import entry")?;
            imported_count += 1;
        } else {
            skipped_count += 1;
        }
    }

    println!(
        "{} Imported {} entries ({} skipped)",
        "✓".green(),
        imported_count,
        skipped_count
    );

    Ok(())
}

/// Export vault entries to a JSON file
pub async fn cmd_vault_export(file_path: String) -> Result<()> {
    let store = VaultStore::new().context("Failed to open vault store")?;

    let keys = store.list_keys();

    if keys.is_empty() {
        println!("No vault entries to export.");
        return Ok(());
    }

    // Build export data
    let mut export_data = std::collections::HashMap::new();
    for meta in keys {
        if let Some(value) = store.get(&meta.key) {
            let entry = nanobot_core::vault::VaultEntry {
                key: meta.key.clone(),
                value,
                description: meta.description.clone(),
                created_at: meta.created_at,
                last_used: meta.last_used,
            };
            export_data.insert(meta.key, entry);
        }
    }

    let content =
        serde_json::to_string_pretty(&export_data).context("Failed to serialize vault data")?;

    std::fs::write(&file_path, content).context("Failed to write export file")?;

    println!(
        "{} Exported {} entries to: {}",
        "✓".green(),
        export_data.len(),
        file_path
    );
    println!();
    println!(
        "{}",
        "⚠ Warning: Exported file contains sensitive values. Handle with care!".yellow()
    );

    Ok(())
}
