-- Add file_mtime column to memory_metadata table
-- Stores the filesystem mtime (nanoseconds since UNIX_EPOCH) of the .md file
-- Used for cache invalidation: if disk_mtime <= sqlite_mtime, skip re-indexing

ALTER TABLE memory_metadata ADD COLUMN file_mtime BIGINT DEFAULT 0;

-- Add index for efficient decay candidate queries
CREATE INDEX IF NOT EXISTS idx_memory_metadata_mtime ON memory_metadata(file_mtime);
