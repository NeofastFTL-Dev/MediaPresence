//! Minimal Discord RPC client: talks to the local Discord socket, exchanges length-prefixed
//! JSON frames and pushes `SET_ACTIVITY`. It reconnects on its own when Discord starts, quits or
//! restarts, and never blocks the HTTP server - the presence worker is the only writer.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;

/// A stalled Discord must never hold us forever.
const WRITE_TIMEOUT: Duration = Duration::from_millis(2000);

#[cfg(unix)]
type Handle = std::os::unix::net::UnixStream;
#[cfg(windows)]
type Handle = std::fs::File;

pub struct DiscordIpc {
    app_id: Mutex<String>,
    writer: Mutex<Option<Handle>>,
    connected: AtomicBool,
    run: AtomicBool,
    user: Mutex<String>,
    error: Mutex<String>,
}

impl DiscordIpc {
    pub fn new(app_id: String) -> Arc<Self> {
        Arc::new(Self {
            app_id: Mutex::new(app_id),
            writer: Mutex::new(None),
            connected: AtomicBool::new(false),
            run: AtomicBool::new(true),
            user: Mutex::new(String::new()),
            error: Mutex::new(String::new()),
        })
    }

    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    pub fn user_name(&self) -> String {
        self.user.lock().map(|u| u.clone()).unwrap_or_default()
    }

    pub fn last_error(&self) -> String {
        self.error.lock().map(|e| e.clone()).unwrap_or_default()
    }

    pub fn set_app_id(&self, app_id: &str) {
        if let Ok(mut id) = self.app_id.lock() {
            *id = app_id.to_string();
        }
        // The handshake carries the id, so a changed id only takes effect on the next connect.
        self.drop_pipe();
    }

    pub fn stop(&self) {
        self.run.store(false, Ordering::Relaxed);
        self.connected.store(false, Ordering::Relaxed);
        self.drop_pipe();
    }

    /// Starts the reconnect loop on its own thread.
    pub fn start(self: &Arc<Self>) {
        let this = Arc::clone(self);
        std::thread::Builder::new()
            .name("discord-ipc".into())
            .spawn(move || this.run_loop())
            .ok();
    }

    fn run_loop(self: Arc<Self>) {
        while self.run.load(Ordering::Relaxed) {
            let Some((reader, writer)) = open_pipe() else {
                self.set_error("Discord is niet open");
                self.sleep_unless_stopped();
                continue;
            };

            if let Ok(mut slot) = self.writer.lock() {
                *slot = Some(writer);
            }

            let app_id = self.app_id.lock().map(|id| id.clone()).unwrap_or_default();
            if !self.write_frame(&json!({ "v": 1, "client_id": app_id }), OP_HANDSHAKE) {
                self.sleep_unless_stopped();
                continue;
            }

            self.read_loop(reader);

            self.connected.store(false, Ordering::Relaxed);
            if let Ok(mut user) = self.user.lock() {
                user.clear();
            }
            self.drop_pipe();
            self.sleep_unless_stopped();
        }
    }

