using System;
using System.Text.Json.Nodes;

namespace MediaPresence
{
    /// <summary>
    /// Turns the payloads the phone sends into Discord activities, coalesces bursts
    /// (Discord allows only a handful of updates per 20 seconds) and clears when paused.
    /// Requests only record the newest state; the worker thread does the Discord calls,
    /// so a slow or stalled Discord can never block the phone.
    /// </summary>
    public sealed class PresenceManager : IDisposable
    {
        /// <summary>Discord rate limit: keep at least this much between real updates.</summary>
        private const int MinUpdateSpacingMs = 4000;

        private readonly Config _cfg;
        private readonly DiscordIpc _ipc;
        private readonly object _gate = new();
        private readonly System.Threading.Timer _timer;

        private JsonObject? _desired;
        private int _pumping;

        /// <summary>
        /// What Discord currently shows: null = unknown (must be sent again), "" = cleared.
        /// A plain "" default made clearing impossible: the empty key compared equal to the
        /// empty "applied" value, so a stop or disable never removed the presence.
        /// </summary>
        private string? _applied;
        private DateTime _lastApply = DateTime.MinValue;
        private DateTime _pausedSince = DateTime.MinValue;

        private volatile string _status = "Nog niets ontvangen";
        private volatile string _nowPlaying = "";

        public PresenceManager(Config cfg, DiscordIpc ipc)
        {
            _cfg = cfg;
            _ipc = ipc;
            _timer = new System.Threading.Timer(_ => Pump(), null, 250, 250);
        }

        public string Status => _status;
        public string NowPlaying => _nowPlaying;

        public void Dispose() => _timer.Dispose();

        /// <summary>Handles one payload from the phone. Returns immediately.</summary>
        public void Update(JsonObject payload)
        {
            lock (_gate)
            {
                if (Bool(payload["stopped"], false))
                {
                    _desired = null;
                    _nowPlaying = "";
                    _pausedSince = DateTime.MinValue;
                    _applied = null;
                    _lastApply = DateTime.MinValue;
                    _status = "Gestopt (" + DateTime.Now.ToString("HH:mm:ss") + ")";
                    return;
                }

                var playing = Bool(payload["playing"], true);
                var type = Int(payload["type"], 2);
                var details = Str(payload["details"]);
                var state = Str(payload["state"]);
                var duration = Long(payload["duration"]);
                var position = Long(payload["position"]);

                if (details.Length == 0 && state.Length == 0)
                {
                    _status = "Leeg bericht genegeerd";
                    return;
                }

                _nowPlaying = (playing ? "▶ " : "⏸ ") + details +
                              (state.Length > 0 ? Environment.NewLine + state : "");

                if (!_cfg.Enabled)
                {
                    _desired = null;
                    _applied = null;
                    _status = "Uitgeschakeld";
                    return;
                }

                _desired = Build(type, details, state, playing, duration, position, payload["buttons"] as JsonArray);
                _pausedSince = playing ? DateTime.MinValue : (_pausedSince == DateTime.MinValue ? DateTime.Now : _pausedSince);
                _status = "Bijgewerkt " + DateTime.Now.ToString("HH:mm:ss") + (_ipc.Connected ? "" : " (Discord niet verbonden)");
            }
        }

        /// <summary>Re-applies the current activity after the user changes settings.</summary>
        public void Refresh()
        {
            lock (_gate)
            {
                _applied = null;
                _lastApply = DateTime.MinValue;
            }
        }

        public void Clear()
        {
            lock (_gate)
            {
                _desired = null;
                _pausedSince = DateTime.MinValue;
                _applied = null;
                _lastApply = DateTime.MinValue;
            }
        }

        private void Pump()
        {
            // System.Threading.Timer may fire again while a Discord write is still in flight.
            // Two overlapping writes would interleave frames on the same pipe and corrupt it.
            if (Interlocked.CompareExchange(ref _pumping, 1, 0) != 0) return;
            try { PumpCore(); }
            finally { Interlocked.Exchange(ref _pumping, 0); }
        }

        private void PumpCore()
        {
            JsonObject? target;
            string key;

            lock (_gate)
            {
                if (!_ipc.Connected)
                {
                    _applied = null; // a fresh Discord connection needs the activity again
                    return;
                }

                if (!_cfg.Enabled) _desired = null;

                if (_desired != null && _pausedSince != DateTime.MinValue && _cfg.PauseClearSeconds > 0 &&
                    (DateTime.Now - _pausedSince).TotalSeconds >= _cfg.PauseClearSeconds)
                {
                    _desired = null;
                }

                key = _desired?.ToJsonString() ?? "";
                if (key == _applied) return;

                if (_desired != null && (DateTime.Now - _lastApply).TotalMilliseconds < MinUpdateSpacingMs) return;

                target = _desired;
            }

            // Discord calls happen outside the lock, on this worker thread only.
            var ok = _ipc.SetActivity(target);
            if (!ok) return;

            lock (_gate)
            {
                var stillCurrent = ReferenceEquals(target, _desired) || (target == null && _desired == null);
                if (stillCurrent) _applied = key;
                _lastApply = DateTime.Now;
            }
        }

        private JsonObject? Build(int type, string details, string state, bool playing, long duration, long position, JsonArray? buttons)
        {
            var activity = new JsonObject { ["type"] = type };

            if (details.Length > 0) activity["details"] = Clip(details, 128);
            if (state.Length > 0) activity["state"] = Clip(state, 128);

            if (playing && duration > 0 && position >= 0)
            {
                var now = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();
                var start = now - Math.Min(position, duration);
                activity["timestamps"] = new JsonObject { ["start"] = start, ["end"] = start + duration };
            }

            if (_cfg.LargeImage.Length > 0)
            {
                var assets = new JsonObject { ["large_image"] = _cfg.LargeImage };
                if (details.Length > 0) assets["large_text"] = Clip(details, 128);
                activity["assets"] = assets;
            }

            if (buttons != null)
            {
                var arr = new JsonArray();
                foreach (var b in buttons)
                {
                    if (arr.Count == 2) break;
                    var label = Str(b?["label"]);
                    var url = Str(b?["url"]);
                    if (label.Length == 0 || !url.StartsWith("http", StringComparison.OrdinalIgnoreCase)) continue;
                    arr.Add(new JsonObject { ["label"] = Clip(label, 32), ["url"] = url });
                }
                if (arr.Count > 0) activity["buttons"] = arr;
            }

            if (activity["details"] == null && activity["state"] == null) return null;
            return activity;
        }

        private static string Clip(string s, int max) => s.Length <= max ? s : s[..max];

        private static string Str(JsonNode? node)
        {
            if (node is JsonValue value && value.TryGetValue<string>(out var s)) return s ?? "";
            return "";
        }

        private static bool Bool(JsonNode? node, bool fallback)
        {
            if (node is JsonValue value && value.TryGetValue<bool>(out var b)) return b;
            return fallback;
        }

        private static int Int(JsonNode? node, int fallback)
        {
            if (node is JsonValue value && value.TryGetValue<int>(out var i)) return i;
            return fallback;
        }

        private static long Long(JsonNode? node)
        {
            if (node is JsonValue value)
            {
                if (value.TryGetValue<long>(out var l)) return l;
                if (value.TryGetValue<double>(out var d)) return (long)d;
                if (value.TryGetValue<int>(out var i)) return i;
            }
            return 0;
        }
    }
}
