//! MediaPresence for the PC: receives what your phone is playing over HTTP, pushes it to Discord
//! as rich presence, and offers the pairing QR code. Rust + egui, same stack as my other apps.
//!
//! `--selftest` runs the checks CI uses: config normalisation, the pairing link, the presence
//! builder, a real HTTP round trip against the server and the UDP discovery answer.

mod config;
mod discord;
mod presence;
mod qr;
mod server;

use config::Config;
use discord::DiscordIpc;
use eframe::egui;
use presence::Presence;
use serde_json::json;
use server::Server;
use std::sync::Arc;
use std::time::{Duration, Instant};

const ORANGE: egui::Color32 = egui::Color32::from_rgb(255, 136, 0);
const BACK: egui::Color32 = egui::Color32::from_rgb(18, 18, 24);
const MUTED: egui::Color32 = egui::Color32::from_rgb(170, 170, 185);
const FIELD: egui::Color32 = egui::Color32::from_rgb(43, 45, 49);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--selftest") {
        std::process::exit(selftest());
    }

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        config::log_error("panic", &info.to_string());
        default_hook(info);
    }));

    let force_show = args.iter().any(|arg| arg == "--show");
    let mut cfg = Config::load();
    cfg.save();

    let ipc = DiscordIpc::new(cfg.discord_app_id.clone());
    ipc.start();

    let presence = Presence::new(cfg.enabled, cfg.pause_clear_seconds, cfg.large_image.clone());
    presence.start(Arc::clone(&ipc));

    let server = Server::new(Arc::clone(&presence), &cfg);
    server.start(&cfg);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([560.0, 820.0])
            .with_min_inner_size([520.0, 560.0])
            .with_title("MediaPresence"),
        ..Default::default()
    };

    let start_minimized = cfg.start_hidden && !force_show;
    let result = eframe::run_native(
        "MediaPresence",
        options,
        Box::new(move |cc| {
            Ok(Box::new(App::new(cc, cfg, ipc, presence, server, start_minimized)))
        }),
    );

    if let Err(error) = result {
        eprintln!("MediaPresence: {error}");
    }
}

struct App {
    cfg: Config,
    ipc: Arc<DiscordIpc>,
    presence: Arc<Presence>,
    server: Arc<Server>,

    public_url: String,
    port: i64,
    app_id: String,
    large_image: String,
    pause_clear: i64,
    auto_start: bool,
    enabled: bool,

    qr_texture: Option<egui::TextureHandle>,
    qr_text: String,
    toast: Option<(String, Instant)>,
    start_minimized: bool,
}

impl App {
    fn new(
        cc: &eframe::CreationContext<'_>,
        cfg: Config,
        ipc: Arc<DiscordIpc>,
        presence: Arc<Presence>,
        server: Arc<Server>,
        start_minimized: bool,
    ) -> Self {
        let mut style = (*cc.egui_ctx.style()).clone();
        style.visuals = egui::Visuals::dark();
        style.visuals.panel_fill = BACK;
        style.visuals.extreme_bg_color = FIELD;
        style.visuals.widgets.inactive.bg_fill = FIELD;
        style.visuals.widgets.inactive.fg_stroke.color = egui::Color32::WHITE;
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        cc.egui_ctx.set_style(style);

        let mut app = Self {
            public_url: cfg.public_url.clone(),
            port: cfg.port,
            app_id: cfg.discord_app_id.clone(),
            large_image: cfg.large_image.clone(),
            pause_clear: cfg.pause_clear_seconds,
            auto_start: cfg.auto_start,
            enabled: cfg.enabled,
            cfg,
            ipc,
            presence,
            server,
            qr_texture: None,
            qr_text: String::new(),
            toast: None,
            start_minimized,
        };
        app.rebuild_qr(&cc.egui_ctx);
        app
    }

    fn rebuild_qr(&mut self, ctx: &egui::Context) {
        let uri = self.cfg.pair_uri();
        if uri == self.qr_text && self.qr_texture.is_some() {
            return;
        }

        match qr::matrix(&uri) {
            Ok((width, modules)) => {
                const SCALE: usize = 4;
                const QUIET: usize = 4;
                let size = (width + QUIET * 2) * SCALE;
                let mut image = egui::ColorImage::new([size, size], egui::Color32::WHITE);
                for y in 0..width {
                    for x in 0..width {
                        if !modules[y * width + x] {
                            continue;
                        }
                        for dy in 0..SCALE {
                            for dx in 0..SCALE {
                                let px = (x + QUIET) * SCALE + dx;
                                let py = (y + QUIET) * SCALE + dy;
                                image.pixels[py * size + px] = egui::Color32::BLACK;
                            }
                        }
                    }
                }
                self.qr_texture = Some(ctx.load_texture("pair-qr", image, egui::TextureOptions::NEAREST));
                self.qr_text = uri;
            }
            Err(error) => {
                self.qr_texture = None;
                self.toast(error);
            }
        }
    }

    fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now() + Duration::from_secs(6)));
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.start_minimized {
            self.start_minimized = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }

        // The status rows read live state from the worker threads, so refresh once per second.
        ctx.request_repaint_after(Duration::from_millis(500));
        self.rebuild_qr(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(BACK).inner_margin(egui::Margin::symmetric(18, 12)))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.label(egui::RichText::new("MediaPresence").size(26.0).strong().color(ORANGE));
                    ui.label(
                        egui::RichText::new("Laat zien wat je in VLC luistert op je Discord-profiel.")
                            .color(MUTED),
                    );

                    self.status_section(ui);
                    self.pairing_section(ui, ctx);
                    self.connection_section(ui);
                    self.discord_section(ui);

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Opslaan").clicked() {
                            self.save_clicked(ctx);
                        }
                        if ui.button("Configuratiemap openen").clicked() {
                            open_folder();
                        }
                        if ui.button("Sluiten").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("Koppelcode en instellingen staan in de configuratiemap.")
                            .small()
                            .color(MUTED),
                    );
                });
            });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.ipc.stop();
        self.server.stop();
    }
}

impl App {
    fn save_clicked(&mut self, ctx: &egui::Context) {
        let url = self.public_url.trim().trim_end_matches('/').to_string();
        if !url.to_ascii_lowercase().starts_with("https://") {
            self.toast(
                "Het openbare adres moet met https:// beginnen, bijvoorbeeld https://presence.example.nl",
            );
            return;
        }

        self.cfg.public_url = url.clone();
        self.public_url = url;
        self.cfg.port = self.port.clamp(1024, 65535);
        self.port = self.cfg.port;
        self.cfg.discord_app_id = self.app_id.trim().to_string();
        self.app_id = self.cfg.discord_app_id.clone();
        self.cfg.large_image = self.large_image.trim().to_string();
        self.large_image = self.cfg.large_image.clone();
        self.cfg.pause_clear_seconds = self.pause_clear.clamp(0, 3600);
        self.pause_clear = self.cfg.pause_clear_seconds;
        self.cfg.enabled = self.enabled;
        self.cfg.auto_start = self.auto_start;

        config::apply_auto_start(self.cfg.auto_start);
        self.cfg.save();
        self.server.restart(&self.cfg);
        self.presence
            .set_options(self.cfg.enabled, self.cfg.pause_clear_seconds, self.cfg.large_image.clone());
        self.ipc.set_app_id(&self.cfg.discord_app_id);

        self.qr_text.clear();
        self.rebuild_qr(ctx);

        let message = if self.cfg.last_error.is_empty() {
            "Instellingen opgeslagen".to_string()
        } else {
            self.cfg.last_error.clone()
        };
        self.toast(message);
    }

    fn status_section(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.label(egui::RichText::new("Status").strong().color(ORANGE));

        let discord = if self.ipc.connected() {
            let name = self.ipc.user_name();
            if name.is_empty() {
                "Verbonden".to_string()
            } else {
                format!("Verbonden als {name}")
            }
        } else {
            let error = self.ipc.last_error();
            if error.is_empty() {
                "Niet verbonden".to_string()
            } else {
                format!("Niet verbonden ({error})")
            }
        };

        let mut server_text = self.server.status();
        let server_error = self.server.last_error();
        if !server_error.is_empty() {
            server_text.push_str(" - ");
            server_text.push_str(&server_error);
        }
        server_text.push_str(&format!(" (wifi: {})", config::lan_ip()));

        let now_playing = self.presence.now_playing();
        let now = if now_playing.is_empty() { "Niets".to_string() } else { now_playing };

        let last = match &self.toast {
            Some((text, until)) if Instant::now() < *until => text.clone(),
            _ => self.presence.status(),
        };

        row(ui, "Discord", &discord);
        row(ui, "Server", &server_text);
        row(ui, "Nu", &now);
        row(ui, "Laatste update", &last);

        ui.horizontal(|ui| {
            if ui.checkbox(&mut self.enabled, "Status op Discord tonen").changed() {
                self.cfg.enabled = self.enabled;
                self.cfg.save();
                self.presence.set_options(self.enabled, self.cfg.pause_clear_seconds, self.cfg.large_image.clone());
                if self.enabled {
                    self.presence.refresh();
                } else {
                    self.presence.clear();
                }
            }
        });
    }

