-- Add file_size column to memory_metadata table
-- Stores the filesystem size (in bytes) of the .md file
-- Used together with file_mtime for cache invalidation:
-- if disk_mtime <= sqlite_mtime AND disk_size == sqlite_size, skip re-indexing
-- This handles low-precision filesystems (e.g., 1-second mtime resolution in Docker)

ALTER TABLE memory_metadata ADD COLUMN file_size BIGINT DEFAULT 0;

-- Add index for efficient cache invalidation queries
CREATE INDEX IF NOT EXISTS idx_memory_metadata_size ON memory_metadata(file_size);
