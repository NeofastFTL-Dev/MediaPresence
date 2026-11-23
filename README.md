# MediaPresence

Show what you are playing on your **Discord profile** — driven from your Android phone instead of
from the PC. MediaPresence reads the media session on your phone (VLC or any other music app),
sends it to the PC over your wifi, and the PC pushes it to Discord as rich presence.

The PC side is a small desktop app written in Rust with egui; the phone side is an Android app
that pairs with it by scanning a QR code.

> The Windows UI is in Dutch, the code and this README are in English.

## How it works

```
┌─────────────── Android ───────────────┐        ┌──────────────── Windows ───────────────┐
│ MediaListener (notification listener) │        │  Server (HTTP :8787 + UDP discovery)   │
│   title / artist / album / position   │  POST  │        │                              │
│   ────────────────────────────────────┼───────►│  PresenceManager                       │
│  Sender (Bearer token from pairing)   │ /update│        │                              │
└───────────────────┬───────────────────┘        │  DiscordIpc ──► \\.\pipe\discord-ipc-0 │
                    │  QR / neopresence://pair   │        │              (Discord RPC)     │
                    └───────────────────────────►│  Settings window (QR, token, status)   │
                                                 └────────────────────────────────────────┘
```

* The phone keeps a **notification listener** alive so it sees the media session of whichever app
  is playing; tags are read when file access was granted, otherwise the file name is used.
* It POSTs a normalised payload to the PC (`/update`) with a **pairing token** of at least 32
  characters. The token is created by the PC and handed over once, by QR code or by typing it.
* `PresenceManager` debounces the updates and `DiscordIpc` speaks Discord's IPC protocol directly
  (handshake v1 + `SET_ACTIVITY`), so there is no Discord RPC NuGet dependency.
* When nothing is playing, presence is cleared after `PauseClearSeconds` (default 60 s).

## Repository layout

| Path | What |
|---|---|
| `pc/` | Desktop app, **Rust** + `eframe`/`egui` on the `glow` backend, `qrcode` for the pairing QR |
| `android/` | Android app, Kotlin, `nl.neofast.presence`, minSdk 26 / targetSdk 35 |
| `installer/` | NSIS 3 script for the per-user Windows installer |

## Configuration

`config.json` (created on first run): `%APPDATA%\MediaPresence\` on Windows,
`$XDG_CONFIG_HOME/mediapresence/` (usually `~/.config/mediapresence/`) elsewhere. Before the rename this lived in
`%APPDATA%\NeoPresence`; that folder is moved across on the first start, so the pairing token and
the Discord settings survive. Two identities keep their old value on purpose, because they are
install/pairing keys rather than labels: the Android application id `nl.neofast.presence` and the
`neopresence://` pairing scheme (plus the `NEOPRESENCE_*` UDP discovery strings).

| Key | Default | Meaning |
|---|---|---|
| `Token` | generated | Pairing secret the phone sends as `Authorization: Bearer …` |
| `Port` | `8787` | HTTP port of the PC server |
| `DiscordAppId` | `1562742366731763765` | Discord application whose name/icon the presence uses |
| `PublicUrl` | `https://presence.neofastftldev.nl` | Address the phone uses when it is not on the same wifi |
| `Enabled` | `true` | Master switch (checkbox in the status section) |
| `StartHidden` | `true` | Start minimised instead of opening the window |
| `AutoStart` | `false` | Start at login (`HKCU\…\Run` on Windows, `~/.config/autostart` elsewhere) |
| `PauseClearSeconds` | `60` | How long a pause is tolerated before presence is cleared |
| `LargeImage` | `""` | Optional large image asset key |

Run `mediapresence --show` to open the settings window immediately. The app is a normal window
now: closing it quits, and `StartHidden` only means it opens minimised.

## HTTP API (what the phone calls)

| Method | Path | Notes |
|---|---|---|
| `GET` | `/status`, `/health` | `{"ok":true,"enabled":true,"port":8787,"lastError":""}` — no auth |
| `POST` | `/update` | Requires `Authorization: Bearer <token>`, JSON body below, max 200 requests / 10 s |

```json
{ "details": "{title}", "state": "{artist}", "playing": true,
  "duration": 214000, "position": 51000, "type": 2,
  "buttons": [{ "label": "Open in VLC", "url": "https://…" }] }
```

Send `{"stopped": true}` to clear the activity. `details`/`state` of 128 chars are truncated by
Discord; both empty is ignored. Discovery uses UDP port `47801` so the phone can find the PC on
the local network without knowing its IP.

