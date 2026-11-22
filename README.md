# NeoPresence

Show what you are playing on your **Discord profile** — driven from your Android phone instead of
from the PC. NeoPresence reads the media session on your phone (VLC or any other music app),
sends it to the PC over your wifi, and the PC pushes it to Discord as rich presence.

The PC side is a small Windows tray app; the phone side is an Android app that pairs with it by
scanning a QR code.

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
                    └───────────────────────────►│  SettingsForm (tray icon, QR, token)   │
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
| `pc/` | Windows tray app, C# / WinForms on **.NET 10** (`net10.0-windows`), `QRCoder` for the pairing QR |
| `android/` | Android app, Kotlin, `nl.neofast.presence`, minSdk 26 / targetSdk 35 |
| `installer/` | NSIS 3 script for the per-user Windows installer |

## Configuration

`%APPDATA%\NeoPresence\config.json` (created on first run):

| Key | Default | Meaning |
|---|---|---|
| `Token` | generated | Pairing secret the phone sends as `Authorization: Bearer …` |
| `Port` | `8787` | HTTP port of the PC server |
| `DiscordAppId` | `1562742366731763765` | Discord application whose name/icon the presence uses |
| `PublicUrl` | `https://presence.neofastftldev.nl` | Address the phone uses when it is not on the same wifi |
| `Enabled` | `true` | Master switch (also in the tray menu) |
| `StartHidden` | `true` | Start in the tray instead of showing the window |
| `AutoStart` | `false` | Register the app under `HKCU\…\Run` |
| `PauseClearSeconds` | `60` | How long a pause is tolerated before presence is cleared |
| `LargeImage` | `""` | Optional large image asset key |

Run `NeoPresence.exe --show` to open the settings window immediately; running it twice is a no-op
(single-instance mutex).

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

Everything below was verified on CachyOS with **wine-11.22**, the .NET **10.0.404** and **8.0.428**
SDKs (Linux under `~/.dotnet`, Windows inside the prefix `~/.wine-dotnet`), and JDK 21 + the Linux
Android SDK 35 in `~/Android/Sdk` for the phone app.

### 1. PC app (Windows or Linux cross-build)

On Windows:

```powershell
dotnet publish pc/NeoPresence.csproj -c Release -r win-x64 --self-contained true `
  -p:PublishSingleFile=true -o pc/publish
```

On Linux, the same command needs the Windows Desktop targeting pack:

```bash
~/.dotnet/dotnet publish pc/NeoPresence.csproj -c Release -r win-x64 --self-contained true \
  -p:PublishSingleFile=true -p:EnableWindowsTargeting=true -o pc/publish
```

To reproduce the exact shipping exe here, the build runs through the **Windows** .NET SDK inside a
Wine prefix instead (`wine C:\dotnet\dotnet.exe publish …`), because Wine cannot verify signed
NuGet packages and the restore therefore has to be prepared by the Linux SDK first. `./build-wine.sh`
in this repository runs those three steps and writes `pc/dist/wine-x64/NeoPresence.exe`;
`./build-wine.sh --dry-run` prints them without running anything. The two commands above are the
portable equivalents if you would rather not use the script.

### 2. Installer

```bash
makensis /INPUTCHARSET UTF8 installer/NeoPresence.nsi   # → dist/NeoPresence-Setup-1.0.0.exe
```

It expects `pc/publish/NeoPresence.exe`, installs per-user into
`%LOCALAPPDATA%\Programs\NeoPresence` and leaves `%APPDATA%\NeoPresence` alone on uninstall.
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

### 4. Running it under Wine (Linux)

`./run-wine.sh` launches `pc/dist/wine-x64/NeoPresence.exe` in `~/.wine-apps` (override with
`WINEPREFIX=…`) and warns when the bridge below is missing.

NeoPresence talks to Discord over the Windows named pipe `\\.\pipe\discord-ipc-0`. Wine apps
cannot reach Discord's unix socket themselves, so on Linux you also need the companion project
**discord-pipe-bridge** (a separate repository, <https://github.com/NeofastFTL-Dev/discord-pipe-bridge>):
build it, drop both files into
`C:\windows` of the prefix, start it with `wine C:\windows\discord-pipe-bridge.exe`, then start
NeoPresence in the same prefix. Without the bridge the app still runs, but stays
“Discord niet verbonden”.

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

* **“Discord is niet open”** — Discord is not running, or (on Linux) the pipe bridge is missing.
* **Phone cannot reach the PC** — same wifi? Windows Firewall rule for port 8787 and UDP 47801?
  Otherwise set `PublicUrl` to a tunnel address.
* **Nothing shows while music plays** — notification access must be granted to NeoPresence, and on
  Android 13+ media notifications have to be visible.
* Errors are appended to `%APPDATA%\NeoPresence\error.log`.

## Credits

* QR codes: [QRCoder](https://github.com/codebude/QRCoder) (MIT).
* Phone QR scanner: [zxing-android-embedded](https://github.com/journeyapps/zxing-android-embedded)
  (Apache-2.0) and [AndroidX core](https://developer.android.com/jetpack/androidx) (Apache-2.0).
* Discord IPC implemented directly from the documented handshake — no third-party RPC library.
* App icon and the Linux desktop icon were drawn for this project.

## License

MIT - see [LICENSE](LICENSE), covering both the Android app and the PC tray app.
