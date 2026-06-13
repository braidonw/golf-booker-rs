-- Email for login accounts (for password reset + booking notifications).
ALTER TABLE users ADD COLUMN email TEXT;

-- One-time, expiring password-reset tokens. Only the SHA-256 hash is stored;
-- the raw token lives only in the emailed link.
CREATE TABLE IF NOT EXISTS password_reset_tokens (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    expires_at TEXT NOT NULL,   -- RFC3339 UTC
    used_at    TEXT,            -- set when redeemed
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_reset_token_hash ON password_reset_tokens(token_hash);