    fn pairing_section(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.add_space(10.0);
        ui.label(egui::RichText::new("Koppelen met je telefoon").strong().color(ORANGE));
        ui.label(
            egui::RichText::new(
                "Scan deze code met de knop \"QR-code van je pc scannen\" in de MediaPresence-app op je telefoon.",
            )
            .color(MUTED),
        );

        if let Some(texture) = &self.qr_texture {
            let size = egui::vec2(220.0, 220.0);
            let button = egui::ImageButton::new(egui::load::SizedTexture::new(texture.id(), size))
                .frame(false);
            if ui
                .add(button)
                .on_hover_text("Klik om de koppel-link te kopiëren")
                .clicked()
            {
                ctx.copy_text(self.cfg.pair_uri());
                self.toast("Koppel-link gekopieerd");
            }
        }

        ui.label(egui::RichText::new("Koppelcode").color(MUTED));
        ui.monospace(&self.cfg.token);

        if ui.button("Kopieer koppelcode").clicked() {
            ctx.copy_text(self.cfg.token.clone());
            self.toast("Koppelcode gekopieerd");
        }

        ui.label(
            egui::RichText::new("Koppel-link (als scannen niet lukt, plak dit in de app)").color(MUTED),
        );
        ui.monospace(self.cfg.pair_uri());

        if ui.button("Kopieer koppel-link").clicked() {
            ctx.copy_text(self.cfg.pair_uri());
            self.toast("Koppel-link gekopieerd");
        }

        if ui
            .button("Nieuwe koppelcode")
            .on_hover_text("De huidige koppeling met je telefoon stopt")
            .clicked()
        {
            self.cfg.token = config::new_token();
            self.cfg.save();
            self.qr_text.clear();
            self.rebuild_qr(ctx);
            self.toast("Nieuwe koppelcode gemaakt - scan opnieuw");
        }
    }

    fn connection_section(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.label(egui::RichText::new("Verbinding").strong().color(ORANGE));
        ui.label(
            egui::RichText::new(
                "Openbare adres (tunnel) waarmee de telefoon je pc bereikt buiten je wifi",
            )
            .color(MUTED),
        );
        ui.add(egui::TextEdit::singleline(&mut self.public_url).desired_width(f32::INFINITY));

        ui.label(egui::RichText::new("Poort op je wifi").color(MUTED));
        ui.add(egui::DragValue::new(&mut self.port).range(1024..=65535));

        ui.checkbox(&mut self.auto_start, "Automatisch starten bij inloggen");
    }

    fn discord_section(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.label(egui::RichText::new("Discord").strong().color(ORANGE));

        ui.label(egui::RichText::new("Application ID van je Discord-app").color(MUTED));
        ui.add(egui::TextEdit::singleline(&mut self.app_id).desired_width(f32::INFINITY));

        ui.label(egui::RichText::new("Grote afbeelding (assetsleutel)").color(MUTED));
        ui.add(
            egui::TextEdit::singleline(&mut self.large_image)
                .hint_text("leeg = geen grote afbeelding")
                .desired_width(f32::INFINITY),
        );

        ui.label(
            egui::RichText::new("Seconden wachten met wissen nadat je pauzeert (0 = nooit wissen)")
                .color(MUTED),
        );
        ui.add(egui::DragValue::new(&mut self.pause_clear).range(0..=3600));
    }
}

fn row(ui: &mut egui::Ui, name: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [110.0, 18.0],
            egui::Label::new(egui::RichText::new(format!("{name}:")).color(MUTED)),
        );
        ui.add(egui::Label::new(value).wrap());
    });
}

fn open_folder() {
    let dir = config::data_dir();
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(dir).spawn();
    #[cfg(not(windows))]
    let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
}

