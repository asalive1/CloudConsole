//! Pass-through TCP metadata relay. Every enabled input fans its bytes out to every
//! enabled output unchanged, so the payload format (XML, ASCII, RDS, HD Radio, custom)
//! never matters. Outputs marked `delayed` hold data for the current Program audio
//! delay so metadata stays aligned with what listeners hear.

use std::{
    collections::{HashMap, VecDeque},
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpSocket, TcpStream},
    sync::broadcast,
    task::{JoinHandle, JoinSet},
    time::sleep,
};
use uuid::Uuid;

const MAX_PER_DIRECTION: usize = 2;
const RETRY_DELAY: Duration = Duration::from_secs(3);

type Chunk = Arc<(Instant, Vec<u8>)>;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Input,
    Output,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Server,
    Client,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct MetadataEndpoint {
    pub id: Uuid,
    pub direction: Direction,
    pub mode: Mode,
    pub host: Option<String>,
    pub port: u16,
    pub interface: Option<String>,
    #[serde(default = "default_delayed")]
    pub delayed: bool,
}

fn default_delayed() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct MetadataConfig {
    pub endpoints: Vec<MetadataEndpoint>,
}

/// The Program delay as seen by the relay, refreshed by the engine monitor.
#[derive(Default)]
pub struct DelayClock {
    buffered_ms: AtomicU64,
    // Bumped when the buffer is dumped so held metadata is discarded with the audio.
    epoch: AtomicU64,
}

