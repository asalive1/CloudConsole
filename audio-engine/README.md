# Audio engine

The Phase 3 GStreamer service owns the Program audio pipeline and exposes its internal API on port `8001`. The backend proxies station-authenticated requests through `/api/audio/*`; the engine is not published directly to the host.

## Input sources

`POST /api/audio/sources` accepts:

```json
{
  "name": "Automation",
  "source_type": "automation",
  "transport": {
    "type": "rtp",
    "address": "0.0.0.0",
    "port": 5005,
    "encoding": "l24",
    "channels": 2,
    "payload_type": 96,
    "jitter_buffer_ms": 100
  }
}
```

RTP input supports L16 and L24, with L24 stereo as the default. SRT input supports caller or listener mode and decodes audio from MPEG-TS; it defaults to listener mode for Automation/AUX use. ALSA inputs are configurable by device and accept 48 kHz, mono/stereo, 16-/24-bit PCM. The test transport uses a GStreamer tone source and is intended for integration checks.

At most one Automation and five AUX source pipelines may be active. A source may be removed with `DELETE /api/audio/sources/{id}` or recovered with `POST /api/audio/sources/{id}/recover`.

## Program outputs

`POST /api/audio/outputs` attaches a Program bus output. ALSA output supports 48 kHz, mono/stereo, 16-/24-bit PCM. RTP output supports L16/L24. SRT output supports caller/listener mode and AAC in MPEG-TS; caller mode is the default for Program output. SRT URI passphrases are never returned by list endpoints or written to application logs.

The Program bus has a null sink in addition to configured outputs so the pipeline can initialize without a physical sound device. `GET /api/audio/metrics` reports GStreamer pipeline state, source/output counts, levels, RTP jitter-buffer statistics, and pipeline latency when available.

## Program delay

End gradually drains the delay and leaves it off until Build is pressed. Changing the configured delay while it is off or ending does not restart it. Dump clears the buffered audio and automatically rebuilds the delay; End can cancel that rebuild.

## Validation notes

Linux container tests use `audiotestsrc` and `fakesink`. SRT AAC/MPEG-TS input and output were verified container-to-container. Compose publishes UDP `7010-7019` by default for RTP inputs and SRT listener inputs (override with `ENGINE_UDP_PORTS`); configure inputs on ports within the station's range. External reachability still depends on host firewall and network configuration. Real ALSA hardware playback requires a Linux host/device. WebRTC signaling and participant connections are Phase 4.
