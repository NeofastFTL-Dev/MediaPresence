//! Turns the payloads the phone sends into Discord activities, coalesces bursts (Discord allows
//! only a handful of updates per 20 seconds) and clears when playback stops. Requests only record
//! the newest state; the worker thread does the Discord calls, so a slow Discord can never block
//! the phone.

use crate::discord::DiscordIpc;
use chrono::Local;
use serde_json::{json, Map, Value};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Discord rate limit: keep at least this much between real updates.
const MIN_UPDATE_SPACING: Duration = Duration::from_millis(4000);

#[derive(Default)]
struct State {
    desired: Option<Value>,
    /// What Discord currently shows: `None` = unknown (must be sent again), `Some("")` = cleared.
    applied: Option<String>,
    last_apply: Option<Instant>,
    paused_since: Option<Instant>,
    status: String,
    now_playing: String,
}

pub struct Presence {
    state: Mutex<State>,
    enabled: AtomicBool,
    pause_clear_seconds: AtomicI64,
    large_image: Mutex<String>,
    /// Mirrors the IPC state so the status line can say Discord is not reachable.
    connected: AtomicBool,
}

impl Presence {
    pub fn new(enabled: bool, pause_clear_seconds: i64, large_image: String) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                status: "Nog niets ontvangen".to_string(),
                ..State::default()
            }),
            enabled: AtomicBool::new(enabled),
            pause_clear_seconds: AtomicI64::new(pause_clear_seconds),
            large_image: Mutex::new(large_image),
            connected: AtomicBool::new(false),
        })
    }

    pub fn set_options(&self, enabled: bool, pause_clear_seconds: i64, large_image: String) {
        self.enabled.store(enabled, Ordering::Relaxed);
        self.pause_clear_seconds.store(pause_clear_seconds, Ordering::Relaxed);
        if let Ok(mut image) = self.large_image.lock() {
            *image = large_image;
        }
        self.refresh();
    }

    pub fn status(&self) -> String {
        self.state.lock().map(|s| s.status.clone()).unwrap_or_default()
    }

    pub fn now_playing(&self) -> String {
        self.state.lock().map(|s| s.now_playing.clone()).unwrap_or_default()
    }

    /// The activity that would be sent next; `None` means "clear". Used by the self-test.
    pub fn desired_json(&self) -> Option<Value> {
        self.state.lock().ok().and_then(|state| state.desired.clone())
    }

    /// Handles one payload from the phone. Returns immediately.
    pub fn update(&self, payload: &Value) {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return,
        };

        if payload.get("stopped").and_then(Value::as_bool).unwrap_or(false) {
            state.desired = None;
            state.now_playing.clear();
            state.paused_since = None;
            state.applied = None;
            state.last_apply = None;
            state.status = format!("Gestopt ({})", clock());
            return;
        }

        let playing = payload.get("playing").and_then(Value::as_bool).unwrap_or(true);
        let kind = payload.get("type").and_then(Value::as_i64).unwrap_or(2) as i32;
        let details = text(payload.get("details"));
        let state_text = text(payload.get("state"));
        let duration = number(payload.get("duration"));
        let position = number(payload.get("position"));

        if details.is_empty() && state_text.is_empty() {
            state.status = "Leeg bericht genegeerd".to_string();
            return;
        }

        state.now_playing = format!(
            "{}{}{}",
            if playing { "▶ " } else { "⏸ " },
            details,
            if state_text.is_empty() { String::new() } else { format!("\n{state_text}") }
        );

        if !self.enabled.load(Ordering::Relaxed) {
            state.desired = None;
            state.applied = None;
            state.status = "Uitgeschakeld".to_string();
            return;
        }

        state.desired = self.build(
            kind,
            &details,
            &state_text,
            playing,
            duration,
            position,
            payload.get("buttons").and_then(Value::as_array),
        );
        state.paused_since = if playing {
            None
        } else {
            Some(state.paused_since.unwrap_or_else(Instant::now))
        };
        state.status = format!(
            "Bijgewerkt {}{}",
            clock(),
            if self.connected.load(Ordering::Relaxed) {
                ""
            } else {
                " (Discord niet verbonden)"
            }
        );
    }

    /// Re-applies the current activity after the user changes settings.
    pub fn refresh(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.applied = None;
            state.last_apply = None;
        }
    }

    pub fn clear(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.desired = None;
            state.paused_since = None;
            state.applied = None;
            state.last_apply = None;
        }
    }

    /// Starts the pump that reconciles "what should be shown" with "what Discord has".
    pub fn start(self: &Arc<Self>, ipc: Arc<DiscordIpc>) {
        let this = Arc::clone(self);
        std::thread::Builder::new()
            .name("presence-pump".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_millis(250));
                this.pump(&ipc);
            })
            .ok();
    }

    fn pump(&self, ipc: &DiscordIpc) {
        self.connected.store(ipc.connected(), Ordering::Relaxed);
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return,
        };

        if !ipc.connected() {
            // A fresh Discord connection needs the activity again.
            state.applied = None;
            return;
        }

        if !self.enabled.load(Ordering::Relaxed) {
            state.desired = None;
        }

        let pause_clear = self.pause_clear_seconds.load(Ordering::Relaxed);
        if state.desired.is_some() {
            if let Some(since) = state.paused_since {
                if pause_clear > 0 && since.elapsed() >= Duration::from_secs(pause_clear as u64) {
                    state.desired = None;
                }
            }
        }

        let key = state
            .desired
            .as_ref()
            .map(|value| value.to_string())
            .unwrap_or_default();
        if state.applied.as_deref() == Some(key.as_str()) {
            return;
        }

        if state.desired.is_some() {
            if let Some(last) = state.last_apply {
                if last.elapsed() < MIN_UPDATE_SPACING {
                    return;
                }
            }
        }

        let target = state.desired.clone();
        drop(state); // Discord calls happen outside the lock

        if !ipc.set_activity(target.as_ref()) {
            return;
        }

        if let Ok(mut state) = self.state.lock() {
            let still_current = match (&state.desired, &target) {
                (Some(current), Some(sent)) => current == sent,
                (None, None) => true,
                _ => false,
            };
            if still_current {
                state.applied = Some(key);
            }
            state.last_apply = Some(Instant::now());
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        &self,
        kind: i32,
        details: &str,
        state_text: &str,
        playing: bool,
        duration: i64,
        position: i64,
        buttons: Option<&Vec<Value>>,
    ) -> Option<Value> {
        let mut activity = Map::new();
        activity.insert("type".into(), json!(kind));

        if !details.is_empty() {
            activity.insert("details".into(), json!(clip(details, 128)));
        }
        if !state_text.is_empty() {
            activity.insert("state".into(), json!(clip(state_text, 128)));
        }

        if playing && duration > 0 && position >= 0 {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let start = now - position.min(duration);
            activity.insert(
                "timestamps".into(),
                json!({ "start": start, "end": start + duration }),
            );
        }

        let large_image = self.large_image.lock().map(|image| image.clone()).unwrap_or_default();
        if !large_image.is_empty() {
            let mut assets = Map::new();
            assets.insert("large_image".into(), json!(large_image));
            if !details.is_empty() {
                assets.insert("large_text".into(), json!(clip(details, 128)));
            }
            activity.insert("assets".into(), Value::Object(assets));
        }

        if let Some(buttons) = buttons {
            let mut list = Vec::new();
            for button in buttons {
                if list.len() == 2 {
                    break;
                }
                let label = text(button.get("label"));
                let url = text(button.get("url"));
                if label.is_empty() || !url.to_ascii_lowercase().starts_with("http") {
                    continue;
                }
                list.push(json!({ "label": clip(&label, 32), "url": url }));
            }
            if !list.is_empty() {
                activity.insert("buttons".into(), Value::Array(list));
            }
        }

        if !activity.contains_key("details") && !activity.contains_key("state") {
            return None;
        }
        Some(Value::Object(activity))
    }
}

fn clock() -> String {
    Local::now().format("%H:%M:%S").to_string()
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max).collect()
}

fn text(node: Option<&Value>) -> String {
    node.and_then(Value::as_str).unwrap_or("").to_string()
}

fn number(node: Option<&Value>) -> i64 {
    match node {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}
