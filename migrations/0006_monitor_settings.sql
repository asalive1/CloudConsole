CREATE TABLE station_monitor_settings (
    station_id UUID PRIMARY KEY REFERENCES stations(id) ON DELETE CASCADE,
    local_source TEXT NOT NULL DEFAULT 'program_mix' CHECK (local_source IN ('cue', 'program_mix', 'mix_minus', 'input')),
    local_input_id UUID,
    local_volume DOUBLE PRECISION NOT NULL DEFAULT 1.0 CHECK (local_volume >= 0.0 AND local_volume <= 1.0),
    local_muted BOOLEAN NOT NULL DEFAULT FALSE,
    guest_source TEXT NOT NULL DEFAULT 'program_mix' CHECK (guest_source IN ('cue', 'program_mix', 'mix_minus', 'input')),
    guest_input_id UUID,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
