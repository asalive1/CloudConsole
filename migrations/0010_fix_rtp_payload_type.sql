-- Fix invalid RTP payload_type values that exceed the valid range (0-127)
-- RTP payload types must be in the range 0-127 per RFC 3550
-- This migration corrects any output configurations with invalid payload_type values
-- by resetting them to the default value of 100 (a standard dynamic RTP payload type for audio)

UPDATE outputs
SET config = jsonb_set(
    config,
    '{payload_type}',
    '100'::jsonb,
    true
)
WHERE output_type = 'rtp'
  AND config ? 'payload_type'
  AND (config->>'payload_type')::int > 127;