    fn sleep_unless_stopped(&self) {
        for _ in 0..50 {
            if !self.run.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn set_error(&self, text: &str) {
        if let Ok(mut error) = self.error.lock() {
            *error = text.to_string();
        }
    }

    fn drop_pipe(&self) {
        if let Ok(mut slot) = self.writer.lock() {
            *slot = None;
        }
    }

    fn read_loop(&self, mut reader: Handle) {
        let mut header = [0u8; 8];
        while self.run.load(Ordering::Relaxed) {
            if !read_exact(&mut reader, &mut header) {
                return;
            }
            let op = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
            let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
            if length == 0 || length > 1024 * 1024 {
                return;
            }

            let mut payload = vec![0u8; length];
            if !read_exact(&mut reader, &mut payload) {
                return;
            }

            let Ok(node) = serde_json::from_slice::<Value>(&payload) else {
                continue;
            };

            if op == OP_CLOSE {
                return;
            }
            if op != OP_FRAME {
                continue;
            }

            match node.get("evt").and_then(Value::as_str).unwrap_or("") {
                "READY" => {
                    let name = node
                        .pointer("/data/user/username")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if let Ok(mut user) = self.user.lock() {
                        *user = name;
                    }
                    self.set_error("");
                    self.connected.store(true, Ordering::Relaxed);
                }
                "ERROR" => {
                    let message = node
                        .pointer("/data/message")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let message = if message.is_empty() { "fout" } else { message };
                    self.set_error(&format!("Discord: {message}"));
                }
                _ => {}
            }
        }
    }

    fn write_frame(&self, payload: &Value, op: u32) -> bool {
        let json = payload.to_string();
        let mut frame = Vec::with_capacity(8 + json.len());
        frame.extend_from_slice(&op.to_le_bytes());
        frame.extend_from_slice(&(json.len() as u32).to_le_bytes());
        frame.extend_from_slice(json.as_bytes());

        let mut slot = match self.writer.lock() {
            Ok(slot) => slot,
            Err(_) => return false,
        };
        let Some(handle) = slot.as_mut() else {
            return false;
        };

        let written = handle.write_all(&frame).and_then(|_| handle.flush());
        if let Err(e) = written {
            self.set_error(&format!("Discord schrijffout: {e}"));
            *slot = None;
            return false;
        }
        true
    }

    /// Pushes an activity, or clears it when `activity` is `None`.
    pub fn set_activity(&self, activity: Option<&Value>) -> bool {
        if !self.connected() {
            return false;
        }

        // Copy through serialisation, so the same activity can be sent again after a reconnect.
        let mut args = serde_json::Map::new();
        args.insert("pid".into(), json!(std::process::id()));
        if let Some(activity) = activity {
            args.insert("activity".into(), activity.clone());
        }

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        self.write_frame(
            &json!({
                "cmd": "SET_ACTIVITY",
                "args": Value::Object(args),
                "nonce": format!("{nonce:x}"),
            }),
            OP_FRAME,
        )
    }
}

fn read_exact(handle: &mut Handle, buffer: &mut [u8]) -> bool {
    let mut read = 0;
    while read < buffer.len() {
        match handle.read(&mut buffer[read..]) {
            Ok(0) => return false,
            Ok(n) => read += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return false,
        }
    }
    true
}

fn open_pipe() -> Option<(Handle, Handle)> {
    for index in 0..10 {
        if let Some(handle) = connect(index) {
            if let Ok(clone) = handle.try_clone() {
                #[cfg(unix)]
                {
                    let _ = handle.set_write_timeout(Some(WRITE_TIMEOUT));
                    let _ = clone.set_read_timeout(Some(Duration::from_secs(3600)));
                }
                return Some((handle, clone));
            }
        }
    }
    None
}

#[cfg(unix)]
fn connect(index: u32) -> Option<Handle> {
    use std::path::PathBuf;

    let mut dirs: Vec<PathBuf> = Vec::new();
    for var in ["XDG_RUNTIME_DIR", "TMPDIR"] {
        if let Ok(value) = std::env::var(var) {
            if !value.is_empty() {
                dirs.push(PathBuf::from(&value));
                // Snap and Flatpak installs put the socket one directory deeper.
                dirs.push(PathBuf::from(&value).join("snap.discord"));
                dirs.push(PathBuf::from(&value).join("app/com.discordapp.Discord"));
            }
        }
    }
    dirs.push(PathBuf::from("/tmp"));

    for dir in dirs {
        let path = dir.join(format!("discord-ipc-{index}"));
        if let Ok(stream) = std::os::unix::net::UnixStream::connect(&path) {
            return Some(stream);
        }
    }
    None
}

#[cfg(windows)]
fn connect(index: u32) -> Option<Handle> {
    let path = format!(r"\\.\pipe\discord-ipc-{index}");
    std::fs::OpenOptions::new().read(true).write(true).open(path).ok()
}
