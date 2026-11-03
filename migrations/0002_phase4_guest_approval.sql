ALTER TABLE sessions
    ADD COLUMN approval_expires_at TIMESTAMPTZ;

UPDATE sessions
SET approval_expires_at = LEAST(expires_at, created_at + INTERVAL '20 minutes')
WHERE status = 'pending';
