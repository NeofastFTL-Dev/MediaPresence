using System;
using System.IO;
using System.Net;
using System.Net.NetworkInformation;
using System.Net.Sockets;
using System.Security.Cryptography;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace NeoPresence
{
    /// <summary>Settings, stored as JSON in %APPDATA%\NeoPresence\config.json.</summary>
    public sealed class Config
    {
        private static readonly JsonSerializerOptions Json = new() { WriteIndented = true };

        /// <summary>Pairing secret the phone sends as a bearer token. At least 32 characters.</summary>
        public string Token { get; set; } = "";

        /// <summary>Public (tunnel) address of this PC, used by the phone when it is not on the same wifi.</summary>
        public string PublicUrl { get; set; } = "https://presence.neofastftldev.nl";

        public int Port { get; set; } = 8787;
        public bool Enabled { get; set; } = true;
        public bool AutoStart { get; set; }
        public bool StartHidden { get; set; } = true;
        public string DiscordAppId { get; set; } = "1562742366731763765";
        public string LargeImage { get; set; } = "";
        public int PauseClearSeconds { get; set; } = 60;

        [JsonIgnore] public string LastError { get; set; } = "";

        [JsonIgnore]
        public static string Dir =>
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "NeoPresence");

        [JsonIgnore]
        public static string FilePath => Path.Combine(Dir, "config.json");

        public static Config Load()
        {
            Config cfg;
            try
            {
                cfg = File.Exists(FilePath)
                    ? JsonSerializer.Deserialize<Config>(File.ReadAllText(FilePath), Json) ?? new Config()
                    : new Config();
            }
            catch
            {
                cfg = new Config();
            }

            // A hand-edited config.json can hold nulls for any string; normalize first so the
            // checks below can never throw a NullReferenceException at startup.
            cfg.Token = (cfg.Token ?? "").Trim();
            cfg.PublicUrl = (cfg.PublicUrl ?? "").Trim().TrimEnd('/');
            cfg.DiscordAppId = (cfg.DiscordAppId ?? "").Trim();
            cfg.LargeImage = (cfg.LargeImage ?? "").Trim();

            var dirty = false;
            if (cfg.Token.Length < 32) { cfg.Token = NewToken(); dirty = true; }
            if (cfg.Port < 1024 || cfg.Port > 65535) { cfg.Port = 8787; dirty = true; }
            if (!cfg.PublicUrl.StartsWith("https://", StringComparison.OrdinalIgnoreCase))
            {
                cfg.PublicUrl = "https://presence.neofastftldev.nl";
                dirty = true;
            }
            if (cfg.DiscordAppId.Length < 15 || !cfg.DiscordAppId.All(char.IsDigit))
            {
                cfg.DiscordAppId = "1562742366731763765";
                dirty = true;
            }
            if (cfg.PauseClearSeconds < 0 || cfg.PauseClearSeconds > 3600)
            {
                cfg.PauseClearSeconds = 60;
                dirty = true;
            }
            if (dirty) cfg.Save();
            return cfg;
        }

        public void Save()
        {
            try
            {
                Directory.CreateDirectory(Dir);
                File.WriteAllText(FilePath, JsonSerializer.Serialize(this, Json));
                LastError = "";
            }
            catch (Exception e)
            {
                LastError = "Opslaan mislukt: " + e.Message;
            }
        }

        public static string NewToken() =>
            Convert.ToHexString(RandomNumberGenerator.GetBytes(24)).ToLowerInvariant();

        /// <summary>Deep link the phone understands: neopresence://pair?t=..&amp;lan=..&amp;pub=..</summary>
        [JsonIgnore]
        public string PairUri =>
            "neopresence://pair?t=" + Uri.EscapeDataString(Token) +
            "&lan=" + Uri.EscapeDataString(LanIp() + ":" + Port) +
            "&pub=" + Uri.EscapeDataString(PublicUrl.TrimEnd('/'));

        private static string _lanIp = "";
        private static long _lanIpAt;

        /// <summary>First usable LAN IPv4 address, so the phone can reach us without the internet.</summary>
        public static string LanIp()
        {
            // The settings window asks for this every second; enumerating every network
            // interface that often is wasteful, so the answer is cached for a few seconds.
            var now = Environment.TickCount64;
            if (_lanIp.Length > 0 && now - _lanIpAt < 5000) return _lanIp;

            var found = "127.0.0.1";
            try
            {
                foreach (var ni in NetworkInterface.GetAllNetworkInterfaces())
                {
                    if (ni.OperationalStatus != OperationalStatus.Up) continue;
                    if (ni.NetworkInterfaceType == NetworkInterfaceType.Loopback) continue;
                    foreach (var ua in ni.GetIPProperties().UnicastAddresses)
                    {
                        if (ua.Address.AddressFamily != AddressFamily.InterNetwork) continue;
                        if (IPAddress.IsLoopback(ua.Address)) continue;
                        var b = ua.Address.GetAddressBytes();
                        if (b[0] == 169 && b[1] == 254) continue; // link-local
                        found = ua.Address.ToString();
                        break;
                    }
                    if (found != "127.0.0.1") break;
                }
            }
            catch
            {
                // fall through to the loopback address
            }

            _lanIp = found;
            _lanIpAt = now;
            return found;
        }

        public static void ApplyAutoStart(bool on)
        {
            try
            {
                using var key = Microsoft.Win32.Registry.CurrentUser.OpenSubKey(
                    @"Software\Microsoft\Windows\CurrentVersion\Run", writable: true);
                if (key == null) return;
                if (on)
                    key.SetValue("NeoPresence", "\"" + Application_ExecutablePath() + "\"");
                else
                    key.DeleteValue("NeoPresence", throwOnMissingValue: false);
            }
            catch
            {
                // autostart is best effort
            }
        }

        private static string Application_ExecutablePath() => Environment.ProcessPath ?? "";
    }
}
