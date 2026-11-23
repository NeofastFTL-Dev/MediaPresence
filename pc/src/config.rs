//! Settings, stored as JSON next to the app's data:
//!
//! - Windows: `%APPDATA%\MediaPresence\config.json`
//! - macOS: `~/Library/Application Support/MediaPresence/config.json`
//! - elsewhere: `$XDG_CONFIG_HOME/mediapresence/config.json` (or `~/.config/...`)
//!
//! The keys stay PascalCase and the file keeps every field the .NET build wrote, so an existing
//! `config.json` is picked up as-is - the pairing token included, which means the phone does not
//! have to be paired again.

use serde::{Deserialize, Serialize};
use std::net::{IpAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const DEFAULT_PUBLIC_URL: &str = "https://presence.neofastftldev.nl";
pub const DEFAULT_APP_ID: &str = "1562742366731763765";
pub const DEFAULT_PORT: i64 = 8787;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "PascalCase", default)]
pub struct Config {
    /// Pairing secret the phone sends as a bearer token. At least 32 characters.
    pub token: String,
    /// Public (tunnel) address of this PC, used by the phone when it is not on the same wifi.
    pub public_url: String,
    pub port: i64,
    pub enabled: bool,
    pub auto_start: bool,
    pub start_hidden: bool,
    pub discord_app_id: String,
    pub large_image: String,
    pub pause_clear_seconds: i64,
    /// Never persisted: only shown in the status rows.
    #[serde(skip)]
    pub last_error: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            token: String::new(),
            public_url: DEFAULT_PUBLIC_URL.to_string(),
            port: DEFAULT_PORT,
            enabled: true,
            auto_start: false,
            start_hidden: true,
            discord_app_id: DEFAULT_APP_ID.to_string(),
            large_image: String::new(),
            pause_clear_seconds: 60,
            last_error: String::new(),
        }
    }
}

/// Folder the app keeps its files in. The old `NeoPresence` folder is moved here on the first
/// start after the rename, so the pairing token and Discord settings survive it.
pub fn data_dir() -> PathBuf {
    let base = app_data_dir();
    let dir = base.join(dir_name());
    for legacy in legacy_names() {
        let old = base.join(legacy);
        if !dir.exists() && old.exists() {
            let _ = std::fs::rename(&old, &dir);
            break;
        }
    }
    dir
}

/// Windows keeps the PascalCase folder the .NET build used; elsewhere the XDG-style lowercase
/// name is the convention.
fn dir_name() -> &'static str {
    if cfg!(windows) || cfg!(target_os = "macos") {
        "MediaPresence"
    } else {
        "mediapresence"
    }
}

fn legacy_names() -> &'static [&'static str] {
    if cfg!(windows) || cfg!(target_os = "macos") {
        &["NeoPresence"]
    } else {
        &["MediaPresence", "NeoPresence"]
    }
}

fn app_data_dir() -> PathBuf {
    if cfg!(windows) {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata);
        }
    } else if cfg!(target_os = "macos") {
        if let Ok(home) = std::env::var("HOME") {
            if !home.is_empty() {
                return PathBuf::from(home).join("Library").join("Application Support");
            }
        }
    } else if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg);
        }
    } else if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".config");
    }
    std::env::temp_dir()
}

/// Appends a line to `error.log` in the config folder. Used for faults that would otherwise only
/// be visible while the window is open.
pub fn log_error(scope: &str, message: &str) {
    use std::io::Write;

    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let line = format!(
        "[{}] {scope}: {message}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    );
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("error.log"))
    {
        let _ = file.write_all(line.as_bytes());
    }
}

pub fn config_path() -> PathBuf {
    data_dir().join("config.json")
}

impl Config {
    pub fn load() -> Config {
        let path = config_path();
        let mut cfg: Config = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        if cfg.normalize() {
            cfg.save();
        }
        cfg
    }

    /// Clamps everything a hand-edited file could get wrong. Returns whether anything changed.
    pub fn normalize(&mut self) -> bool {
        let mut dirty = false;

        self.token = self.token.trim().to_string();
        self.public_url = self.public_url.trim().trim_end_matches('/').to_string();
        self.discord_app_id = self.discord_app_id.trim().to_string();
        self.large_image = self.large_image.trim().to_string();

        if self.token.chars().count() < 32 {
            self.token = new_token();
            dirty = true;
        }
        if !(1024..=65535).contains(&self.port) {
            self.port = DEFAULT_PORT;
            dirty = true;
        }
        if !self.public_url.to_ascii_lowercase().starts_with("https://") {
            self.public_url = DEFAULT_PUBLIC_URL.to_string();
            dirty = true;
        }
        if self.discord_app_id.len() < 15 || !self.discord_app_id.chars().all(|c| c.is_ascii_digit()) {
            self.discord_app_id = DEFAULT_APP_ID.to_string();
            dirty = true;
        }
        if !(0..=3600).contains(&self.pause_clear_seconds) {
            self.pause_clear_seconds = 60;
            dirty = true;
        }

        dirty
    }

    pub fn save(&mut self) {
        let dir = data_dir();
        let result = std::fs::create_dir_all(&dir).and_then(|_| {
            let text = serde_json::to_string_pretty(&*self)?;
            std::fs::write(dir.join("config.json"), text + "\n")
        });
        match result {
            Ok(()) => self.last_error.clear(),
            Err(e) => self.last_error = format!("Opslaan mislukt: {e}"),
        }
    }

