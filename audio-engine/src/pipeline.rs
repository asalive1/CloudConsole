use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use gstreamer as gst;
use gstreamer::prelude::*;
use serde::{Deserialize, Serialize};
use url::Host;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("{0}")]
    InvalidRequest(String),
    #[error("resource not found")]
    NotFound,
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Pipeline(String),
}

impl From<gst::glib::Error> for EngineError {
    fn from(error: gst::glib::Error) -> Self {
        Self::Pipeline(error.to_string())
    }
}

impl From<gst::glib::BoolError> for EngineError {
    fn from(error: gst::glib::BoolError) -> Self {
        Self::Pipeline(error.to_string())
    }
}

impl From<gst::StateChangeError> for EngineError {
    fn from(error: gst::StateChangeError) -> Self {
        Self::Pipeline(error.to_string())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceType {
    Automation,
    Aux,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SrtMode {
    Caller,
    #[default]
    Listener,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RtpEncoding {
    L16,
    #[default]
    L24,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputTransport {
    Test {
        #[serde(default = "default_test_frequency")]
        frequency: u32,
    },
    Rtp {
        #[serde(default = "default_rtp_address")]
        address: String,
        #[serde(default = "default_rtp_port")]
        port: u16,
        #[serde(default)]
        encoding: RtpEncoding,
        #[serde(default = "default_channels")]
        channels: u8,
        #[serde(default = "default_payload_type")]
        payload_type: u8,
        #[serde(default = "default_jitter_buffer_ms")]
        jitter_buffer_ms: u32,
        #[serde(default, alias = "multicast_interface")]
        interface: Option<String>,
    },
    Srt {
        #[serde(default = "default_srt_host")]
        host: String,
        port: u16,
        #[serde(default)]
        mode: SrtMode,
        #[serde(default = "default_srt_latency_ms")]
        latency_ms: u32,
        #[serde(default)]
        stream_id: Option<String>,
        #[serde(default)]
        passphrase: Option<String>,
        #[serde(default)]
        interface: Option<String>,
    },
    Alsa {
        device: String,
        #[serde(default = "default_sample_rate")]
        sample_rate: u32,
        #[serde(default = "default_channels")]
        channels: u8,
        #[serde(default = "default_bit_depth")]
        bit_depth: u8,
    },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputTransport {
    Alsa {
        device: String,
        #[serde(default = "default_sample_rate")]
        sample_rate: u32,
        #[serde(default = "default_channels")]
        channels: u8,
        #[serde(default = "default_bit_depth")]
        bit_depth: u8,
    },
    Rtp {
        host: String,
        port: u16,
        #[serde(default)]
        encoding: RtpEncoding,
        #[serde(default = "default_channels")]
        channels: u8,
        #[serde(default = "default_payload_type")]
        payload_type: u8,
        #[serde(default)]
        interface: Option<String>,
    },
    Srt {
        host: String,
        port: u16,
        #[serde(default = "default_srt_output_mode")]
        mode: SrtMode,
        #[serde(default = "default_srt_latency_ms")]
        latency_ms: u32,
        #[serde(default)]
        stream_id: Option<String>,
        #[serde(default)]
        passphrase: Option<String>,
        #[serde(default)]
        interface: Option<String>,
        #[serde(default)]
        pbkeylen: Option<u8>,
        #[serde(default = "default_aac_bitrate")]
        bitrate: u32,
    },
}

#[derive(Serialize)]
pub struct SourceSummary {
    pub id: Uuid,
    pub name: String,
    pub source_type: SourceType,
    pub transport: &'static str,
    pub state: &'static str,
}

#[derive(Serialize)]
pub struct OutputSummary {
    pub id: Uuid,
    pub name: String,
    pub transport: &'static str,
    pub state: &'static str,
}

#[derive(Clone, Serialize)]
pub struct SourceMetrics {
    pub id: Uuid,
    pub state: &'static str,
    pub peak_dbfs: Option<Vec<f64>>,
    pub jitter_ms: Option<f64>,
    pub packets_lost: Option<u64>,
    pub configured_latency_ms: Option<u32>,
}

#[derive(Serialize)]
pub struct AudioMetrics {
    pub pipeline_state: &'static str,
    pub source_count: usize,
    pub output_count: usize,
    pub program_peak_dbfs: Option<Vec<f64>>,
    pub pipeline_latency_ms: Option<f64>,
    pub sources: Vec<SourceMetrics>,
    pub last_pipeline_error: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DelayMode {
    Normal,
    Rebuilding,
    EndingDelay,
    Live,
}

#[derive(Clone, Debug, Serialize)]
pub struct DelayStatus {
    pub configured_delay_seconds: u32,
    pub buffered_delay_seconds: f64,
    pub recovery_percent: f64,
    pub state: DelayMode,
}

#[derive(Clone, Debug)]
struct DelayRuntime {
    configured_delay_seconds: u32,
    target_delay_seconds: u32,
    buffered_delay_seconds: f64,
    state: DelayMode,
    last_micro_skip: Instant,
    // While set, buffers leaving the delay queue are discarded.
    dumping: Arc<AtomicBool>,
    // Set after a dump so the next buffer is flagged discontinuous and the pitch element re-times its output.
    resync: Arc<AtomicBool>,
}

#[derive(Default)]
struct LiveMetrics {
    source_peaks: HashMap<Uuid, Vec<f64>>,
    program_peaks: Option<Vec<f64>>,
    last_pipeline_error: Option<String>,
}

struct ActiveSource {
    id: Uuid,
    name: String,
    source_type: SourceType,
    transport: &'static str,
    input: InputTransport,
    bin: gst::Bin,
    route: SourceRoute,
    jitter_buffer: Option<gst::Element>,
}

// A source fans out to the program mixer and to the sources-only bridge mixer.
#[derive(Default)]
struct SourceRoute {
    source_pad: Option<gst::Pad>,
    elements: Vec<gst::Element>,
    mixer_pad: Option<gst::Pad>,
    bridge_pad: Option<gst::Pad>,
    mixer_queue_src: Option<gst::Pad>,
    bridge_queue_src: Option<gst::Pad>,
    cue_pad: Option<gst::Pad>,
    cue_queue_src: Option<gst::Pad>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BridgeBus {
    #[default]
    Program,
    Cue,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BridgeConfig {
    #[serde(skip)]
    pub bus: BridgeBus,
    pub listen_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
    #[serde(default = "default_bridge_payload_type")]
    pub payload_type: u8,
}

fn default_bridge_payload_type() -> u8 {
    100
}

#[derive(Clone, Debug, Serialize)]
pub struct BridgeStatus {
    pub active: bool,
    pub listen_port: Option<u16>,
    pub remote: Option<String>,
    pub received_packets: u64,
    pub sent_packets: u64,
}

struct ActiveBridge {
    config: BridgeConfig,
    rx_bin: gst::Bin,
    rx_mixer_pad: Option<gst::Pad>,
    tx_bin: gst::Bin,
    tx_tee_pad: gst::Pad,
    received: Arc<AtomicU64>,
    sent: Arc<AtomicU64>,
}

struct ActiveOutput {
    id: Uuid,
    name: String,
    transport: &'static str,
    bin: gst::Bin,
    tee_pad: gst::Pad,
}

pub struct AudioEngine {
    pipeline: gst::Pipeline,
    mixer: gst::Element,
    delay_queue: gst::Element,
    delay_pitch: gst::Element,
    delay_state: Arc<Mutex<DelayRuntime>>,
    program_tee: gst::Element,
    bridge_mixer: gst::Element,
    bridge_tee: gst::Element,
    bridge: Option<ActiveBridge>,
    cue_mixer: gst::Element,
    cue_tee: gst::Element,
    cue_bridge: Option<ActiveBridge>,
    sources: HashMap<Uuid, ActiveSource>,
    outputs: HashMap<Uuid, ActiveOutput>,
    metrics: Arc<Mutex<LiveMetrics>>,
}

impl AudioEngine {
    pub fn new() -> Result<Self, EngineError> {
        let pipeline = gst::Pipeline::new();
        let configured_delay_seconds = 5;
        let dumping = Arc::new(AtomicBool::new(false));
        let resync = Arc::new(AtomicBool::new(false));
        let delay_state = Arc::new(Mutex::new(DelayRuntime {
            configured_delay_seconds,
            target_delay_seconds: configured_delay_seconds,
            buffered_delay_seconds: 0.0,
            state: DelayMode::Rebuilding,
            last_micro_skip: Instant::now(),
            dumping: Arc::clone(&dumping),
            resync: Arc::clone(&resync),
        }));
        let mixer = gst::ElementFactory::make("audiomixer")
            .name("program_mixer")
            .build()?;
        let convert = gst::ElementFactory::make("audioconvert")
            .name("program_convert")
            .build()?;
        let resample = gst::ElementFactory::make("audioresample")
            .name("program_resample")
            .build()?;
        let level = gst::ElementFactory::make("level")
            .name("level-program")
            .property("message", true)
            .build()?;
        let delay_queue = gst::ElementFactory::make("queue")
            .name("delay_buffer")
            .property("max-size-buffers", 0u32)
            .property("max-size-bytes", 0u32)
            .property("max-size-time", 65_000_000_000u64)
            .property(
                "min-threshold-time",
                u64::from(configured_delay_seconds) * 1_000_000_000,
            )
            .build()?;
        if let Some(queue_src) = delay_queue.static_pad("src") {
            queue_src.add_probe(
                gst::PadProbeType::BUFFER | gst::PadProbeType::BUFFER_LIST,
                move |_, info| {
                    if dumping.load(Ordering::Relaxed) {
                        return gst::PadProbeReturn::Drop;
                    }
                    if resync.swap(false, Ordering::Relaxed) {
                        if let Some(buffer) = info.buffer_mut() {
                            buffer.make_mut().set_flags(gst::BufferFlags::DISCONT);
                        }
                    }
                    gst::PadProbeReturn::Ok
                },
            );
        }
        let delay_convert = gst::ElementFactory::make("audioconvert")
            .name("delay_convert")
            .build()?;
        let delay_caps = gst::ElementFactory::make("capsfilter")
            .name("delay_caps")
            .property(
                "caps",
                gst::Caps::builder("audio/x-raw")
                    .field("format", "F32LE")
                    .field("rate", 48_000i32)
                    .field("channels", 2i32)
                    .build(),
            )
            .build()?;
        let delay_pitch = gst::ElementFactory::make("pitch")
            .name("delay_tempo")
            .property("tempo", 1.0f32)
            .property("pitch", 1.0f32)
            .build()?;
        let output_convert = gst::ElementFactory::make("audioconvert")
            .name("program_output_convert")
            .build()?;
        let program_tee = gst::ElementFactory::make("tee")
            .name("program_tee")
            .build()?;
        let output_queue = gst::ElementFactory::make("queue")
            .name("local_null_queue")
            .property("max-size-buffers", 5u32)
            .property("max-size-bytes", 0u32)
            .property("max-size-time", 0u64)
            .build()?;
        let null_sink = gst::ElementFactory::make("fakesink")
            .name("local_null_sink")
            .property("sync", true)
            .property("async", false)
            .build()?;

        pipeline.add_many([
            &mixer,
            &convert,
            &resample,
            &level,
            &delay_convert,
            &delay_caps,
            &delay_queue,
            &delay_pitch,
            &output_convert,
            &program_tee,
            &output_queue,
            &null_sink,
        ])?;
        gst::Element::link_many([
            &mixer,
            &convert,
            &resample,
            &level,
            &delay_convert,
            &delay_caps,
            &delay_queue,
            &delay_pitch,
            &output_convert,
            &program_tee,
        ])?;
        output_queue.link(&null_sink)?;
        link_request_pad(&program_tee, "src_%u", &output_queue, "sink")?;

        let bridge_mixer = gst::ElementFactory::make("audiomixer")
            .name("bridge_mixer")
            .build()?;
        let bridge_silence = gst::ElementFactory::make("audiotestsrc")
            .name("bridge_silence")
            .property_from_str("wave", "silence")
            .property("is-live", true)
            .build()?;
        let bridge_silence_caps = gst::ElementFactory::make("capsfilter")
            .name("bridge_silence_caps")
            .property("caps", standard_caps())
            .build()?;
        let bridge_convert = gst::ElementFactory::make("audioconvert")
            .name("bridge_convert")
            .build()?;
        let bridge_resample = gst::ElementFactory::make("audioresample")
            .name("bridge_resample")
            .build()?;
        let bridge_caps = gst::ElementFactory::make("capsfilter")
            .name("bridge_caps")
            .property("caps", standard_caps())
            .build()?;
        let bridge_tee = gst::ElementFactory::make("tee")
            .name("bridge_tee")
            .build()?;
        let bridge_null_queue = gst::ElementFactory::make("queue")
            .name("bridge_null_queue")
            .property("max-size-buffers", 5u32)
            .property("max-size-bytes", 0u32)
            .property("max-size-time", 0u64)
            .build()?;
        let bridge_null_sink = gst::ElementFactory::make("fakesink")
            .name("bridge_null_sink")
            .property("sync", true)
            .property("async", false)
            .build()?;
        pipeline.add_many([
            &bridge_mixer,
            &bridge_silence,
            &bridge_silence_caps,
            &bridge_convert,
            &bridge_resample,
            &bridge_caps,
            &bridge_tee,
            &bridge_null_queue,
            &bridge_null_sink,
        ])?;
        gst::Element::link_many([&bridge_silence, &bridge_silence_caps])?;
        let silence_pad = bridge_mixer
            .request_pad_simple("sink_%u")
            .ok_or_else(|| EngineError::Pipeline("bridge mixer rejected a pad".to_owned()))?;
        bridge_silence_caps
            .static_pad("src")
            .ok_or_else(|| EngineError::Pipeline("bridge silence has no pad".to_owned()))?
            .link(&silence_pad)
            .map_err(|error| EngineError::Pipeline(error.to_string()))?;
        gst::Element::link_many([
            &bridge_mixer,
            &bridge_convert,
            &bridge_resample,
            &bridge_caps,
            &bridge_tee,
        ])?;
        bridge_null_queue.link(&bridge_null_sink)?;
        link_request_pad(&bridge_tee, "src_%u", &bridge_null_queue, "sink")?;
        let (cue_mixer, cue_tee) = build_bus_mixer(&pipeline, "cue")?;
        let delay_src = delay_queue
            .static_pad("src")
            .ok_or_else(|| EngineError::Pipeline("delay queue has no source pad".to_owned()))?;
        let ending_state = Arc::clone(&delay_state);
        let delay_probe = delay_src.add_probe(gst::PadProbeType::BUFFER, move |_, info| {
            let Some(buffer) = info.buffer() else {
                return gst::PadProbeReturn::Ok;
            };
            let Ok(mut delay) = ending_state.lock() else {
                tracing::error!("delay state lock is poisoned");
                return gst::PadProbeReturn::Ok;
            };
            if delay.state != DelayMode::EndingDelay {
                return gst::PadProbeReturn::Ok;
            }
            let is_silent = match buffer.map_readable() {
                Ok(mapped) => is_silent_pcm(mapped.as_slice()),
                Err(error) => {
                    tracing::warn!(%error, "could not inspect delay audio buffer");
                    false
                }
            };
            if is_silent {
                return gst::PadProbeReturn::Drop;
            }
            if delay.last_micro_skip.elapsed() >= Duration::from_millis(500) {
                delay.last_micro_skip = Instant::now();
                return gst::PadProbeReturn::Drop;
            }
            gst::PadProbeReturn::Ok
        });
        if delay_probe.is_none() {
            return Err(EngineError::Pipeline(
                "could not register delay audio probe".to_owned(),
            ));
        }
        pipeline.set_state(gst::State::Playing)?;

        let metrics = Arc::new(Mutex::new(LiveMetrics::default()));
        start_bus_monitor(&pipeline, Arc::clone(&metrics));
        start_delay_monitor(
            delay_queue.clone(),
            delay_pitch.clone(),
            Arc::clone(&delay_state),
        );

        Ok(Self {
            pipeline,
            mixer,
            delay_queue,
            delay_pitch,
            delay_state,
            program_tee,
            bridge_mixer,
            bridge_tee,
            bridge: None,
            cue_mixer,
            cue_tee,
            cue_bridge: None,
            sources: HashMap::new(),
            outputs: HashMap::new(),
            metrics,
        })
    }

    pub fn add_source_with_id(
        &mut self,
        id: Uuid,
        name: String,
        source_type: SourceType,
        input: InputTransport,
    ) -> Result<SourceSummary, EngineError> {
        let name = validate_name(name)?;
        let count = self
            .sources
            .values()
            .filter(|source| source.source_type == source_type)
            .count();
        match source_type {
            SourceType::Automation if count >= 1 => {
                return Err(EngineError::Conflict(
                    "a station supports one Automation input".to_owned(),
                ));
            }
            SourceType::Aux if count >= 5 => {
                return Err(EngineError::Conflict(
                    "a station supports up to five AUX inputs".to_owned(),
                ));
            }
            _ => {}
        }

        validate_input(&input)?;
        let transport = input.transport_name();
        let bin = match &input {
            InputTransport::Srt {
                host,
                port,
                mode,
                latency_ms,
                stream_id,
                passphrase,
                interface,
            } => {
                let (host, local_address) = srt_binding(host, *mode, interface.as_deref())?;
                let uri = srt_uri(
                    &host,
                    *port,
                    *mode,
                    *latency_ms,
                    stream_id.as_deref(),
                    passphrase.as_deref(),
                    None,
                )?;
                build_srt_input_bin(id, &uri, local_address.as_deref())?
            }
            _ => {
                let description = input.gst_description(id)?;
                gst::parse::bin_from_description(&description, true).map_err(|error| {
                    EngineError::Pipeline(sanitize_pipeline_error(error.to_string()))
                })?
            }
        };
        if matches!(&input, InputTransport::Srt { .. }) {
            bin.set_property("async-handling", true);
        }
        self.pipeline.add(&bin)?;
        let source_pad = bin
            .static_pad("src")
            .ok_or_else(|| EngineError::Pipeline("input pipeline has no output pad".to_owned()))?;
        let mut route = SourceRoute {
            source_pad: Some(source_pad.clone()),
            ..SourceRoute::default()
        };
        if let Err(error) = self.build_route(&source_pad, &mut route) {
            self.teardown_route(&route);
            let _ = self.pipeline.remove(&bin);
            return Err(error);
        }
        if let Err(error) = bin.sync_state_with_parent() {
            self.teardown_route(&route);
            let _ = self.pipeline.remove(&bin);
            return Err(EngineError::Pipeline(error.to_string()));
        }
        let jitter_buffer = bin.by_name(&format!("jitter-{}", id.simple()));
        self.sources.insert(
            id,
            ActiveSource {
                id,
                name,
                source_type,
                transport,
                input,
                bin,
                route,
                jitter_buffer,
            },
        );
        Ok(self.source_summary(id).expect("source was just inserted"))
    }

    pub fn remove_source(&mut self, id: Uuid) -> Result<(), EngineError> {
        let source = self.sources.remove(&id).ok_or(EngineError::NotFound)?;
        source.bin.set_state(gst::State::Null)?;
        self.teardown_route(&source.route);
        self.pipeline.remove(&source.bin)?;
        if let Ok(mut metrics) = self.metrics.lock() {
            metrics.source_peaks.remove(&id);
        }
        Ok(())
    }

    fn build_route(
        &self,
        source_pad: &gst::Pad,
        route: &mut SourceRoute,
    ) -> Result<(), EngineError> {
        let fail = |message: &str| EngineError::Pipeline(message.to_owned());
        let tee = gst::ElementFactory::make("tee").build()?;
        let mixer_queue = gst::ElementFactory::make("queue").build()?;
        let bridge_queue = gst::ElementFactory::make("queue")
            .property_from_str("leaky", "downstream")
            .property("max-size-buffers", 0u32)
            .property("max-size-bytes", 0u32)
            .property("max-size-time", 300_000_000u64)
            .build()?;
        let cue_queue = gst::ElementFactory::make("queue")
            .property_from_str("leaky", "downstream")
            .property("max-size-buffers", 0u32)
            .property("max-size-bytes", 0u32)
            .property("max-size-time", 300_000_000u64)
            .build()?;
        route.elements = vec![
            tee.clone(),
            mixer_queue.clone(),
            bridge_queue.clone(),
            cue_queue.clone(),
        ];
        self.pipeline
            .add_many([&tee, &mixer_queue, &bridge_queue, &cue_queue])?;
        source_pad
            .link(
                &tee.static_pad("sink")
                    .ok_or_else(|| fail("tee has no sink"))?,
            )
            .map_err(|error| EngineError::Pipeline(error.to_string()))?;
        link_request_pad(&tee, "src_%u", &mixer_queue, "sink")?;
        link_request_pad(&tee, "src_%u", &bridge_queue, "sink")?;
        link_request_pad(&tee, "src_%u", &cue_queue, "sink")?;
        let cue_pad = self
            .cue_mixer
            .request_pad_simple("sink_%u")
            .ok_or_else(|| fail("cue mixer rejected an input pad"))?;
        cue_pad.set_property("mute", true);
        route.cue_pad = Some(cue_pad.clone());
        let cue_queue_src = cue_queue
            .static_pad("src")
            .ok_or_else(|| fail("queue has no source pad"))?;
        cue_queue_src
            .link(&cue_pad)
            .map_err(|error| EngineError::Pipeline(error.to_string()))?;
        route.cue_queue_src = Some(cue_queue_src);
        let mixer_pad = self
            .mixer
            .request_pad_simple("sink_%u")
            .ok_or_else(|| fail("mixer rejected an input pad"))?;
        route.mixer_pad = Some(mixer_pad.clone());
        let bridge_pad = self
            .bridge_mixer
            .request_pad_simple("sink_%u")
            .ok_or_else(|| fail("bridge mixer rejected an input pad"))?;
        route.bridge_pad = Some(bridge_pad.clone());
        let mixer_queue_src = mixer_queue
            .static_pad("src")
            .ok_or_else(|| fail("queue has no source pad"))?;
        mixer_queue_src
            .link(&mixer_pad)
            .map_err(|error| EngineError::Pipeline(error.to_string()))?;
        route.mixer_queue_src = Some(mixer_queue_src);
        let bridge_queue_src = bridge_queue
            .static_pad("src")
            .ok_or_else(|| fail("queue has no source pad"))?;
        bridge_queue_src
            .link(&bridge_pad)
            .map_err(|error| EngineError::Pipeline(error.to_string()))?;
        route.bridge_queue_src = Some(bridge_queue_src);
        for element in &route.elements {
            element.sync_state_with_parent()?;
        }
        Ok(())
    }

    fn teardown_route(&self, route: &SourceRoute) {
        for element in &route.elements {
            let _ = element.set_state(gst::State::Null);
        }
        if let (Some(source_pad), Some(tee)) = (&route.source_pad, route.elements.first()) {
            if let Some(sink) = tee.static_pad("sink") {
                let _ = source_pad.unlink(&sink);
            }
        }
        if let (Some(src), Some(pad)) = (&route.mixer_queue_src, &route.mixer_pad) {
            let _ = src.unlink(pad);
        }
        if let (Some(src), Some(pad)) = (&route.bridge_queue_src, &route.bridge_pad) {
            let _ = src.unlink(pad);
        }
        if let (Some(src), Some(pad)) = (&route.cue_queue_src, &route.cue_pad) {
            let _ = src.unlink(pad);
        }
        if let Some(pad) = &route.mixer_pad {
            self.mixer.release_request_pad(pad);
        }
        if let Some(pad) = &route.cue_pad {
            self.cue_mixer.release_request_pad(pad);
        }
        if let Some(pad) = &route.bridge_pad {
            self.bridge_mixer.release_request_pad(pad);
        }
        for element in &route.elements {
            let _ = self.pipeline.remove(element);
        }
    }

    pub fn start_bridge(&mut self, config: BridgeConfig) -> Result<BridgeStatus, EngineError> {
        if config.remote_host.parse::<IpAddr>().is_err() {
            return Err(EngineError::InvalidRequest(
                "remote_host must be an IP address".to_owned(),
            ));
        }
        if config.listen_port < 1024 || config.remote_port < 1024 {
            return Err(EngineError::InvalidRequest(
                "bridge ports must be 1024 or higher".to_owned(),
            ));
        }
        if !(96..=127).contains(&config.payload_type) {
            return Err(EngineError::InvalidRequest(
                "payload_type must be between 96 and 127".to_owned(),
            ));
        }
        self.stop_bridge_bus(config.bus);
        let received = Arc::new(AtomicU64::new(0));
        let sent = Arc::new(AtomicU64::new(0));

        // The cue bus is send-only, so its receiver only holds the socket open.
        let rx_tail = if config.bus == BridgeBus::Program {
            "rtpjitterbuffer latency=60 do-lost=true ! rtpopusdepay ! opusdec plc=true ! audioconvert ! audioresample ! audio/x-raw,format=F32LE,layout=interleaved,rate=48000,channels=2 ! queue"
        } else {
            "fakesink sync=false async=false"
        };
        let rx_bin = gst::parse::bin_from_description(
            &format!(
                "udpsrc name=bridge_udpsrc port={} caps=\"application/x-rtp,media=(string)audio,clock-rate=(int)48000,encoding-name=(string)OPUS,payload=(int){}\" ! {rx_tail}",
                config.listen_port, config.payload_type
            ),
            true,
        )
        .map_err(|error| EngineError::Pipeline(sanitize_pipeline_error(error.to_string())))?;
        let tx_bin = gst::parse::bin_from_description(
            &format!(
                "queue leaky=downstream max-size-buffers=0 max-size-bytes=0 max-size-time=200000000 ! audioconvert ! audioresample ! audio/x-raw,rate=48000,channels=2 ! opusenc bitrate=96000 frame-size=20 ! rtpopuspay pt={} ! udpsink name=bridge_udpsink host={} port={} sync=false async=false",
                config.payload_type, config.remote_host, config.remote_port
            ),
            true,
        )
        .map_err(|error| EngineError::Pipeline(sanitize_pipeline_error(error.to_string())))?;

        count_buffers(&rx_bin, "bridge_udpsrc", "src", &received);
        count_buffers(&tx_bin, "bridge_udpsink", "sink", &sent);

        // Janus only accepts plain-RTP audio from the address it sends to, so the
        // sender reuses the receiver's socket and transmits from the listen port.
        rx_bin.set_state(gst::State::Ready).map_err(|_| {
            EngineError::Pipeline("bridge receiver could not open its socket".to_owned())
        })?;
        if let (Some(src), Some(sink)) = (
            rx_bin.by_name("bridge_udpsrc"),
            tx_bin.by_name("bridge_udpsink"),
        ) {
            let socket = src.property_value("used-socket");
            sink.set_property_from_value("socket", &socket);
            sink.set_property("close-socket", false);
        }

        self.pipeline.add_many([&rx_bin, &tx_bin])?;
        let rx_mixer_pad =
            if config.bus == BridgeBus::Program {
                Some(self.mixer.request_pad_simple("sink_%u").ok_or_else(|| {
                    EngineError::Pipeline("mixer rejected the bridge pad".to_owned())
                })?)
            } else {
                None
            };
        let tx_pad = tx_bin
            .static_pad("sink")
            .ok_or_else(|| EngineError::Pipeline("bridge sender has no pad".to_owned()))?;
        let tee = match config.bus {
            BridgeBus::Program => &self.bridge_tee,
            BridgeBus::Cue => &self.cue_tee,
        };
        let tx_tee_pad = tee
            .request_pad_simple("src_%u")
            .ok_or_else(|| EngineError::Pipeline("bridge router rejected a pad".to_owned()))?;
        let rx_link = match (&rx_mixer_pad, rx_bin.static_pad("src")) {
            (Some(mixer_pad), Some(rx_pad)) => rx_pad.link(mixer_pad).map(|_| ()),
            _ => Ok(()),
        };
        let linked = rx_link.and_then(|_| tx_tee_pad.link(&tx_pad).map(|_| ()));
        let bus = config.bus;
        let bridge = ActiveBridge {
            config,
            rx_bin,
            rx_mixer_pad,
            tx_bin,
            tx_tee_pad,
            received,
            sent,
        };
        if let Err(error) = linked {
            self.release_bridge(&bridge);
            return Err(EngineError::Pipeline(error.to_string()));
        }
        let started = bridge
            .rx_bin
            .sync_state_with_parent()
            .and_then(|_| bridge.tx_bin.sync_state_with_parent());
        if let Err(error) = started {
            self.release_bridge(&bridge);
            return Err(EngineError::Pipeline(error.to_string()));
        }
        match bus {
            BridgeBus::Program => self.bridge = Some(bridge),
            BridgeBus::Cue => self.cue_bridge = Some(bridge),
        }
        Ok(self.bridge_status(bus))
    }

    pub fn stop_bridge(&mut self) {
        self.stop_bridge_bus(BridgeBus::Program);
    }

    pub fn stop_cue_bridge(&mut self) {
        self.stop_bridge_bus(BridgeBus::Cue);
    }

    fn stop_bridge_bus(&mut self, bus: BridgeBus) {
        let slot = match bus {
            BridgeBus::Program => self.bridge.take(),
            BridgeBus::Cue => self.cue_bridge.take(),
        };
        if let Some(bridge) = slot {
            self.release_bridge(&bridge);
        }
    }

    fn release_bridge(&self, bridge: &ActiveBridge) {
        let _ = bridge.rx_bin.set_state(gst::State::Null);
        let _ = bridge.tx_bin.set_state(gst::State::Null);
        if let (Some(rx_pad), Some(mixer_pad)) =
            (bridge.rx_bin.static_pad("src"), &bridge.rx_mixer_pad)
        {
            let _ = rx_pad.unlink(mixer_pad);
        }
        if let Some(pad) = bridge.tx_bin.static_pad("sink") {
            let _ = bridge.tx_tee_pad.unlink(&pad);
        }
        if let Some(mixer_pad) = &bridge.rx_mixer_pad {
            self.mixer.release_request_pad(mixer_pad);
        }
        match bridge.config.bus {
            BridgeBus::Program => self.bridge_tee.release_request_pad(&bridge.tx_tee_pad),
            BridgeBus::Cue => self.cue_tee.release_request_pad(&bridge.tx_tee_pad),
        }
        let _ = self.pipeline.remove(&bridge.rx_bin);
        let _ = self.pipeline.remove(&bridge.tx_bin);
    }

    pub fn bridge_status(&self, bus: BridgeBus) -> BridgeStatus {
        let active = match bus {
            BridgeBus::Program => &self.bridge,
            BridgeBus::Cue => &self.cue_bridge,
        };
        match active {
            Some(bridge) => BridgeStatus {
                active: true,
                listen_port: Some(bridge.config.listen_port),
                remote: Some(format!(
                    "{}:{}",
                    bridge.config.remote_host, bridge.config.remote_port
                )),
                received_packets: bridge.received.load(Ordering::Relaxed),
                sent_packets: bridge.sent.load(Ordering::Relaxed),
            },
            None => BridgeStatus {
                active: false,
                listen_port: None,
                remote: None,
                received_packets: 0,
                sent_packets: 0,
            },
        }
    }

    pub fn recover_source(&mut self, id: Uuid) -> Result<SourceSummary, EngineError> {
        let source = self.sources.get(&id).ok_or(EngineError::NotFound)?;
        let name = source.name.clone();
        let source_type = source.source_type;
        let input = source.input.clone();
        self.remove_source(id)?;
        self.add_source_with_id(id, name, source_type, input)?;
        Ok(self
            .source_summary(id)
            .expect("recovered source was just inserted"))
    }

    pub fn add_output_with_id(
        &mut self,
        id: Uuid,
        name: String,
        transport: OutputTransport,
    ) -> Result<OutputSummary, EngineError> {
        let name = validate_name(name)?;
        validate_output(&transport)?;
        let transport_name = transport.transport_name();
        let description = transport.gst_description()?;
        let bin = gst::parse::bin_from_description(&description, true)
            .map_err(|error| EngineError::Pipeline(sanitize_pipeline_error(error.to_string())))?;
        self.pipeline.add(&bin)?;
        let tee_pad = self
            .program_tee
            .request_pad_simple("src_%u")
            .ok_or_else(|| {
                EngineError::Pipeline("program router rejected an output pad".to_owned())
            })?;
        let sink_pad = bin
            .static_pad("sink")
            .ok_or_else(|| EngineError::Pipeline("output pipeline has no input pad".to_owned()))?;
        if let Err(error) = tee_pad.link(&sink_pad) {
            self.program_tee.release_request_pad(&tee_pad);
            let _ = self.pipeline.remove(&bin);
            return Err(EngineError::Pipeline(error.to_string()));
        }
        if let Err(error) = bin.sync_state_with_parent() {
            let _ = tee_pad.unlink(&sink_pad);
            self.program_tee.release_request_pad(&tee_pad);
            let _ = self.pipeline.remove(&bin);
            return Err(EngineError::Pipeline(error.to_string()));
        }
        self.outputs.insert(
            id,
            ActiveOutput {
                id,
                name,
                transport: transport_name,
                bin,
                tee_pad,
            },
        );
        Ok(self.output_summary(id).expect("output was just inserted"))
    }

    pub fn remove_output(&mut self, id: Uuid) -> Result<(), EngineError> {
        let output = self.outputs.remove(&id).ok_or(EngineError::NotFound)?;
        let sink_pad = output.bin.static_pad("sink");
        output.bin.set_state(gst::State::Null)?;
        if let Some(sink_pad) = sink_pad {
            let _ = output.tee_pad.unlink(&sink_pad);
        }
        self.program_tee.release_request_pad(&output.tee_pad);
        self.pipeline.remove(&output.bin)?;
        Ok(())
    }

    pub fn sources(&self) -> Vec<SourceSummary> {
        self.sources
            .keys()
            .filter_map(|id| self.source_summary(*id))
            .collect()
    }

    pub fn outputs(&self) -> Vec<OutputSummary> {
        self.outputs
            .keys()
            .filter_map(|id| self.output_summary(*id))
            .collect()
    }

    pub fn metrics(&self) -> Result<AudioMetrics, EngineError> {
        let live_metrics = self
            .metrics
            .lock()
            .map_err(|_| EngineError::Pipeline("audio metrics lock is poisoned".to_owned()))?;
        let sources = self
            .sources
            .values()
            .map(|source| {
                let (jitter_ms, packets_lost) = source
                    .jitter_buffer
                    .as_ref()
                    .map(jitter_stats)
                    .unwrap_or((None, None));
                SourceMetrics {
                    id: source.id,
                    state: state_name(source.bin.current_state()),
                    peak_dbfs: live_metrics.source_peaks.get(&source.id).cloned(),
                    jitter_ms,
                    packets_lost,
                    configured_latency_ms: source.input.configured_latency_ms(),
                }
            })
            .collect();
        Ok(AudioMetrics {
            pipeline_state: state_name(self.pipeline.current_state()),
            source_count: self.sources.len(),
            output_count: self.outputs.len(),
            program_peak_dbfs: live_metrics.program_peaks.clone(),
            pipeline_latency_ms: query_pipeline_latency(&self.pipeline),
            sources,
            last_pipeline_error: live_metrics.last_pipeline_error.clone(),
        })
    }

    pub fn delay_status(&self) -> Result<DelayStatus, EngineError> {
        let delay = self
            .delay_state
            .lock()
            .map_err(|_| EngineError::Pipeline("delay state lock is poisoned".to_owned()))?;
        Ok(delay_status(&delay))
    }

    pub fn configure_delay(&mut self, delay_seconds: u32) -> Result<DelayStatus, EngineError> {
        if !(5..=60).contains(&delay_seconds) {
            return Err(EngineError::InvalidRequest(
                "delay must be between 5 and 60 seconds".to_owned(),
            ));
        }
        let mut delay = self
            .delay_state
            .lock()
            .map_err(|_| EngineError::Pipeline("delay state lock is poisoned".to_owned()))?;
        delay.configured_delay_seconds = delay_seconds;
        if delay.state == DelayMode::Live
            || (delay.state == DelayMode::EndingDelay && delay.target_delay_seconds == 0)
        {
            return Ok(delay_status(&delay));
        }
        delay.target_delay_seconds = delay_seconds;
        delay.last_micro_skip = Instant::now();
        let target = f64::from(delay_seconds);
        if delay.buffered_delay_seconds + 0.05 < target {
            self.delay_queue.set_property("min-threshold-time", 0u64);
            self.delay_pitch.set_property("tempo", 0.95f32);
            delay.state = DelayMode::Rebuilding;
        } else if delay.buffered_delay_seconds > target + 0.05 {
            self.delay_queue.set_property("min-threshold-time", 0u64);
            self.delay_pitch.set_property("tempo", 1.05f32);
            delay.state = DelayMode::EndingDelay;
        } else {
            self.delay_queue.set_property(
                "min-threshold-time",
                u64::from(delay_seconds) * 1_000_000_000,
            );
            self.delay_pitch.set_property("tempo", 1.0f32);
            delay.state = DelayMode::Normal;
        }
        Ok(delay_status(&delay))
    }

    pub fn dump_delay(&mut self) -> Result<DelayStatus, EngineError> {
        let mut delay = self
            .delay_state
            .lock()
            .map_err(|_| EngineError::Pipeline("delay state lock is poisoned".to_owned()))?;
        if delay.state == DelayMode::Live || delay.buffered_delay_seconds <= 0.05 {
            return Err(EngineError::Conflict(
                "DUMP is unavailable because no delay buffer exists".to_owned(),
            ));
        }
        // Discarding buffers at the queue output empties the delay without flushing the pipeline;
        // the monitor thread starts the rebuild once the queue is empty.
        self.delay_queue.set_property("min-threshold-time", 0u64);
        self.delay_pitch.set_property("tempo", 1.0f32);
        delay.dumping.store(true, Ordering::Relaxed);
        delay.target_delay_seconds = delay.configured_delay_seconds;
        delay.last_micro_skip = Instant::now();
        delay.state = DelayMode::Rebuilding;
        Ok(delay_status(&delay))
    }

    pub fn build_delay(&mut self) -> Result<DelayStatus, EngineError> {
        let mut delay = self
            .delay_state
            .lock()
            .map_err(|_| EngineError::Pipeline("delay state lock is poisoned".to_owned()))?;
        let target = delay.configured_delay_seconds;
        if delay.buffered_delay_seconds + 0.05 >= f64::from(target) {
            return Err(EngineError::Conflict(
                "the delay buffer is already built".to_owned(),
            ));
        }
        self.delay_queue.set_property("min-threshold-time", 0u64);
        self.delay_pitch.set_property("tempo", 0.9f32);
        delay.target_delay_seconds = target;
        delay.last_micro_skip = Instant::now();
        delay.state = DelayMode::Rebuilding;
        Ok(delay_status(&delay))
    }

    pub fn set_source_mix(
        &mut self,
        id: Uuid,
        volume: f64,
        muted: bool,
        cue: bool,
    ) -> Result<(), EngineError> {
        let source = self.sources.get(&id).ok_or(EngineError::NotFound)?;
        let volume = volume.clamp(0.0, 2.0);
        for pad in [&source.route.mixer_pad, &source.route.bridge_pad]
            .into_iter()
            .flatten()
        {
            pad.set_property("volume", volume);
            pad.set_property("mute", muted);
        }
        // Cue is pre-fader and independent of the program on/off state.
        if let Some(pad) = &source.route.cue_pad {
            pad.set_property("volume", 1.0f64);
            pad.set_property("mute", !cue);
        }
        Ok(())
    }

    pub fn end_delay(&mut self) -> Result<DelayStatus, EngineError> {
        let mut delay = self
            .delay_state
            .lock()
            .map_err(|_| EngineError::Pipeline("delay state lock is poisoned".to_owned()))?;
        let dumping = delay.dumping.load(Ordering::Relaxed);
        if delay.state == DelayMode::Live || (delay.buffered_delay_seconds <= 0.05 && !dumping) {
            return Err(EngineError::Conflict(
                "End Delay is unavailable because no delay buffer exists".to_owned(),
            ));
        }
        self.delay_queue.set_property("min-threshold-time", 0u64);
        delay.target_delay_seconds = 0;
        delay.last_micro_skip = Instant::now();
        if dumping {
            delay.dumping.store(false, Ordering::Relaxed);
            delay.resync.store(true, Ordering::Relaxed);
        }
        if delay.buffered_delay_seconds <= 0.05 {
            self.delay_pitch.set_property("tempo", 1.0f32);
            delay.state = DelayMode::Live;
        } else {
            self.delay_pitch.set_property("tempo", 1.05f32);
            delay.state = DelayMode::EndingDelay;
        }
        Ok(delay_status(&delay))
    }

    fn source_summary(&self, id: Uuid) -> Option<SourceSummary> {
        self.sources.get(&id).map(|source| SourceSummary {
            id: source.id,
            name: source.name.clone(),
            source_type: source.source_type,
            transport: source.transport,
            state: state_name(source.bin.current_state()),
        })
    }

    fn output_summary(&self, id: Uuid) -> Option<OutputSummary> {
        self.outputs.get(&id).map(|output| OutputSummary {
            id: output.id,
            name: output.name.clone(),
            transport: output.transport,
            state: state_name(output.bin.current_state()),
        })
    }
}

impl InputTransport {
    fn transport_name(&self) -> &'static str {
        match self {
            Self::Test { .. } => "test",
            Self::Rtp { .. } => "rtp",
            Self::Srt { .. } => "srt",
            Self::Alsa { .. } => "alsa",
        }
    }

    fn configured_latency_ms(&self) -> Option<u32> {
        match self {
            Self::Rtp {
                jitter_buffer_ms, ..
            } => Some(*jitter_buffer_ms),
            Self::Srt { latency_ms, .. } => Some(*latency_ms),
            Self::Test { .. } | Self::Alsa { .. } => None,
        }
    }

    fn gst_description(&self, id: Uuid) -> Result<String, EngineError> {
        let standardize = format!(
            "! audioconvert ! audioresample ! audio/x-raw,format=F32LE,layout=interleaved,rate=48000,channels=2 ! level name=level-{} message=true ! queue",
            id.simple()
        );
        let description = match self {
            Self::Test { frequency } => {
                format!("audiotestsrc is-live=true wave=sine freq={frequency} {standardize}")
            }
            Self::Rtp {
                address,
                port,
                encoding,
                channels,
                payload_type,
                jitter_buffer_ms,
                interface,
            } => {
                validate_host(address)?;
                let encoding_name = match encoding {
                    RtpEncoding::L16 => "L16",
                    RtpEncoding::L24 => "L24",
                };
                let (bind_address, interface_property) =
                    rtp_input_binding(address, interface.as_deref())?;
                format!(
                    "udpsrc address={} port={port}{interface_property} caps=\"application/x-rtp,media=(string)audio,clock-rate=(int)48000,encoding-name=(string){encoding_name},channels=(int){channels},payload=(int){payload_type}\" ! rtpjitterbuffer name=jitter-{} latency={jitter_buffer_ms} do-lost=true ! rtp{encoding_name}depay {standardize}",
                    gst_quote(&bind_address),
                    id.simple()
                )
            }
            Self::Srt {
                host,
                port,
                mode,
                latency_ms,
                stream_id,
                passphrase,
                ..
            } => {
                let uri = srt_uri(
                    host,
                    *port,
                    *mode,
                    *latency_ms,
                    stream_id.as_deref(),
                    passphrase.as_deref(),
                    None,
                )?;
                format!(
                    "srtsrc uri={} ! tsdemux ! decodebin {standardize}",
                    gst_quote(&uri)
                )
            }
            Self::Alsa {
                device,
                sample_rate,
                channels,
                bit_depth,
            } => {
                let format = pcm_format(*bit_depth, false)?;
                format!(
                    "alsasrc device={} ! audio/x-raw,rate={sample_rate},channels={channels},format={format} {standardize}",
                    gst_quote(device)
                )
            }
        };
        Ok(description)
    }
}

fn build_srt_input_bin(
    id: Uuid,
    uri: &str,
    local_address: Option<&str>,
) -> Result<gst::Bin, EngineError> {
    let bin = gst::Bin::new();
    let source = gst::ElementFactory::make("srtsrc")
        .name(format!("srt-source-{}", id.simple()))
        .property("uri", uri)
        .property("wait-for-connection", false)
        .build()?;
    if let Some(address) = local_address {
        source.set_property("localaddress", address);
    }
    let demux = gst::ElementFactory::make("tsdemux").build()?;
    let decoder = gst::ElementFactory::make("decodebin").build()?;
    let convert = gst::ElementFactory::make("audioconvert").build()?;
    let resample = gst::ElementFactory::make("audioresample").build()?;
    let caps = gst::Caps::builder("audio/x-raw")
        .field("format", "F32LE")
        .field("layout", "interleaved")
        .field("rate", 48_000i32)
        .field("channels", 2i32)
        .build();
    let caps_filter = gst::ElementFactory::make("capsfilter")
        .property("caps", caps)
        .build()?;
    let level = gst::ElementFactory::make("level")
        .name(format!("level-{}", id.simple()))
        .property("message", true)
        .build()?;
    let queue = gst::ElementFactory::make("queue").build()?;

    bin.add_many([
        &source,
        &demux,
        &decoder,
        &convert,
        &resample,
        &caps_filter,
        &level,
        &queue,
    ])?;
    source.link(&demux)?;
    gst::Element::link_many([&convert, &resample, &caps_filter, &level, &queue])?;

    let decoder_sink = decoder
        .static_pad("sink")
        .ok_or_else(|| EngineError::Pipeline("decoder has no input pad".to_owned()))?;
    demux.connect_pad_added(move |_demux, src_pad| {
        if src_pad.name().starts_with("audio_") && !decoder_sink.is_linked() {
            if let Err(error) = src_pad.link(&decoder_sink) {
                tracing::error!(pad = %src_pad.name(), link_error = ?error, "Failed to link SRT audio stream to decoder");
            }
        }
    });

    let convert_sink = convert
        .static_pad("sink")
        .ok_or_else(|| EngineError::Pipeline("audio converter has no input pad".to_owned()))?;
    decoder.connect_pad_added(move |_decoder, src_pad| {
        let is_audio = src_pad
            .current_caps()
            .and_then(|caps| caps.structure(0).map(|structure| structure.name().starts_with("audio/")))
            .unwrap_or(false);
        if !is_audio {
            return;
        }
        if !convert_sink.is_linked() {
            if let Err(error) = src_pad.link(&convert_sink) {
                tracing::error!(pad = %src_pad.name(), link_error = ?error, "Failed to link decoded SRT audio");
            }
        }
    });

    let queue_src = queue
        .static_pad("src")
        .ok_or_else(|| EngineError::Pipeline("SRT audio queue has no output pad".to_owned()))?;
    bin.add_pad(&gst::GhostPad::with_target(&queue_src)?)?;
    Ok(bin)
}

impl OutputTransport {
    fn transport_name(&self) -> &'static str {
        match self {
            Self::Alsa { .. } => "alsa",
            Self::Rtp { .. } => "rtp",
            Self::Srt { .. } => "srt",
        }
    }

    fn gst_description(&self) -> Result<String, EngineError> {
        match self {
            Self::Alsa {
                device,
                sample_rate,
                channels,
                bit_depth,
            } => {
                let format = pcm_format(*bit_depth, false)?;
                Ok(format!(
                    "queue max-size-buffers=5 max-size-bytes=0 max-size-time=0 ! audioconvert ! audioresample ! audio/x-raw,rate={sample_rate},channels={channels},format={format} ! alsasink device={} sync=true",
                    gst_quote(device)
                ))
            }
            Self::Rtp {
                host,
                port,
                encoding,
                channels,
                payload_type,
                interface,
            } => {
                validate_host(host)?;
                let (encoding_name, format) = match encoding {
                    RtpEncoding::L16 => ("L16", "S16BE"),
                    RtpEncoding::L24 => ("L24", "S24BE"),
                };
                let interface_property = rtp_output_binding(host, interface.as_deref())?;
                Ok(format!(
                    "queue max-size-buffers=5 max-size-bytes=0 max-size-time=0 ! audioconvert ! audioresample ! audio/x-raw,rate=48000,channels={channels},format={format} ! rtp{encoding_name}pay pt={payload_type} ! udpsink host={} port={port}{interface_property} sync=true",
                    gst_quote(host)
                ))
            }
            Self::Srt {
                host,
                port,
                mode,
                latency_ms,
                stream_id,
                passphrase,
                pbkeylen,
                bitrate,
                interface,
            } => {
                let (host, local_address) = srt_binding(host, *mode, interface.as_deref())?;
                let uri = srt_uri(
                    &host,
                    *port,
                    *mode,
                    *latency_ms,
                    stream_id.as_deref(),
                    passphrase.as_deref(),
                    *pbkeylen,
                )?;
                let local_property = local_address
                    .map(|address| format!(" localaddress={}", gst_quote(&address)))
                    .unwrap_or_default();
                Ok(format!(
                    "queue max-size-buffers=5 max-size-bytes=0 max-size-time=0 ! audioconvert ! audioresample ! audio/x-raw,rate=48000,channels=2 ! voaacenc bitrate={bitrate} ! aacparse ! mpegtsmux ! srtsink uri={}{local_property} sync=true",
                    gst_quote(&uri)
                ))
            }
        }
    }
}

fn validate_input(input: &InputTransport) -> Result<(), EngineError> {
    match input {
        InputTransport::Test { frequency } if *frequency == 0 || *frequency > 20_000 => {
            Err(EngineError::InvalidRequest(
                "test tone frequency must be between 1 and 20000 Hz".to_owned(),
            ))
        }
        InputTransport::Rtp {
            address,
            port,
            channels,
            payload_type,
            jitter_buffer_ms,
            interface,
            ..
        } => {
            validate_host(address)?;
            validate_audio_channels(*channels)?;
            validate_port(*port)?;
            if *payload_type > 127 {
                return Err(EngineError::InvalidRequest(
                    "RTP payload_type must be between 0 and 127".to_owned(),
                ));
            }
            if *jitter_buffer_ms > 2000 {
                return Err(EngineError::InvalidRequest(
                    "RTP jitter_buffer_ms must not exceed 2000".to_owned(),
                ));
            }
            validate_interface(interface.as_deref())?;
            Ok(())
        }
        InputTransport::Srt {
            host,
            port,
            latency_ms,
            stream_id,
            passphrase,
            interface,
            ..
        } => {
            validate_interface(interface.as_deref())?;
            validate_host(host)?;
            validate_port(*port)?;
            validate_srt_latency(*latency_ms)?;
            validate_srt_credentials(stream_id.as_deref(), passphrase.as_deref())?;
            Ok(())
        }
        InputTransport::Alsa {
            device,
            sample_rate,
            channels,
            bit_depth,
        } => {
            validate_alsa_device(device)?;
            validate_sample_rate(*sample_rate)?;
            validate_audio_channels(*channels)?;
            validate_bit_depth(*bit_depth)?;
            Ok(())
        }
        _ => Ok(()),
    }
}

fn validate_output(output: &OutputTransport) -> Result<(), EngineError> {
    match output {
        OutputTransport::Alsa {
            device,
            sample_rate,
            channels,
            bit_depth,
        } => {
            validate_alsa_device(device)?;
            validate_sample_rate(*sample_rate)?;
            validate_audio_channels(*channels)?;
            validate_bit_depth(*bit_depth)?;
            Ok(())
        }
        OutputTransport::Rtp {
            host,
            port,
            channels,
            payload_type,
            interface,
            ..
        } => {
            validate_host(host)?;
            validate_port(*port)?;
            validate_audio_channels(*channels)?;
            validate_interface(interface.as_deref())?;
            if *payload_type > 127 {
                return Err(EngineError::InvalidRequest(
                    "RTP payload_type must be between 0 and 127".to_owned(),
                ));
            }
            Ok(())
        }
        OutputTransport::Srt {
            host,
            port,
            latency_ms,
            stream_id,
            passphrase,
            pbkeylen,
            bitrate,
            interface,
            ..
        } => {
            validate_interface(interface.as_deref())?;
            validate_host(host)?;
            validate_port(*port)?;
            validate_srt_latency(*latency_ms)?;
            validate_srt_credentials(stream_id.as_deref(), passphrase.as_deref())?;
            if let Some(key_length) = pbkeylen {
                if ![16, 24, 32].contains(key_length) {
                    return Err(EngineError::InvalidRequest(
                        "SRT pbkeylen must be 16, 24, or 32".to_owned(),
                    ));
                }
                if passphrase.as_ref().is_none_or(String::is_empty) {
                    return Err(EngineError::InvalidRequest(
                        "an SRT passphrase is required when pbkeylen is set".to_owned(),
                    ));
                }
            }
            if !(32_000..=512_000).contains(bitrate) {
                return Err(EngineError::InvalidRequest(
                    "SRT AAC bitrate must be between 32000 and 512000".to_owned(),
                ));
            }
            Ok(())
        }
    }
}

fn standard_caps() -> gst::Caps {
    gst::Caps::builder("audio/x-raw")
        .field("format", "F32LE")
        .field("layout", "interleaved")
        .field("rate", 48_000i32)
        .field("channels", 2i32)
        .build()
}

fn count_buffers(bin: &gst::Bin, element: &str, pad: &str, counter: &Arc<AtomicU64>) {
    let Some(pad) = bin.by_name(element).and_then(|e| e.static_pad(pad)) else {
        return;
    };
    let counter = Arc::clone(counter);
    pad.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
        counter.fetch_add(1, Ordering::Relaxed);
        gst::PadProbeReturn::Ok
    });
}

fn link_request_pad(
    source: &gst::Element,
    source_template: &str,
    sink: &gst::Element,
    sink_pad_name: &str,
) -> Result<(), EngineError> {
    let source_pad = source
        .request_pad_simple(source_template)
        .ok_or_else(|| EngineError::Pipeline("router rejected a source pad".to_owned()))?;
    let sink_pad = sink
        .static_pad(sink_pad_name)
        .ok_or_else(|| EngineError::Pipeline("output queue has no sink pad".to_owned()))?;
    if let Err(error) = source_pad.link(&sink_pad) {
        source.release_request_pad(&source_pad);
        return Err(EngineError::Pipeline(error.to_string()));
    }
    Ok(())
}

fn is_silent_pcm(pcm: &[u8]) -> bool {
    let (samples, _) = pcm.as_chunks::<4>();
    !samples.is_empty()
        && samples.iter().all(|sample| {
            let value = f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]);
            value.is_finite() && value.abs() <= 0.001
        })
}

fn delay_status(delay: &DelayRuntime) -> DelayStatus {
    let buffered_delay_seconds = if delay.state == DelayMode::Live {
        0.0
    } else {
        delay.buffered_delay_seconds
    };
    let recovery_percent = if delay.configured_delay_seconds == 0 {
        0.0
    } else {
        (buffered_delay_seconds / f64::from(delay.configured_delay_seconds) * 100.0)
            .clamp(0.0, 100.0)
    };
    DelayStatus {
        configured_delay_seconds: delay.configured_delay_seconds,
        buffered_delay_seconds,
        recovery_percent,
        state: delay.state,
    }
}

fn start_delay_monitor(queue: gst::Element, pitch: gst::Element, delay: Arc<Mutex<DelayRuntime>>) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(50));
        let buffered_ns = queue.property::<u64>("current-level-time");
        let Ok(mut delay) = delay.lock() else {
            tracing::error!("delay state lock is poisoned");
            break;
        };
        delay.buffered_delay_seconds = buffered_ns as f64 / 1_000_000_000.0;
        if delay.dumping.load(Ordering::Relaxed) {
            // The delay is empty: stop discarding and refill by slowing playback.
            if delay.buffered_delay_seconds <= 0.05 {
                delay.dumping.store(false, Ordering::Relaxed);
                delay.resync.store(true, Ordering::Relaxed);
                pitch.set_property("tempo", 0.9f32);
            }
            continue;
        }
        let target_seconds = f64::from(delay.target_delay_seconds);
        match delay.state {
            DelayMode::Rebuilding if delay.buffered_delay_seconds + 0.05 >= target_seconds => {
                queue.set_property("min-threshold-time", target_seconds as u64 * 1_000_000_000);
                pitch.set_property("tempo", 1.0f32);
                delay.state = DelayMode::Normal;
            }
            DelayMode::EndingDelay if delay.buffered_delay_seconds <= target_seconds + 0.05 => {
                queue.set_property("min-threshold-time", target_seconds as u64 * 1_000_000_000);
                pitch.set_property("tempo", 1.0f32);
                delay.state = if delay.target_delay_seconds == 0 {
                    DelayMode::Live
                } else {
                    DelayMode::Normal
                };
            }
            _ => {}
        }
    });
}

