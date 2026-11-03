CREATE TABLE mixer_channels (
    station_id UUID NOT NULL REFERENCES stations(id) ON DELETE CASCADE,
    channel_id UUID NOT NULL,
    fader DOUBLE PRECISION NOT NULL DEFAULT 1.0 CHECK (fader >= 0.0 AND fader <= 2.0),
    is_on BOOLEAN NOT NULL DEFAULT TRUE,
    cue_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    muted BOOLEAN NOT NULL DEFAULT FALSE,
    revision BIGINT NOT NULL DEFAULT 1,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (station_id, channel_id)
);
