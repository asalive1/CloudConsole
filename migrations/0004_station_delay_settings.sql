CREATE TABLE station_delay_settings (
    station_id UUID PRIMARY KEY REFERENCES stations(id) ON DELETE CASCADE,
    delay_seconds SMALLINT NOT NULL DEFAULT 5 CHECK (delay_seconds BETWEEN 5 AND 60),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