fn start_bus_monitor(pipeline: &gst::Pipeline, metrics: Arc<Mutex<LiveMetrics>>) {
    if let Some(bus) = pipeline.bus() {
        std::thread::Builder::new()
            .name("audio-engine-bus".to_owned())
            .spawn(move || loop {
                let Some(message) = bus.timed_pop(gst::ClockTime::from_mseconds(250)) else {
                    continue;
                };
                match message.view() {
                    gst::MessageView::Error(error) => {
                        let source = message
                            .src()
                            .map(|object| object.name().to_string())
                            .unwrap_or_else(|| "unknown".to_owned());
                        tracing::error!(source = %source, error = %error.error(), "GStreamer pipeline error");
                        if let Ok(mut metrics) = metrics.lock() {
                            metrics.last_pipeline_error =
                                Some("GStreamer reported an audio pipeline error".to_owned());
                        }
                    }
                    gst::MessageView::Element(element_message) => {
                        let Some(structure) = element_message.structure() else {
                            continue;
                        };
                        if structure.name() != "level" {
                            continue;
                        }
                        let Some(source_name) = message.src().map(|object| object.name().to_string())
                        else {
                            continue;
                        };
                        let Ok(peaks) = structure.get::<gst::glib::ValueArray>("peak") else {
                            continue;
                        };
                        let peaks = peaks
                            .as_slice()
                            .iter()
                            .filter_map(|value| value.get::<f64>().ok())
                            .collect::<Vec<_>>();
                        if peaks.is_empty() {
                            continue;
                        }
                        if let Ok(mut metrics) = metrics.lock() {
                            if source_name == "level-program" {
                                metrics.program_peaks = Some(peaks);
                            } else if let Some(id) = source_name
                                .strip_prefix("level-")
                                .and_then(|value| Uuid::parse_str(value).ok())
                            {
                                metrics.source_peaks.insert(id, peaks);
                            }
                            metrics.last_pipeline_error = None;
                        }
                    }
                    _ => {}
                }
            })
            .expect("audio pipeline bus thread could not be started");
    }
}