fn selftest() -> i32 {
    let mut checks: Vec<(bool, String)> = Vec::new();
    macro_rules! check {
        ($cond:expr, $name:expr) => {
            checks.push(($cond, $name.to_string()))
        };
    }

    // ---- config ----
    let mut broken = Config {
        token: "kort".into(),
        port: 80,
        public_url: "http://example.nl/".into(),
        discord_app_id: "niet-numeriek".into(),
        pause_clear_seconds: 9999,
        ..Config::default()
    };
    check!(broken.normalize(), "config repareren");
    check!(broken.token.len() >= 32, "token minstens 32 tekens");
    check!(broken.port == config::DEFAULT_PORT, "poort terug naar 8787");
    check!(broken.public_url == config::DEFAULT_PUBLIC_URL, "https-adres hersteld");
    check!(broken.discord_app_id == config::DEFAULT_APP_ID, "Discord-app-id hersteld");
    check!(broken.pause_clear_seconds == 60, "pauze-wachtijd terug naar 60");

    let mut good = Config::default();
    good.token = "a".repeat(48);
    check!(!good.normalize(), "geldige config blijft staan");

    let mut pair = Config::default();
    pair.token = "ab cd".into();
    let uri = pair.pair_uri();
    check!(uri.starts_with("neopresence://pair?t=ab%20cd&lan="), "koppel-link begint goed");
    check!(uri.contains("&pub=https%3A%2F%2F"), "openbare adres in koppel-link");
    check!(config::lan_ip().split('.').count() == 4, "wifi-adres opgehaald");
    let expected_dir = if cfg!(windows) { "MediaPresence" } else { "mediapresence" };
    check!(config::data_dir().ends_with(expected_dir), "configuratiemap");

    // ---- QR ----
    match qr::matrix(&uri) {
        Ok((width, modules)) => {
            check!(width >= 21, "QR heeft een geldige grootte");
            check!(modules.len() == width * width, "QR-matrix is vierkant");
            check!(modules.iter().any(|dark| *dark), "QR heeft donkere modules");
        }
        Err(_) => check!(false, "QR maken"),
    }

    // ---- presence builder ----
    let payload = json!({
        "playing": true,
        "details": "Nummer",
        "state": "Artiest",
        "duration": 200_000,
        "position": 1_000,
    });
    let presence = Presence::new(true, 60, String::new());
    presence.update(&payload);
    check!(presence.now_playing().starts_with("▶ Nummer"), "nu speelt-regel");
    let desired = presence.desired_json().unwrap_or_default();
    check!(desired.get("details").and_then(|v| v.as_str()) == Some("Nummer"), "details in activiteit");
    check!(desired.get("timestamps").is_some(), "tijdlijn in activiteit");

    let long = json!({ "details": "x".repeat(300), "state": "" });
    presence.update(&long);
    let clipped = presence
        .desired_json()
        .and_then(|value| value.get("details").and_then(|v| v.as_str()).map(str::len))
        .unwrap_or(0);
    check!(clipped == 128, "details afgekapt op 128 tekens");

    let buttons = json!({
        "details": "Lied",
        "buttons": [
            { "label": "Een", "url": "https://example.nl/1" },
            { "label": "Twee", "url": "https://example.nl/2" },
            { "label": "Drie", "url": "https://example.nl/3" },
            { "label": "Fout", "url": "ftp://example.nl" }
        ]
    });
    presence.update(&buttons);
    let count = presence
        .desired_json()
        .and_then(|value| value.get("buttons").and_then(|v| v.as_array()).map(Vec::len))
        .unwrap_or(0);
    check!(count == 2, "hoogstens twee knoppen");

    presence.update(&json!({ "details": "", "state": "" }));
    check!(presence.status() == "Leeg bericht genegeerd", "leeg bericht genegeerd");

    presence.update(&json!({ "playing": false, "details": "Pauze", "state": "Artiest" }));
    check!(
        presence.desired_json().and_then(|v| v.get("timestamps").cloned()).is_none(),
        "geen tijdlijn bij pauze"
    );

    presence.update(&json!({ "stopped": true }));
    check!(presence.now_playing().is_empty(), "stop wist nu speelt");
    check!(presence.desired_json().is_none(), "stop wist activiteit");

    presence.set_options(false, 60, String::new());
    presence.update(&payload);
    check!(presence.status() == "Uitgeschakeld", "uitgeschakeld negeert updates");

    // ---- HTTP server, end to end ----
    let token = "c".repeat(48);
    let Some(port) = server::pick_free_port() else {
        println!("✖ geen vrije poort gevonden");
        return 1;
    };
    let mut cfg = Config {
        token: token.clone(),
        port: port as i64,
        enabled: true,
        ..Config::default()
    };
    cfg.normalize();
    let live_presence = Presence::new(true, 60, String::new());
    let http = Server::new(Arc::clone(&live_presence), &cfg);
    // A real MediaPresence can be running on this machine; use a port of our own.
    let discovery_port = std::net::UdpSocket::bind(("127.0.0.1", 0))
        .and_then(|socket| socket.local_addr())
        .map(|addr| addr.port())
        .unwrap_or(server::DISCOVERY_PORT);
    http.start_on(&cfg, discovery_port);
    std::thread::sleep(Duration::from_millis(200));
    check!(http.status().contains("Luistert"), "server luistert");

    let get = |path: &str| server::request(port, &format!("GET {path} HTTP/1.1\r\nHost: pc\r\nConnection: close\r\n\r\n"));
    let post = |body: &str, auth: Option<&str>| {
        let auth = auth
            .map(|value| format!("Authorization: Bearer {value}\r\n"))
            .unwrap_or_default();
        server::request(
            port,
            &format!(
                "POST /update HTTP/1.1\r\nHost: pc\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
        )
    };

    let status = get("/status");
    check!(
        status.as_ref().map(|(code, body)| *code == 200 && body.contains("\"ok\":true")).unwrap_or(false),
        "GET /status geeft 200"
    );
    check!(get("/bestaatniet").map(|(code, _)| code == 404).unwrap_or(false), "onbekend pad geeft 404");

    check!(post("{\"details\":\"x\"}", None).map(|(code, _)| code == 401).unwrap_or(false), "zonder token 401");
    check!(
        post("{\"details\":\"x\"}", Some("fout")).map(|(code, _)| code == 401).unwrap_or(false),
        "verkeerd token 401"
    );
    check!(
        post("{niet json", Some(&token)).map(|(code, _)| code == 400).unwrap_or(false),
        "kapotte json 400"
    );

    let oversized = server::request(
        port,
        &format!(
            "POST /update HTTP/1.1\r\nHost: pc\r\nAuthorization: Bearer {token}\r\nContent-Length: 200000\r\nConnection: close\r\n\r\n"
        ),
    );
    check!(oversized.map(|(code, _)| code == 413).unwrap_or(false), "te grote body 413");

    check!(
        post("{\"playing\":true,\"details\":\"Liedje\",\"state\":\"Artiest\"}", Some(&token))
            .map(|(code, _)| code == 200)
            .unwrap_or(false),
        "geldige update 200"
    );
    check!(live_presence.now_playing().contains("Liedje"), "server bereikt presence");

    check!(
        post("{\"stopped\":true}", Some(&token)).map(|(code, _)| code == 200).unwrap_or(false),
        "stop-bericht 200"
    );
    check!(live_presence.now_playing().is_empty(), "stop-bericht gewist");

    // ---- UDP discovery ----
    let discovery = std::net::UdpSocket::bind("127.0.0.1:0").ok().and_then(|socket| {
        socket.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
        socket.send_to(b"NEOPRESENCE_DISCOVER", ("127.0.0.1", discovery_port)).ok()?;
        let mut buffer = [0u8; 256];
        let (read, _) = socket.recv_from(&mut buffer).ok()?;
        Some(String::from_utf8_lossy(&buffer[..read]).to_string())
    });
    check!(
        discovery.map(|reply| reply == format!("NEOPRESENCE:{port}")).unwrap_or(false),
        "wifi-detectie antwoordt"
    );

    // ---- rate limit (last: it exhausts the window) ----
    let limited = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut workers = Vec::new();
    for _ in 0..6 {
        let token = token.clone();
        let limited = std::sync::Arc::clone(&limited);
        workers.push(std::thread::spawn(move || {
            for _ in 0..50 {
                if let Some((code, _)) = hammer(port, &token) {
                    if code == 429 {
                        limited.store(true, std::sync::atomic::Ordering::SeqCst);
                        return;
                    }
                }
            }
        }));
    }
    for worker in workers {
        let _ = worker.join();
    }
    check!(limited.load(std::sync::atomic::Ordering::SeqCst), "te veel verzoeken geeft 429");

    http.stop();

    let failed = checks.iter().filter(|(ok, _)| !ok).count();
    for (ok, name) in &checks {
        println!("{} {name}", if *ok { "✔" } else { "✖" });
    }
    if failed == 0 {
        println!("✔ alles goed: {} controle(s) geslaagd", checks.len());
        0
    } else {
        println!("✖ {failed} van {} controle(s) mislukt", checks.len());
        1
    }
}

/// One authorised update request, used by the rate-limit part of the self-test.
fn hammer(port: u16, token: &str) -> Option<(u16, String)> {
    let body = "{\"details\":\"x\"}";
    server::request(
        port,
        &format!(
            "POST /update HTTP/1.1\r\nHost: pc\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
}