## Building on this machine

JDK 21 + the Linux Android SDK 35 in `~/Android/Sdk` are needed for the phone app; the PC app needs
only a Rust toolchain.

### 1. PC app

```bash
cd pc
cargo build --release     # → pc/target/release/mediapresence
cargo test                # config parsing tests
cargo run --release -- --selftest
```

`--selftest` is what CI runs: it checks the config normalisation, the pairing link and QR matrix,
the presence payload builder (clipping, buttons, timestamps, pause and stop handling) and drives a
real HTTP round trip against the server on an ephemeral port, including the 401/400/404/413/429
paths and the UDP discovery answer. It needs no display and no Discord.

Linux builds need the usual egui/winit libraries (`libxkbcommon-dev`, `libwayland-dev`, the
`libxcb-*` and X11 dev packages, `libgl1-mesa-dev`); on Windows the toolchain is enough.

### 2. Installer

```bash
makensis /INPUTCHARSET UTF8 installer/MediaPresence.nsi   # → dist/MediaPresence-Setup-1.0.0.exe
```

It expects `pc/target/release/mediapresence.exe`, installs per-user into
`%LOCALAPPDATA%\Programs\MediaPresence` and leaves `%APPDATA%\MediaPresence` alone on uninstall.
`makensis` is **not** installed on this Linux box, so run this step on Windows (or install NSIS).

### 3. Android app

```bash
cd android
./gradlew assembleRelease     # → app/build/outputs/apk/release/app-release.apk
```

* Needs a JDK (21 works) and a **Linux** Android SDK with platform 35, pointed at by `sdk.dir` in
  `android/local.properties`. This machine has one at `~/Android/Sdk` (cmdline-tools,
  `platforms;android-35`, `build-tools;34.0.0`). The SDK inside the Windows profile on the mounted
  NTFS drive only contains `.exe`/`.bat` binaries, and Gradle rejects it as "corrupted" — do not
  point `sdk.dir` there.
* Release signing reads `android/keystore.properties`:

  ```properties
  storeFile=keystore/neopresence.jks
  storePassword=…
  keyAlias=…
  keyPassword=…
  ```

  The keystore and that properties file are **deliberately not in the repo** (`.gitignore`).
  Without them the release build stays unsigned and cannot be installed.

## Screenshots

Screenshots belong in `docs/screenshots/`. To keep the repository honest, the images are not
committed yet — capture these two and the section is ready:

| File to add | What to capture |
|---|---|
| `docs/screenshots/pc-settings.png` | Settings window showing the QR code and token with the phone paired |
| `docs/screenshots/android.png` | Phone screen with a track playing and “Gekoppeld” visible |
| `docs/screenshots/discord.png` | Discord profile card with the presence while a song plays |

Blur or regenerate the pairing token before publishing a screenshot — a visible token lets anyone
push presence to that PC.

```html
<!-- Uncomment once the files exist:
![PC settings window](docs/screenshots/pc-settings.png)
![Android app](docs/screenshots/android.png)
![Discord profile card](docs/screenshots/discord.png)
-->
```

## Troubleshooting

* **“Discord is niet open”** — Discord is not running. The app looks for the Discord socket
  (`$XDG_RUNTIME_DIR/discord-ipc-0`, snap/flatpak paths, `/tmp`) or the Windows named pipe itself,
  and retries every five seconds, so starting Discord afterwards is enough.
* **Phone cannot reach the PC** — same wifi? Windows Firewall rule for port 8787 and UDP 47801?
  Otherwise set `PublicUrl` to a tunnel address.
* **Nothing shows while music plays** — notification access must be granted to MediaPresence, and on
  Android 13+ media notifications have to be visible.
* Faults are appended to `error.log` in the config folder (panic messages and server bind
  failures); the status rows show the current ones live.

## Credits

* QR codes: [qrcode](https://github.com/kennytm/qrcode-rust) (MIT/Apache-2.0).
* Phone QR scanner: [zxing-android-embedded](https://github.com/journeyapps/zxing-android-embedded)
  (Apache-2.0) and [AndroidX core](https://developer.android.com/jetpack/androidx) (Apache-2.0).
* Discord IPC implemented directly from the documented handshake — no third-party RPC library.
* App icon and the Linux desktop icon were drawn for this project.

## License

MIT - see [LICENSE](LICENSE), covering both the Android app and the PC app.