fn jitter_stats(jitter_buffer: &gst::Element) -> (Option<f64>, Option<u64>) {
    let stats = jitter_buffer.property::<gst::Structure>("stats");
    let packets_lost = stats.get::<u64>("num-lost").ok().or_else(|| {
        stats
            .get::<i32>("num-lost")
            .ok()
            .map(|value| value.max(0) as u64)
    });
    let jitter_ms = stats
        .get::<u64>("avg-jitter")
        .ok()
        .map(|value| value as f64 / 1_000_000.0)
        .or_else(|| {
            stats
                .get::<i64>("avg-jitter")
                .ok()
                .map(|value| value.max(0) as f64 / 1_000_000.0)
        });
    (jitter_ms, packets_lost)
}

fn query_pipeline_latency(pipeline: &gst::Pipeline) -> Option<f64> {
    let mut query = gst::query::Latency::new();
    if !pipeline.query(&mut query) {
        return None;
    }
    let (_, minimum, _) = query.result();
    Some(minimum.mseconds() as f64)
}

fn state_name(state: gst::State) -> &'static str {
    match state {
        gst::State::Playing => "playing",
        gst::State::Paused => "paused",
        gst::State::Ready => "ready",
        gst::State::Null => "stopped",
        _ => "transitioning",
    }
}