impl DelayClock {
    pub fn update(&self, buffered_seconds: f64) {
        let next = (buffered_seconds.max(0.0) * 1000.0) as u64;
        let previous = self.buffered_ms.swap(next, Ordering::Relaxed);
        if previous > next + 1000 {
            self.epoch.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[derive(Default)]
struct Stats {
    connections: AtomicU32,
    bytes: AtomicU64,
    error: Mutex<Option<String>>,
}

impl Stats {
    fn set_error(&self, message: Option<String>) {
        if let Ok(mut error) = self.error.lock() {
            *error = message;
        }
    }
}

struct ConnectionGuard(Arc<Stats>);

impl ConnectionGuard {
    fn new(stats: &Arc<Stats>) -> Self {
        stats.connections.fetch_add(1, Ordering::Relaxed);
        stats.set_error(None);
        Self(stats.clone())
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.connections.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Serialize)]
pub struct EndpointStatus {
    pub id: Uuid,
    pub direction: Direction,
    pub mode: Mode,
    pub port: u16,
    pub connections: u32,
    pub bytes: u64,
    pub error: Option<String>,
}

struct Running {
    endpoint: MetadataEndpoint,
    stats: Arc<Stats>,
    task: JoinHandle<()>,
}

pub struct MetadataRelay {
    tx: broadcast::Sender<Chunk>,
    clock: Arc<DelayClock>,
    running: HashMap<Uuid, Running>,
}

impl MetadataRelay {
    pub fn new(clock: Arc<DelayClock>) -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self {
            tx,
            clock,
            running: HashMap::new(),
        }
    }

    pub fn apply(&mut self, endpoints: Vec<MetadataEndpoint>) -> Result<(), String> {
        validate(&endpoints)?;
        let wanted: HashMap<Uuid, &MetadataEndpoint> =
            endpoints.iter().map(|item| (item.id, item)).collect();
        self.running.retain(|id, running| {
            let keep = wanted
                .get(id)
                .is_some_and(|next| **next == running.endpoint);
            if !keep {
                running.task.abort();
            }
            keep
        });
        for endpoint in endpoints {
            if self.running.contains_key(&endpoint.id) {
                continue;
            }
            let stats = Arc::new(Stats::default());
            let task = tokio::spawn(run_endpoint(
                endpoint.clone(),
                self.tx.clone(),
                self.clock.clone(),
                stats.clone(),
            ));
            self.running.insert(
                endpoint.id,
                Running {
                    endpoint,
                    stats,
                    task,
                },
            );
        }
        Ok(())
    }

    pub fn status(&self) -> Vec<EndpointStatus> {
        self.running
            .values()
            .map(|running| EndpointStatus {
                id: running.endpoint.id,
                direction: running.endpoint.direction,
                mode: running.endpoint.mode,
                port: running.endpoint.port,
                connections: running.stats.connections.load(Ordering::Relaxed),
                bytes: running.stats.bytes.load(Ordering::Relaxed),
                error: running
                    .stats
                    .error
                    .lock()
                    .ok()
                    .and_then(|error| error.clone()),
            })
            .collect()
    }
}

fn validate(endpoints: &[MetadataEndpoint]) -> Result<(), String> {
    for direction in [Direction::Input, Direction::Output] {
        let count = endpoints
            .iter()
            .filter(|item| item.direction == direction)
            .count();
        if count > MAX_PER_DIRECTION {
            return Err(format!(
                "at most {MAX_PER_DIRECTION} metadata inputs and outputs are supported"
            ));
        }
    }
    for endpoint in endpoints {
        if endpoint.port == 0 {
            return Err("metadata port must be between 1 and 65535".to_owned());
        }
        bind_ip(endpoint)?;
        if endpoint.mode == Mode::Server {
            crate::network::check_listener_port("METADATA_TCP_PORTS", endpoint.port)?;
        }
        if endpoint.mode == Mode::Client {
            let host = endpoint.host.as_deref().unwrap_or("").trim();
            if host.is_empty() || host.len() > 253 || host.contains(char::is_whitespace) {
                return Err("metadata client mode requires a valid host".to_owned());
            }
        }
    }
    Ok(())
}

fn bind_ip(endpoint: &MetadataEndpoint) -> Result<Option<IpAddr>, String> {
    let Some(name) = endpoint
        .interface
        .as_deref()
        .filter(|name| !name.is_empty())
    else {
        return crate::network::default_address("METADATA_BIND")
            .map(|address| {
                address
                    .parse()
                    .map_err(|_| "invalid METADATA_BIND".to_owned())
            })
            .transpose();
    };
    crate::system::interface_ipv4(name)
        .and_then(|address| address.parse().ok())
        .map(Some)
        .ok_or_else(|| format!("network interface {name} was not found or has no IPv4 address"))
}

async fn listen(endpoint: &MetadataEndpoint) -> Result<TcpListener, String> {
    let ip = bind_ip(endpoint)?.unwrap_or(IpAddr::from([0, 0, 0, 0]));
    TcpListener::bind(SocketAddr::new(ip, endpoint.port))
        .await
        .map_err(|error| format!("could not listen on port {}: {error}", endpoint.port))
}

async fn connect(endpoint: &MetadataEndpoint) -> Result<TcpStream, String> {
    let host = endpoint.host.as_deref().unwrap_or("").trim();
    let local = bind_ip(endpoint)?;
    let mut addresses = tokio::net::lookup_host((host, endpoint.port))
        .await
        .map_err(|error| format!("could not resolve {host}: {error}"))?;
    let remote = addresses
        .find(|address| address.is_ipv4())
        .ok_or_else(|| format!("{host} has no IPv4 address"))?;
    let socket = TcpSocket::new_v4().map_err(|error| error.to_string())?;
    if let Some(local) = local {
        socket
            .bind(SocketAddr::new(local, 0))
            .map_err(|error| error.to_string())?;
    }
    tokio::time::timeout(Duration::from_secs(5), socket.connect(remote))
        .await
        .map_err(|_| format!("connection to {remote} timed out"))?
        .map_err(|error| format!("could not connect to {remote}: {error}"))
}

async fn run_endpoint(
    endpoint: MetadataEndpoint,
    tx: broadcast::Sender<Chunk>,
    clock: Arc<DelayClock>,
    stats: Arc<Stats>,
) {
    let mut sessions = JoinSet::new();
    loop {
        match endpoint.mode {
            Mode::Server => match listen(&endpoint).await {
                Ok(listener) => {
                    stats.set_error(None);
                    loop {
                        tokio::select! {
                            accepted = listener.accept() => match accepted {
                                Ok((stream, _)) => {
                                    sessions.spawn(session(
                                        stream, endpoint.clone(), tx.clone(), clock.clone(), stats.clone(),
                                    ));
                                }
                                Err(_) => sleep(Duration::from_secs(1)).await,
                            },
                            Some(_) = sessions.join_next(), if !sessions.is_empty() => {}
                        }
                    }
                }
                Err(error) => stats.set_error(Some(error)),
            },
            Mode::Client => match connect(&endpoint).await {
                Ok(stream) => {
                    session(
                        stream,
                        endpoint.clone(),
                        tx.clone(),
                        clock.clone(),
                        stats.clone(),
                    )
                    .await;
                }
                Err(error) => stats.set_error(Some(error)),
            },
        }
        sleep(RETRY_DELAY).await;
    }
}

async fn session(
    stream: TcpStream,
    endpoint: MetadataEndpoint,
    tx: broadcast::Sender<Chunk>,
    clock: Arc<DelayClock>,
    stats: Arc<Stats>,
) {
    let _guard = ConnectionGuard::new(&stats);
    match endpoint.direction {
        Direction::Input => read_loop(stream, tx, &stats).await,
        Direction::Output => {
            write_loop(stream, tx.subscribe(), &clock, endpoint.delayed, &stats).await;
        }
    }
}

async fn read_loop(mut stream: TcpStream, tx: broadcast::Sender<Chunk>, stats: &Stats) {
    let mut buffer = [0u8; 4096];
    loop {
        match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(count) => {
                stats.bytes.fetch_add(count as u64, Ordering::Relaxed);
                // No receivers simply means no output is currently configured.
                let _ = tx.send(Arc::new((Instant::now(), buffer[..count].to_vec())));
            }
        }
    }
}

async fn write_loop(
    mut stream: TcpStream,
    mut rx: broadcast::Receiver<Chunk>,
    clock: &DelayClock,
    delayed: bool,
    stats: &Stats,
) {
    let mut queue: VecDeque<Chunk> = VecDeque::new();
    let mut epoch = clock.epoch.load(Ordering::Relaxed);
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    let mut discard = [0u8; 256];
    loop {
        tokio::select! {
            received = rx.recv() => match received {
                Ok(chunk) => queue.push_back(chunk),
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            },
            _ = tick.tick() => {
                let current = clock.epoch.load(Ordering::Relaxed);
                if current != epoch {
                    epoch = current;
                    queue.clear();
                }
                let delay = if delayed {
                    Duration::from_millis(clock.buffered_ms.load(Ordering::Relaxed))
                } else {
                    Duration::ZERO
                };
                while queue.front().is_some_and(|chunk| chunk.0.elapsed() >= delay) {
                    let Some(chunk) = queue.pop_front() else { break };
                    if stream.write_all(&chunk.1).await.is_err() {
                        return;
                    }
                    stats.bytes.fetch_add(chunk.1.len() as u64, Ordering::Relaxed);
                }
            }
            // Reading detects a closed peer; anything a puller sends is ignored.
            read = stream.read(&mut discard) => {
                if matches!(read, Ok(0) | Err(_)) {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(
        direction: Direction,
        mode: Mode,
        host: Option<&str>,
        port: u16,
    ) -> MetadataEndpoint {
        MetadataEndpoint {
            id: Uuid::new_v4(),
            direction,
            mode,
            host: host.map(str::to_owned),
            port,
            interface: None,
            delayed: true,
        }
    }

    #[test]
    fn validation_rejects_bad_endpoints() {
        assert!(validate(&[endpoint(Direction::Input, Mode::Server, None, 0)]).is_err());
        assert!(validate(&[endpoint(Direction::Input, Mode::Client, None, 4000)]).is_err());
        assert!(validate(&[endpoint(Direction::Input, Mode::Client, Some("a b"), 4000)]).is_err());
        assert!(validate(&[endpoint(
            Direction::Input,
            Mode::Client,
            Some("10.0.0.1"),
            4000
        )])
        .is_ok());
        let many: Vec<_> = (0..3)
            .map(|index| endpoint(Direction::Output, Mode::Server, None, 6000 + index))
            .collect();
        assert!(validate(&many).is_err());
    }

    #[test]
    fn dumping_the_buffer_bumps_the_epoch() {
        let clock = DelayClock::default();
        clock.update(10.0);
        clock.update(9.9);
        assert_eq!(clock.epoch.load(Ordering::Relaxed), 0);
        clock.update(0.0);
        assert_eq!(clock.epoch.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn relays_input_bytes_to_output_after_the_delay() {
        let clock = Arc::new(DelayClock::default());
        clock.update(0.2);
        let mut relay = MetadataRelay::new(clock);
        let free = |port_probe: std::net::TcpListener| port_probe.local_addr().map(|a| a.port());
        let in_port = free(std::net::TcpListener::bind("127.0.0.1:0").unwrap()).unwrap();
        let out_port = free(std::net::TcpListener::bind("127.0.0.1:0").unwrap()).unwrap();
        relay
            .apply(vec![
                endpoint(Direction::Input, Mode::Server, None, in_port),
                endpoint(Direction::Output, Mode::Server, None, out_port),
            ])
            .unwrap();
        sleep(Duration::from_millis(200)).await;
        let mut output = TcpStream::connect(("127.0.0.1", out_port)).await.unwrap();
        let mut input = TcpStream::connect(("127.0.0.1", in_port)).await.unwrap();
        sleep(Duration::from_millis(100)).await;
        let sent = Instant::now();
        input.write_all(b"<now>song</now>").await.unwrap();
        let mut received = vec![0u8; 15];
        output.read_exact(&mut received).await.unwrap();
        assert_eq!(received, b"<now>song</now>");
        assert!(sent.elapsed() >= Duration::from_millis(180));
    }
}
