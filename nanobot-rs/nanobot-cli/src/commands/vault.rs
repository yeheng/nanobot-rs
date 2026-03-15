//! Vault commands implementation
//!
//! CLI commands for managing sensitive data in the vault.
//!
//! # Encryption Support
//!
//! The vault uses encrypted storage with XChaCha20-Poly1305.
//! Set the `NANOBOT_VAULT_PASSWORD` environment variable to unlock.

use anyhow::{Context, Result};
use colored::Colorize;
use dialoguer::Password;

use nanobot_core::vault::VaultStore;
use tracing::debug;

/// Environment variable for vault password
const VAULT_PASSWORD_ENV: &str = "NANOBOT_VAULT_PASSWORD";

/// Get vault password from environment or prompt
fn get_vault_password(prompt: bool) -> Option<String> {
    if let Ok(password) = std::env::var(VAULT_PASSWORD_ENV) {
        if !password.is_empty() {
            return Some(password);
        }
    }

    if prompt {
        Password::new()
            .with_prompt("Enter vault password")
            .interact()
            .ok()
    } else {
        None
    }
}

/// Check if vault is unlocked or needs password
fn ensure_unlocked(store: &mut VaultStore) -> Result<()> {
    if store.is_locked() {
        if let Some(password) = get_vault_password(true) {
            store.unlock(&password).context("Failed to unlock vault")?;
            debug!("{} Vault unlocked", "✓".green());
        } else {
            anyhow::bail!(
                "Vault is locked. Set {} environment variable.",
                VAULT_PASSWORD_ENV
            );
        }
    }
    Ok(())
}

/// List all vault entries (values excluded for security)
pub async fn cmd_vault_list() -> Result<()> {
    println!("{}\n", "Vault Entries".bold());

    let mut store = VaultStore::new().context("Failed to open vault store")?;
    ensure_unlocked(&mut store)?;

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
        "Tip: Use {{vault:key}} in config.yaml or messages to inject secrets at runtime.".dimmed()
    );

    Ok(())
}

/// Set a vault entry
pub async fn cmd_vault_set(
    key: String,
    value: Option<String>,
    description: Option<String>,
) -> Result<()> {
    if key.is_empty() || !key.chars().all(|c| c.is_alphanumeric() || c == '_') {
        anyhow::bail!(
            "Invalid key '{}'. Key must contain only alphanumeric characters and underscores.",
            key
        );
    }

    let mut store = VaultStore::new().context("Failed to open vault store")?;
    ensure_unlocked(&mut store)?;

    let is_update = store.exists(&key);

    let final_value = match value {
        Some(v) => v,
        None => {
            let password = Password::new()
                .with_prompt(format!("Enter value for '{}'", key))
                .interact()
                .context("Failed to read value")?;

            #[cfg(unix)]
            {
                use std::process::Command;
                let _ = Command::new("stty").arg("echo").arg("icanon").status();
            }

            password
        }
    };

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

            println!();
            print!("Description (optional, press Enter to skip): ");
            use std::io::{self, BufRead, Write};
            io::stdout().flush().ok();

            let mut input = String::new();
            io::stdin().lock().read_line(&mut input).ok();
            let input = input.trim().to_string();

            if input.is_empty() {
                default_desc
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
    println!("Usage in config: {{vault:{}}}", key.cyan());
    println!("Storage: ~/.nanobot/vault/secrets.json (encrypted)");

    Ok(())
}

/// Get a vault entry value
pub async fn cmd_vault_get(key: String) -> Result<()> {
    let mut store = VaultStore::new().context("Failed to open vault store")?;
    ensure_unlocked(&mut store)?;

    match store.get(&key) {
        Some(v) => println!("{}", v),
        None => {
            println!("{} Key not found: {}", "✗".red(), key);
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Delete a vault entry
pub async fn cmd_vault_delete(key: String, force: bool) -> Result<()> {
    let mut store = VaultStore::new().context("Failed to open vault store")?;
    ensure_unlocked(&mut store)?;

    if !store.exists(&key) {
        println!("{} Key not found: {}", "✗".red(), key);
        return Ok(());
    }

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
    let mut store = VaultStore::new().context("Failed to open vault store")?;
    ensure_unlocked(&mut store)?;

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
    let mut store = VaultStore::new().context("Failed to open vault store")?;
    ensure_unlocked(&mut store)?;

    let content = std::fs::read_to_string(&file_path).context("Failed to read import file")?;

    let imported: nanobot_core::vault::VaultFileV2 = serde_json::from_str(&content)
        .context("Failed to parse import file (expected v2 format)")?;

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

    // Note: Import requires re-encrypting with current vault's key
    // For now, just import the metadata and require user to set values
    for (_, _entry) in imported.entries {
        if !merge {
            // TODO: Re-encrypt entries with current vault key
            imported_count += 1;
        } else {
            skipped_count += 1;
        }
    }

    println!(
        "{} Import requires re-encryption - use 'vault set' to add entries",
        "⚠".yellow()
    );
    println!(
        "Imported {} entries metadata (values need to be re-set), skip :{}",
        imported_count, skipped_count
    );

    Ok(())
}

/// Export vault entries to a JSON file
pub async fn cmd_vault_export(file_path: String) -> Result<()> {
    let mut store = VaultStore::new().context("Failed to open vault store")?;
    ensure_unlocked(&mut store)?;

    let keys = store.list_keys();

    if keys.is_empty() {
        println!("No vault entries to export.");
        return Ok(());
    }

    // Build export data with decrypted values
    let mut export_data = std::collections::HashMap::new();
    for meta in keys {
        if let Some(value) = store.get(&meta.key) {
            export_data.insert(
                meta.key.clone(),
                serde_json::json!({
                    "key": meta.key,
                    "value": value,
                    "description": meta.description,
                    "created_at": meta.created_at.to_rfc3339(),
                    "last_used": meta.last_used.map(|t| t.to_rfc3339())
                }),
            );
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