fn srt_uri(
    host: &str,
    port: u16,
    mode: SrtMode,
    latency_ms: u32,
    stream_id: Option<&str>,
    passphrase: Option<&str>,
    pbkeylen: Option<u8>,
) -> Result<String, EngineError> {
    validate_host(host)?;
    validate_port(port)?;
    validate_srt_latency(latency_ms)?;
    validate_srt_credentials(stream_id, passphrase)?;
    let mode = match mode {
        SrtMode::Caller => "caller",
        SrtMode::Listener => "listener",
    };
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("mode", mode);
    query.append_pair("latency", &latency_ms.to_string());
    if let Some(stream_id) = stream_id {
        query.append_pair("streamid", stream_id);
    }
    if let Some(passphrase) = passphrase {
        query.append_pair("passphrase", passphrase);
    }
    if let Some(pbkeylen) = pbkeylen {
        query.append_pair("pbkeylen", &pbkeylen.to_string());
    }
    let host = match Host::parse(host) {
        Ok(Host::Ipv6(address)) => format!("[{address}]"),
        Ok(Host::Ipv4(address)) => address.to_string(),
        Ok(Host::Domain(domain)) => domain,
        Err(_) => {
            return Err(EngineError::InvalidRequest(
                "SRT host must be an IPv4, IPv6, or DNS host".to_owned(),
            ));
        }
    };
    Ok(format!("srt://{host}:{port}?{}", query.finish()))
}

