-- Initial schema for golf-booker.

-- Application login accounts (the family members who use the app).
CREATE TABLE IF NOT EXISTS users (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    username   TEXT NOT NULL UNIQUE,
    password   TEXT NOT NULL,            -- argon2 hash
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Golf clubs we can book against. One operator, so club credentials are stored
-- centrally. NOTE: `password` is the club login, kept in PLAINTEXT for now
-- (it has to be replayed to the club on login). Anyone who can read the DB can
-- read these — treat the whole row as a secret. Encrypt-at-rest is a follow-up.
CREATE TABLE IF NOT EXISTS clubs (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT NOT NULL,
    base_url   TEXT NOT NULL,
    username   TEXT NOT NULL,
    password   TEXT NOT NULL,
    member_id  TEXT NOT NULL,
    -- IANA timezone the club books in, e.g. 'Australia/Sydney'. Slots open at
    -- local time, so scheduling is interpreted/displayed in this zone.
    timezone   TEXT NOT NULL DEFAULT 'Australia/Sydney',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Scheduled booking jobs: "book group X at club Y the moment the sheet opens".
CREATE TABLE IF NOT EXISTS scheduled_jobs (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id        INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    club_id        INTEGER REFERENCES clubs(id) ON DELETE SET NULL,
    event_id       INTEGER,
    job_type       TEXT NOT NULL DEFAULT 'booking',
    scheduled_time TEXT NOT NULL,        -- RFC3339 / ISO-8601, stored as UTC
    status         TEXT NOT NULL DEFAULT 'pending', -- pending|running|completed|failed|cancelled
    job_data       TEXT NOT NULL,        -- JSON booking parameters
    attempts       INTEGER NOT NULL DEFAULT 0,
    max_attempts   INTEGER NOT NULL DEFAULT 3,
    last_error     TEXT,
    created_at     TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at     TEXT NOT NULL DEFAULT (datetime('now')),
    completed_at   TEXT
);

CREATE INDEX IF NOT EXISTS idx_jobs_pending
    ON scheduled_jobs(status, scheduled_time)
    WHERE status = 'pending';

CREATE INDEX IF NOT EXISTS idx_jobs_user
    ON scheduled_jobs(user_id, status);

CREATE TRIGGER IF NOT EXISTS trg_jobs_updated_at
AFTER UPDATE ON scheduled_jobs
FOR EACH ROW
BEGIN
    UPDATE scheduled_jobs SET updated_at = datetime('now') WHERE id = NEW.id;
END;
