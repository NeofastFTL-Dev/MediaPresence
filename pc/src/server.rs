//! Tiny HTTP/1.1 server (raw sockets, so it needs no URL ACL or admin rights) on the configured
//! port, plus the UDP responder the phone uses to find this PC on the wifi.

use crate::config::Config;
use crate::presence::Presence;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const DISCOVERY_PORT: u16 = 47801;
const DISCOVERY_REQUEST: &str = "NEOPRESENCE_DISCOVER";
const MAX_BODY: usize = 128 * 1024;
const MAX_HEAD: usize = 64 * 1024;
const RATE_LIMIT: u64 = 200;
const RATE_WINDOW: Duration = Duration::from_secs(10);

struct Settings {
    token: String,
    port: i64,
    enabled: bool,
}

struct Run {
    stop: Arc<AtomicBool>,
    port: u16,
}

pub struct Server {
    presence: Arc<Presence>,
    settings: Mutex<Settings>,
    run: Mutex<Option<Run>>,
    status: Mutex<String>,
    last_error: Mutex<String>,
    window_start: AtomicU64,
    window_count: AtomicU64,
}

impl Server {
    pub fn new(presence: Arc<Presence>, cfg: &Config) -> Arc<Self> {
        Arc::new(Self {
            presence,
            settings: Mutex::new(Settings {
                token: cfg.token.clone(),
                port: cfg.port,
                enabled: cfg.enabled,
            }),
            run: Mutex::new(None),
            status: Mutex::new("Gestopt".to_string()),
            last_error: Mutex::new(String::new()),
            window_start: AtomicU64::new(0),
            window_count: AtomicU64::new(0),
        })
    }