fn validate_name(name: String) -> Result<String, EngineError> {
    let name = name.trim();
    if name.is_empty() || name.len() > 100 {
        return Err(EngineError::InvalidRequest(
            "name must contain between 1 and 100 characters".to_owned(),
        ));
    }
    Ok(name.to_owned())
}

fn validate_host(host: &str) -> Result<(), EngineError> {
    if Host::parse(host).is_err() && host.parse::<IpAddr>().is_err() {
        return Err(EngineError::InvalidRequest(
            "host/address must be a valid IP address or DNS host".to_owned(),
        ));
    }
    Ok(())
}

fn validate_port(port: u16) -> Result<(), EngineError> {
    if port == 0 {
        return Err(EngineError::InvalidRequest(
            "port must be between 1 and 65535".to_owned(),
        ));
    }
    Ok(())
}

fn validate_sample_rate(sample_rate: u32) -> Result<(), EngineError> {
    if sample_rate != 48_000 {
        return Err(EngineError::InvalidRequest(
            "ALSA output sample_rate must be 48000 Hz".to_owned(),
        ));
    }
    Ok(())
}

fn validate_audio_channels(channels: u8) -> Result<(), EngineError> {
    if !(1..=2).contains(&channels) {
        return Err(EngineError::InvalidRequest(
            "audio channels must be 1 (mono) or 2 (stereo)".to_owned(),
        ));
    }
    Ok(())
}