    /// Deep link the phone understands: `neopresence://pair?t=..&lan=..&pub=..`
    pub fn pair_uri(&self) -> String {
        format!(
            "neopresence://pair?t={}&lan={}&pub={}",
            percent_encode(&self.token),
            percent_encode(&format!("{}:{}", lan_ip(), self.port)),
            percent_encode(self.public_url.trim_end_matches('/')),
        )
    }
}

pub fn new_token() -> String {
    let mut bytes = [0u8; 24];
    if getrandom::fill(&mut bytes).is_err() {
        // Only if the OS source is unavailable; still better than an empty token.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = ((nanos >> ((i % 16) * 8)) as u8) ^ (i as u8);
        }
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

static LAN_IP: Mutex<Option<(Instant, String)>> = Mutex::new(None);

/// First usable LAN IPv4 address, so the phone can reach us without the internet. Cached for a
/// few seconds because the status view asks for it every second.
pub fn lan_ip() -> String {
    if let Ok(guard) = LAN_IP.lock() {
        if let Some((at, ip)) = guard.as_ref() {
            if at.elapsed() < Duration::from_secs(5) {
                return ip.clone();
            }
        }
    }

    let found = primary_ipv4().unwrap_or_else(|| "127.0.0.1".to_string());
    if let Ok(mut guard) = LAN_IP.lock() {
        *guard = Some((Instant::now(), found.clone()));
    }
    found
}

fn primary_ipv4() -> Option<String> {
    // Asking the routing table which source address it would use is a datagram away and needs
    // no platform interface enumeration.
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_link_local() => Some(v4.to_string()),
        _ => None,
    }
}

/// RFC 3986 percent-encoding, matching what `Uri.EscapeDataString` produced.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Registers the app to start when the user logs in. Best effort, like the .NET version: a
/// failure only means the checkbox did not take effect.
pub fn apply_auto_start(on: bool) {
    if cfg!(windows) {
        let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
        let run_key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
        if on {
            let _ = std::process::Command::new("reg")
                .args(["ADD", run_key, "/v", "MediaPresence", "/t", "REG_SZ", "/d", &format!("\"{exe}\""), "/f"])
                .status();
            let _ = std::process::Command::new("reg")
                .args(["DELETE", run_key, "/v", "NeoPresence", "/f"])
                .status();
        } else {
            for name in ["MediaPresence", "NeoPresence"] {
                let _ = std::process::Command::new("reg")
                    .args(["DELETE", run_key, "/v", name, "/f"])
                    .status();
            }
        }
        return;
    }

    if cfg!(target_os = "macos") {
        let Some(home) = std::env::var("HOME").ok().filter(|home| !home.is_empty()) else {
            return;
        };
        let dir = PathBuf::from(home).join("Library").join("LaunchAgents");
        let entry = dir.join("com.mediapresence.app.plist");

        if on {
            let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
            let _ = std::fs::create_dir_all(&dir);
            let text = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n<dict>\n\
  <key>Label</key><string>com.mediapresence.app</string>\n\
  <key>ProgramArguments</key><array><string>{exe}</string></array>\n\
  <key>RunAtLoad</key><true/>\n\
</dict>\n</plist>\n"
            );
            let _ = std::fs::write(&entry, text);
        } else {
            let _ = std::fs::remove_file(&entry);
        }
        return;
    }

    let Some(config) = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|xdg| !xdg.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var("HOME").ok().map(|home| PathBuf::from(home).join(".config")))
    else {
        return;
    };

    let entry = config.join("autostart").join("mediapresence.desktop");
    if on {
        let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
        if let Some(parent) = entry.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let text = format!(
            "[Desktop Entry]\nType=Application\nName=MediaPresence\nExec={exe}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
        );
        let _ = std::fs::write(&entry, text);
    } else {
        let _ = std::fs::remove_file(&entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_broken_values() {
        let mut cfg = Config {
            token: "short".into(),
            port: 99,
            public_url: "http://example.com/".into(),
            discord_app_id: "abc".into(),
            pause_clear_seconds: -5,
            ..Config::default()
        };
        assert!(cfg.normalize());
        assert!(cfg.token.len() >= 32);
        assert_eq!(cfg.port, DEFAULT_PORT);
        assert_eq!(cfg.public_url, DEFAULT_PUBLIC_URL);
        assert_eq!(cfg.discord_app_id, DEFAULT_APP_ID);
        assert_eq!(cfg.pause_clear_seconds, 60);
    }

    #[test]
    fn keeps_good_values() {
        let mut cfg = Config::default();
        cfg.token = "a".repeat(48);
        assert!(!cfg.normalize());
        assert_eq!(cfg.token, "a".repeat(48));
    }

    #[test]
    fn pair_uri_is_escaped() {
        let mut cfg = Config::default();
        cfg.token = "ab cd".into();
        cfg.port = 8787;
        let uri = cfg.pair_uri();
        assert!(uri.starts_with("neopresence://pair?t=ab%20cd&lan="));
        assert!(uri.ends_with("&pub=https%3A%2F%2Fpresence.neofastftldev.nl"), "{uri}");
    }
}
