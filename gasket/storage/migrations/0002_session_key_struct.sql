-- Add structured channel/chat_id columns to sessions_v2 and session_events
-- Enables efficient queries like "all Telegram conversations" or "user X across all platforms"

-- For sessions_v2: add columns, keep session_key for backward compat
ALTER TABLE sessions_v2 ADD COLUMN channel TEXT NOT NULL DEFAULT '';
ALTER TABLE sessions_v2 ADD COLUMN chat_id TEXT NOT NULL DEFAULT '';

-- Populate from existing session_key
UPDATE sessions_v2
SET channel = SUBSTR(key, 1, INSTR(key, ':') - 1),
    chat_id = SUBSTR(key, INSTR(key, ':') + 1)
WHERE INSTR(key, ':') > 0;

-- For session_events: add columns, keep session_key for backward compat
ALTER TABLE session_events ADD COLUMN channel TEXT NOT NULL DEFAULT '';
ALTER TABLE session_events ADD COLUMN chat_id TEXT NOT NULL DEFAULT '';

-- Populate from existing session_key
UPDATE session_events
SET channel = SUBSTR(session_key, 1, INSTR(session_key, ':') - 1),
    chat_id = SUBSTR(session_key, INSTR(session_key, ':') + 1)
WHERE INSTR(session_key, ':') > 0;

-- Add indexes for efficient queries
CREATE INDEX IF NOT EXISTS idx_sessions_channel ON sessions_v2(channel);
CREATE INDEX IF NOT EXISTS idx_sessions_chat_id ON sessions_v2(chat_id);
CREATE INDEX IF NOT EXISTS idx_events_channel ON session_events(channel);
CREATE INDEX IF NOT EXISTS idx_events_chat_id ON session_events(chat_id);