fn validate_bit_depth(bit_depth: u8) -> Result<(), EngineError> {
    if ![16, 24].contains(&bit_depth) {
        return Err(EngineError::InvalidRequest(
            "audio bit_depth must be 16 or 24".to_owned(),
        ));
    }
    Ok(())
}

fn validate_srt_latency(latency_ms: u32) -> Result<(), EngineError> {
    if !(20..=90_000).contains(&latency_ms) {
        return Err(EngineError::InvalidRequest(
            "SRT latency_ms must be between 20 and 90000".to_owned(),
        ));
    }
    Ok(())
}

fn validate_srt_credentials(
    stream_id: Option<&str>,
    passphrase: Option<&str>,
) -> Result<(), EngineError> {
    if stream_id.is_some_and(|value| value.len() > 512 || value.contains('\0')) {
        return Err(EngineError::InvalidRequest(
            "SRT stream_id must not exceed 512 characters".to_owned(),
        ));
    }
    if passphrase.is_some_and(|value| !(10..=79).contains(&value.len())) {
        return Err(EngineError::InvalidRequest(
            "SRT passphrase must contain between 10 and 79 characters".to_owned(),
        ));
    }
    Ok(())
}

fn validate_alsa_device(device: &str) -> Result<(), EngineError> {
    if device.is_empty() || device.len() > 128 || !is_safe_gst_value(device) {
        return Err(EngineError::InvalidRequest(
            "ALSA device must be a non-empty device identifier".to_owned(),
        ));
    }
    Ok(())
}

