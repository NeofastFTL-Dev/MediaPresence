using System;
using System.IO;
using System.IO.Pipes;
using System.Text;
using System.Text.Json.Nodes;
using System.Threading;
using System.Threading.Tasks;

namespace MediaPresence
{
    /// <summary>
    /// Minimal Discord RPC client: talks over the local named pipe, exchanges JSON frames
    /// and pushes SET_ACTIVITY. Reconnects on its own when Discord starts or restarts.
    /// </summary>
    public sealed class DiscordIpc : IDisposable
    {
        private const int OpHandshake = 0;
        private const int OpFrame = 1;
        private const int OpClose = 2;

        /// <summary>A stalled Discord must never block us for longer than this.</summary>
        private const int WriteTimeoutMs = 2000;

        private readonly Config _cfg;
        private readonly object _gate = new();
        private NamedPipeClientStream? _pipe;
        private Thread? _thread;
        private volatile bool _run;
        private volatile bool _connected;
        private volatile string _user = "";
        private volatile string _error = "";

        public DiscordIpc(Config cfg) => _cfg = cfg;

        public bool Connected => _connected;
        public string UserName => _user;
        public string LastError => _error;

        public void Start()
        {
            if (_run) return;
            _run = true;
            _thread = new Thread(Loop) { IsBackground = true, Name = "discord-ipc" };
            _thread.Start();
        }

        public void Dispose()
        {
            _run = false;
            _connected = false;
            DropPipe();
        }

        private void Loop()
        {
            while (_run)
            {
                NamedPipeClientStream? pipe = null;
                try
                {
                    pipe = Open();
                    if (pipe == null)
                    {
                        _error = "Discord is niet open";
                        Thread.Sleep(5000);
                        continue;
                    }

                    lock (_gate) _pipe = pipe;

                    if (!WriteFrame(new JsonObject { ["v"] = 1, ["client_id"] = _cfg.DiscordAppId }, OpHandshake))
                    {
                        Thread.Sleep(5000);
                        continue;
                    }

                    ReadLoop(pipe).GetAwaiter().GetResult();
                }
                catch (Exception e)
                {
                    _error = "Discord: " + e.Message;
                }
                finally
                {
                    _connected = false;
                    _user = "";
                    DropPipe();
                    try { pipe?.Dispose(); } catch { }
                }

                if (_run) Thread.Sleep(5000);
            }
        }

        private void DropPipe()
        {
            NamedPipeClientStream? pipe;
            lock (_gate)
            {
                pipe = _pipe;
                _pipe = null;
            }
            try { pipe?.Dispose(); } catch { }
        }

        private static NamedPipeClientStream? Open()
        {
            for (var i = 0; i < 10; i++)
            {
                var pipe = new NamedPipeClientStream(".", "discord-ipc-" + i, PipeDirection.InOut, PipeOptions.Asynchronous);
                try
                {
                    pipe.Connect(200);
                    return pipe;
                }
                catch
                {
                    pipe.Dispose();
                }
            }
            return null;
        }

        private async Task ReadLoop(Stream stream)
        {
            var header = new byte[8];
            while (_run)
            {
                if (!await ReadExact(stream, header, 8).ConfigureAwait(false)) return;

                var op = BitConverter.ToInt32(header, 0);
                var length = BitConverter.ToInt32(header, 4);
                if (length <= 0 || length > 1024 * 1024) return;

                var payload = new byte[length];
                if (!await ReadExact(stream, payload, length).ConfigureAwait(false)) return;

                JsonNode? node;
                try { node = JsonNode.Parse(Encoding.UTF8.GetString(payload)); }
                catch { continue; }

                if (op == OpClose) return;
                if (op != OpFrame) continue;

                var evt = Str(node?["evt"]);
                if (evt == "READY")
                {
                    _user = Str(node?["data"]?["user"]?["username"]);
                    _error = "";
                    _connected = true;
                }
                else if (evt == "ERROR")
                {
                    var message = Str(node?["data"]?["message"]);
                    _error = "Discord: " + (message.Length > 0 ? message : "fout");
                }
            }
        }

        private static async Task<bool> ReadExact(Stream stream, byte[] buffer, int count)
        {
            var read = 0;
            while (read < count)
            {
                int n;
                try
                {
                    n = await stream.ReadAsync(buffer.AsMemory(read, count - read)).ConfigureAwait(false);
                }
                catch
                {
                    return false;
                }
                if (n <= 0) return false;
                read += n;
            }
            return true;
        }

        private bool WriteFrame(JsonObject payload, int op = OpFrame)
        {
            var json = Encoding.UTF8.GetBytes(payload.ToJsonString());
            var frame = new byte[8 + json.Length];
            BitConverter.GetBytes(op).CopyTo(frame, 0);
            BitConverter.GetBytes(json.Length).CopyTo(frame, 4);
            json.CopyTo(frame, 8);

            NamedPipeClientStream? pipe;
            lock (_gate) pipe = _pipe;
            if (pipe == null) return false;

            try
            {
                using var timeout = new CancellationTokenSource(WriteTimeoutMs);
                pipe.WriteAsync(frame, 0, frame.Length, timeout.Token).GetAwaiter().GetResult();
                pipe.FlushAsync(timeout.Token).GetAwaiter().GetResult();
                return true;
            }
            catch (Exception e)
            {
                _error = "Discord schrijffout: " + e.Message;
                DropPipe();
                return false;
            }
        }

        /// <summary>Pushes an activity, or clears it when <paramref name="activity"/> is null.</summary>
        public bool SetActivity(JsonObject? activity)
        {
            if (!_connected) return false;

            var args = new JsonObject { ["pid"] = Environment.ProcessId };
            // Copy, so the same activity object can safely be sent again after a reconnect.
            if (activity != null) args["activity"] = JsonNode.Parse(activity.ToJsonString());

            return WriteFrame(new JsonObject
            {
                ["cmd"] = "SET_ACTIVITY",
                ["args"] = args,
                ["nonce"] = Guid.NewGuid().ToString()
            });
        }

        private static string Str(JsonNode? node)
        {
            if (node is JsonValue value && value.TryGetValue<string>(out var s)) return s ?? "";
            return "";
        }
    }
}