    pub fn status(&self) -> String {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn last_error(&self) -> String {
        self.last_error.lock().map(|e| e.clone()).unwrap_or_default()
    }

    pub fn restart(self: &Arc<Self>, cfg: &Config) {
        self.stop();
        self.start(cfg);
    }

    pub fn start(self: &Arc<Self>, cfg: &Config) {
        self.start_on(cfg, DISCOVERY_PORT);
    }

    /// Same as [`Server::start`], with the discovery port overridden so the self-test can run
    /// next to a real instance instead of fighting it for 47801.
    pub fn start_on(self: &Arc<Self>, cfg: &Config, discovery_port: u16) {
        if let Ok(mut settings) = self.settings.lock() {
            settings.token = cfg.token.clone();
            settings.port = cfg.port;
            settings.enabled = cfg.enabled;
        }

        let port = if (1024..=65535).contains(&cfg.port) { cfg.port as u16 } else { 8787 };
        let listener = match TcpListener::bind(("0.0.0.0", port)) {
            Ok(listener) => listener,
            Err(e) => {
                self.set_status("Server start niet");
                let message = format!("Poort {port}: {e}");
                self.set_error(&message);
                crate::config::log_error("server", &message);
                return;
            }
        };
        let _ = listener.set_nonblocking(true);

        self.set_status(&format!("Luistert op poort {port}"));
        self.set_error("");

        let stop = Arc::new(AtomicBool::new(false));
        if let Ok(mut run) = self.run.lock() {
            *run = Some(Run { stop: Arc::clone(&stop), port });
        }

        let this = Arc::clone(self);
        std::thread::Builder::new()
            .name("http-accept".into())
            .spawn(move || this.accept_loop(listener, stop))
            .ok();

        self.start_discovery(port, discovery_port);
    }

    pub fn stop(&self) {
        let run = self.run.lock().ok().and_then(|mut run| run.take());
        if let Some(run) = &run {
            run.stop.store(true, Ordering::Relaxed);
            // Unblock the accept loop's listener by dropping it: it is non-blocking, so the loop
            // sees the stop flag within one sleep either way.
            let _ = TcpStream::connect(("127.0.0.1", run.port));
        }
        self.set_status("Gestopt");
    }

    fn set_status(&self, text: &str) {
        if let Ok(mut status) = self.status.lock() {
            *status = text.to_string();
        }
    }

    fn set_error(&self, text: &str) {
        if let Ok(mut error) = self.last_error.lock() {
            *error = text.to_string();
        }
    }

    fn accept_loop(self: Arc<Self>, listener: TcpListener, stop: Arc<AtomicBool>) {
        while !stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _peer)) => {
                    let this = Arc::clone(&self);
                    std::thread::spawn(move || this.handle_client(stream));
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    self.set_error(&e.to_string());
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
        }
    }

    fn handle_client(self: Arc<Self>, mut stream: TcpStream) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

        let (head, extra) = match read_head(&mut stream) {
            Some(parts) => parts,
            None => return,
        };

        let mut lines = head.split("\r\n");
        let request_line = lines.next().unwrap_or("");
        let mut parts = request_line.split(' ');
        let method = parts.next().unwrap_or("");
        let target = parts.next().unwrap_or("");
        let path = target.split('?').next().unwrap_or("");

        let mut headers: Vec<(String, String)> = Vec::new();
        for line in lines {
            if let Some((name, value)) = line.split_once(':') {
                headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
            }
        }
        let header = |name: &str| {
            headers
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };

        if header("expect").map(|v| v.to_ascii_lowercase().contains("100-continue")).unwrap_or(false) {
            let _ = stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
            let _ = stream.flush();
        }

        let body = match read_body(&mut stream, header("content-length"), extra) {
            Some(body) => body,
            None => {
                respond(&mut stream, 413, &error("payload too large"));
                return;
            }
        };

        if method == "GET" && (path == "/status" || path == "/health") {
            respond(&mut stream, 200, &self.status_json().to_string());
            return;
        }

        if method != "POST" || path != "/update" {
            respond(&mut stream, 404, &error("not found"));
            return;
        }

        if !self.authorized(header("authorization")) {
            respond(&mut stream, 401, &error("unauthorized"));
            return;
        }

        if self.rate_limited() {
            respond(&mut stream, 429, &error("too many requests"));
            return;
        }

        let Ok(payload) = serde_json::from_str::<Value>(&body) else {
            respond(&mut stream, 400, &error("invalid json"));
            return;
        };
        if !payload.is_object() {
            respond(&mut stream, 400, &error("invalid json"));
            return;
        }

        self.presence.update(&payload);
        respond(&mut stream, 200, &json!({ "ok": true }).to_string());
    }

    fn status_json(&self) -> Value {
        let (enabled, port) = self
            .settings
            .lock()
            .map(|settings| (settings.enabled, settings.port))
            .unwrap_or((false, 0));
        json!({
            "ok": true,
            "enabled": enabled,
            "port": port,
            "lastError": self.last_error(),
        })
    }

    fn authorized(&self, header: Option<&str>) -> bool {
        let Some(header) = header else { return false };
        let lower = header.to_ascii_lowercase();
        if !lower.starts_with("bearer ") {
            return false;
        }
        let given = header[7..].trim().as_bytes();
        let wanted = self.settings.lock().map(|s| s.token.clone()).unwrap_or_default();
        let wanted = wanted.as_bytes();
        // Length-independent comparison, like the fixed-time compare in the .NET build.
        if given.len() != wanted.len() {
            return false;
        }
        given.iter().zip(wanted).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
    }

    fn rate_limited(&self) -> bool {
        let now = now_ms();
        let start = self.window_start.load(Ordering::Relaxed);
        if now.saturating_sub(start) > RATE_WINDOW.as_millis() as u64 {
            self.window_start.store(now, Ordering::Relaxed);
            self.window_count.store(0, Ordering::Relaxed);
        }
        self.window_count.fetch_add(1, Ordering::Relaxed) + 1 > RATE_LIMIT
    }

    fn start_discovery(self: &Arc<Self>, port: u16, discovery_port: u16) {
        let socket = match UdpSocket::bind(("0.0.0.0", discovery_port)) {
            Ok(socket) => socket,
            Err(e) => {
                // Discovery is optional: the phone then uses the last known address or the tunnel.
                self.set_error(&format!("Wifi-detectie: {e}"));
                return;
            }
        };
        let _ = socket.set_nonblocking(true);

        let stop = self
            .run
            .lock()
            .ok()
            .and_then(|run| run.as_ref().map(|run| Arc::clone(&run.stop)))
            .unwrap_or_else(|| Arc::new(AtomicBool::new(true)));

        std::thread::Builder::new()
            .name("udp-discovery".into())
            .spawn(move || {
                let mut buffer = [0u8; 512];
                while !stop.load(Ordering::Relaxed) {
                    match socket.recv_from(&mut buffer) {
                        Ok((read, peer)) => {
                            let text = String::from_utf8_lossy(&buffer[..read]);
                            if !text.starts_with(DISCOVERY_REQUEST) {
                                continue;
                            }
                            let reply = format!("NEOPRESENCE:{port}");
                            let _ = socket.send_to(reply.as_bytes(), peer);
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(250)),
                    }
                }
            })
            .ok();
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn read_head(stream: &mut TcpStream) -> Option<(String, Vec<u8>)> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(index) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buffer[..index]).to_string();
            let extra = buffer[index + 4..].to_vec();
            return Some((head, extra));
        }
        if buffer.len() > MAX_HEAD {
            return None;
        }
    }
}

fn read_body(stream: &mut TcpStream, content_length: Option<&str>, mut extra: Vec<u8>) -> Option<String> {
    let Some(length) = content_length.and_then(|raw| raw.trim().parse::<usize>().ok()) else {
        return Some(String::new());
    };
    if length == 0 {
        return Some(String::new());
    }
    if length > MAX_BODY {
        return None;
    }

    extra.truncate(length);
    let mut body = extra;
    let mut chunk = [0u8; 8192];
    while body.len() < length {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                let room = length - body.len();
                body.extend_from_slice(&chunk[..n.min(room)]);
            }
            Err(_) => break,
        }
    }
    Some(String::from_utf8_lossy(&body).to_string())
}

fn respond(stream: &mut TcpStream, code: u16, json: &str) {
    let body = json.as_bytes();
    let head = format!(
        "HTTP/1.1 {code} {}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        reason(code),
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        _ => "Error",
    }
}

fn error(message: &str) -> String {
    json!({ "error": message }).to_string()
}

/// Binds an ephemeral port and hands it back, so the self-test never fights a real instance.
pub fn pick_free_port() -> Option<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).ok()?;
    let port = listener.local_addr().ok()?.port();
    drop(listener);
    Some(port)
}

/// One raw request against a running server, used by the self-test.
pub fn request(port: u16, raw: &str) -> Option<(u16, String)> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    stream.write_all(raw.as_bytes()).ok()?;
    stream.flush().ok()?;

    let mut response = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&response).to_string();
    let code = text.split(' ').nth(1)?.parse().ok()?;
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Some((code, body))
}