fn is_safe_gst_value(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"_-. :,/".contains(&byte))
}

fn validate_interface(interface: Option<&str>) -> Result<(), EngineError> {
    match interface {
        Some(name) if name.is_empty() || name.len() > 15 || !is_safe_gst_value(name) => Err(
            EngineError::InvalidRequest("network interface name is not valid".to_owned()),
        ),
        _ => Ok(()),
    }
}

fn interface_ipv4(name: &str) -> Result<String, EngineError> {
    crate::system::interface_ipv4(name).ok_or_else(|| {
        EngineError::InvalidRequest(format!(
            "network interface {name} was not found or has no IPv4 address"
        ))
    })
}

fn is_multicast_host(host: &str) -> bool {
    host.parse::<IpAddr>()
        .is_ok_and(|address| address.is_multicast())
}

// Returns the address to bind and any extra udpsrc properties.
fn build_bus_mixer(
    pipeline: &gst::Pipeline,
    prefix: &str,
) -> Result<(gst::Element, gst::Element), EngineError> {
    let make = |factory: &str, suffix: &str| {
        gst::ElementFactory::make(factory)
            .name(format!("{prefix}_{suffix}"))
            .build()
    };
    let mixer = make("audiomixer", "mixer")?;
    let silence = gst::ElementFactory::make("audiotestsrc")
        .name(format!("{prefix}_silence"))
        .property_from_str("wave", "silence")
        .property("is-live", true)
        .build()?;
    let silence_caps = gst::ElementFactory::make("capsfilter")
        .name(format!("{prefix}_silence_caps"))
        .property("caps", standard_caps())
        .build()?;
    let convert = make("audioconvert", "convert")?;
    let resample = make("audioresample", "resample")?;
    let caps = gst::ElementFactory::make("capsfilter")
        .name(format!("{prefix}_caps"))
        .property("caps", standard_caps())
        .build()?;
    let tee = make("tee", "tee")?;
    let null_queue = gst::ElementFactory::make("queue")
        .name(format!("{prefix}_null_queue"))
        .property("max-size-buffers", 5u32)
        .property("max-size-bytes", 0u32)
        .property("max-size-time", 0u64)
        .build()?;
    let null_sink = gst::ElementFactory::make("fakesink")
        .name(format!("{prefix}_null_sink"))
        .property("sync", true)
        .property("async", false)
        .build()?;
    pipeline.add_many([
        &mixer,
        &silence,
        &silence_caps,
        &convert,
        &resample,
        &caps,
        &tee,
        &null_queue,
        &null_sink,
    ])?;
    gst::Element::link_many([&silence, &silence_caps])?;
    let silence_pad = mixer
        .request_pad_simple("sink_%u")
        .ok_or_else(|| EngineError::Pipeline(format!("{prefix} mixer rejected a pad")))?;
    silence_caps
        .static_pad("src")
        .ok_or_else(|| EngineError::Pipeline(format!("{prefix} silence has no pad")))?
        .link(&silence_pad)
        .map_err(|error| EngineError::Pipeline(error.to_string()))?;
    gst::Element::link_many([&mixer, &convert, &resample, &caps, &tee])?;
    null_queue.link(&null_sink)?;
    link_request_pad(&tee, "src_%u", &null_queue, "sink")?;
    Ok((mixer, tee))
}

