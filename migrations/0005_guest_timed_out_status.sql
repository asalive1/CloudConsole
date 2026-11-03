ALTER TABLE sessions DROP CONSTRAINT sessions_status_check;

ALTER TABLE sessions
    ADD CONSTRAINT sessions_status_check
    CHECK (status IN ('pending', 'approved', 'connected', 'rejected', 'disconnected', 'expired', 'timed_out'));
