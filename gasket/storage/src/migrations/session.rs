//! Session and conversation history tables.

use sqlx::SqlitePool;

/// Run session schema migrations (tables + indexes).
pub async fn run_schema(pool: &SqlitePool) -> anyhow::Result<()> {
    create_sessions_table(pool).await?;
    create_events_table(pool).await?;
    create_summaries_table(pool).await?;
    create_checkpoints_table(pool).await?;
    create_session_embeddings_table(pool).await?;
    create_session_indexes(pool).await?;
    Ok(())
}

async fn create_sessions_table(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS sessions_v2 (
            key             TEXT PRIMARY KEY,
            channel         TEXT NOT NULL DEFAULT '',
            chat_id         TEXT NOT NULL DEFAULT '',
            created_at      TEXT NOT NULL,
            updated_at      TEXT NOT NULL,
            last_consolidated_event TEXT,
            total_events    INTEGER NOT NULL DEFAULT 0,
            total_tokens    INTEGER NOT NULL DEFAULT 0
        )
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn create_events_table(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS session_events (
            id              TEXT PRIMARY KEY,
            session_key     TEXT NOT NULL,
            channel         TEXT NOT NULL DEFAULT '',
            chat_id         TEXT NOT NULL DEFAULT '',
            event_type      TEXT NOT NULL,
            content         TEXT NOT NULL,
            tools_used      TEXT DEFAULT '[]',
            token_usage     TEXT,
            token_len       INTEGER NOT NULL DEFAULT 0,
            event_data      TEXT,
            extra           TEXT DEFAULT '{}',
            created_at      TEXT NOT NULL,
            sequence        INTEGER NOT NULL DEFAULT 0,
            FOREIGN KEY (session_key) REFERENCES sessions_v2(key) ON DELETE CASCADE
        )
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn create_summaries_table(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS session_summaries (
            session_key            TEXT PRIMARY KEY,
            content                TEXT NOT NULL,
            covered_upto_sequence  INTEGER NOT NULL DEFAULT 0,
            created_at             TEXT NOT NULL,
            compaction_in_progress INTEGER NOT NULL DEFAULT 0,
            compaction_started_at  TEXT
        )",
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn create_checkpoints_table(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS session_checkpoints (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            session_key     TEXT NOT NULL,
            target_sequence INTEGER NOT NULL,
            summary         TEXT NOT NULL,
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(session_key, target_sequence)
        )
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn create_session_embeddings_table(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS session_embeddings (
            message_id  TEXT PRIMARY KEY,
            session_key TEXT NOT NULL,
            embedding   BLOB NOT NULL,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (session_key) REFERENCES sessions_v2(key) ON DELETE CASCADE
        )",
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn create_session_indexes(pool: &SqlitePool) -> anyhow::Result<()> {
    // sessions_v2 indexes
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_channel ON sessions_v2(channel)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_chat_id ON sessions_v2(chat_id)")
        .execute(pool)
        .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_sessions_v2_channel_chat ON sessions_v2(channel, chat_id)",
    )
    .execute(pool)
    .await?;

    // session_events indexes
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_events_channel_chat ON session_events(channel, chat_id)",
    )
    .execute(pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_events_created ON session_events(created_at)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_events_type ON session_events(event_type)")
        .execute(pool)
        .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_events_session_type_created \
         ON session_events(session_key, event_type, created_at DESC)",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_events_channel_chat_sequence \
         ON session_events(channel, chat_id, sequence)",
    )
    .execute(pool)
    .await?;

    // session_checkpoints indexes
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_session_checkpoints_key_seq \
         ON session_checkpoints(session_key, target_sequence)",
    )
    .execute(pool)
    .await?;

    // session_embeddings indexes
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_session_embeddings_session_key \
         ON session_embeddings(session_key)",
    )
    .execute(pool)
    .await?;

    Ok(())
}