fn rtp_input_binding(
    address: &str,
    interface: Option<&str>,
) -> Result<(String, String), EngineError> {
    let Some(name) = interface.filter(|name| !name.is_empty()) else {
        return Ok((address.to_owned(), String::new()));
    };
    if is_multicast_host(address) {
        return Ok((
            address.to_owned(),
            format!(" multicast-iface={}", gst_quote(name)),
        ));
    }
    let unspecified = address
        .parse::<IpAddr>()
        .is_ok_and(|address| address.is_unspecified());
    if unspecified {
        return Ok((interface_ipv4(name)?, String::new()));
    }
    Ok((address.to_owned(), String::new()))
}

fn rtp_output_binding(host: &str, interface: Option<&str>) -> Result<String, EngineError> {
    let Some(name) = interface.filter(|name| !name.is_empty()) else {
        return Ok(String::new());
    };
    let mut properties = format!(" bind-address={}", gst_quote(&interface_ipv4(name)?));
    if is_multicast_host(host) {
        properties.push_str(&format!(" multicast-iface={}", gst_quote(name)));
    }
    Ok(properties)
}

// Listener mode binds to the URI host, so the interface address replaces it; callers use localaddress.
fn srt_binding(
    host: &str,
    mode: SrtMode,
    interface: Option<&str>,
) -> Result<(String, Option<String>), EngineError> {
    let Some(name) = interface.filter(|name| !name.is_empty()) else {
        return Ok((host.to_owned(), None));
    };
    let address = interface_ipv4(name)?;
    match mode {
        SrtMode::Listener => Ok((address, None)),
        SrtMode::Caller => Ok((host.to_owned(), Some(address))),
    }
}

fn pcm_format(bit_depth: u8, big_endian: bool) -> Result<&'static str, EngineError> {
    match (bit_depth, big_endian) {
        (16, false) => Ok("S16LE"),
        (16, true) => Ok("S16BE"),
        (24, false) => Ok("S24LE"),
        (24, true) => Ok("S24BE"),
        _ => Err(EngineError::InvalidRequest(
            "audio bit_depth must be 16 or 24".to_owned(),
        )),
    }
}

fn gst_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn sanitize_pipeline_error(error: String) -> String {
    if error.contains("passphrase=") {
        "GStreamer rejected the configured pipeline".to_owned()
    } else {
        error
    }
}

fn default_test_frequency() -> u32 {
    440
}

fn default_rtp_address() -> String {
    "0.0.0.0".to_owned()
}

fn default_rtp_port() -> u16 {
    5005
}

fn default_srt_host() -> String {
    "0.0.0.0".to_owned()
}

fn default_srt_latency_ms() -> u32 {
    120
}

fn default_srt_output_mode() -> SrtMode {
    SrtMode::Caller
}

fn default_channels() -> u8 {
    2
}

fn default_payload_type() -> u8 {
    96
}

fn default_jitter_buffer_ms() -> u32 {
    100
}

fn default_sample_rate() -> u32 {
    48_000
}

fn default_bit_depth() -> u8 {
    24
}

fn default_aac_bitrate() -> u32 {
    128_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_delay_status_ignores_transient_queue_levels() {
        let mut delay = DelayRuntime {
            configured_delay_seconds: 10,
            target_delay_seconds: 0,
            buffered_delay_seconds: 0.12,
            state: DelayMode::Live,
            last_micro_skip: Instant::now(),
            dumping: Arc::new(AtomicBool::new(false)),
            resync: Arc::new(AtomicBool::new(false)),
        };
        let status = delay_status(&delay);
        assert_eq!(status.buffered_delay_seconds, 0.0);
        assert_eq!(status.recovery_percent, 0.0);
        delay.state = DelayMode::Rebuilding;
        let status = delay_status(&delay);
        assert_eq!(status.buffered_delay_seconds, 0.12);
        assert!((status.recovery_percent - 1.2).abs() < 1e-10);
    }

    #[test]
    fn srt_uris_apply_role_defaults_and_encode_secrets() {
        let uri = srt_uri(
            "127.0.0.1",
            9000,
            SrtMode::Listener,
            120,
            Some("cloud console"),
            Some("secret-pass"),
            Some(16),
        )
        .expect("valid SRT URI");
        assert!(uri.contains("mode=listener"));
        assert!(uri.contains("streamid=cloud+console"));
        assert!(uri.contains("passphrase=secret-pass"));
        assert!(uri.contains("pbkeylen=16"));
    }

    #[test]
    fn rejects_invalid_audio_formats_and_transport_settings() {
        assert!(validate_sample_rate(44_100).is_err());
        assert!(validate_bit_depth(32).is_err());
        assert!(validate_audio_channels(3).is_err());
        assert!(validate_port(0).is_err());
    }

    #[test]
    fn errors_and_summaries_do_not_include_secrets() {
        let error = sanitize_pipeline_error(
            "pipeline error for srt://host:9000?passphrase=secret".to_owned(),
        );
        assert!(!error.contains("secret"));
        assert_eq!(
            serde_json::json!({"mode": "caller"})["mode"],
            serde_json::Value::String("caller".to_owned())
        );
    }

    #[test]
    fn silence_detection_only_drops_low_level_float_audio() {
        let silent = [0.0f32, 0.0005, -0.0008]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>();
        let speech = [0.0f32, 0.02]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>();
        assert!(is_silent_pcm(&silent));
        assert!(!is_silent_pcm(&speech));
        assert!(!is_silent_pcm(&[]));
        assert!(is_silent_pcm(&[silent.as_slice(), &[1]].concat()));
    }
}
