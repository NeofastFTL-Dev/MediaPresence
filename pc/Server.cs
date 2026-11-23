using System;
using System.Collections.Generic;
using System.Net;
using System.Net.Sockets;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json.Nodes;
using System.Threading;
using System.Threading.Tasks;

namespace MediaPresence
{
    /// <summary>
    /// Tiny HTTP/1.1 server (raw sockets, so it works without an URL ACL or admin rights)
    /// plus the UDP discovery responder the phone uses to find this PC on the wifi.
    /// </summary>
    public sealed class Server : IDisposable
    {
        private const int DiscoveryPort = 47801;

        private readonly Config _cfg;
        private readonly PresenceManager _presence;

        private TcpListener? _listener;
        private UdpClient? _udp;
        private CancellationTokenSource? _cts;

        private long _windowStart;
        private int _windowCount;

        private volatile string _status = "Gestopt";
        private volatile string _lastError = "";

        public Server(Config cfg, PresenceManager presence)
        {
            _cfg = cfg;
            _presence = presence;
        }

        public string Status => _status;
        public string LastError => _lastError;

        public void Restart()
        {
            Stop();
            Start();
        }

        public void Start()
        {
            _cts = new CancellationTokenSource();
            var token = _cts.Token;

            try
            {
                var listener = new TcpListener(IPAddress.Any, _cfg.Port);
                listener.Start();
                _listener = listener;
                _status = "Luistert op poort " + _cfg.Port;
                _lastError = "";
                _ = Task.Run(() => AcceptLoop(listener, token), token);
            }
            catch (Exception e)
            {
                _status = "Server start niet";
                _lastError = "Poort " + _cfg.Port + ": " + e.Message;
                return;
            }

            StartDiscovery(token);
        }

        public void Stop()
        {
            try { _cts?.Cancel(); } catch { }
            try { _listener?.Stop(); } catch { }
            try { _udp?.Dispose(); } catch { }
            _listener = null;
            _udp = null;
            _cts = null;
            _status = "Gestopt";
        }

        public void Dispose() => Stop();

        private async Task AcceptLoop(TcpListener listener, CancellationToken token)
        {
            while (!token.IsCancellationRequested)
            {
                TcpClient client;
                try
                {
                    client = await listener.AcceptTcpClientAsync(token);
                }
                catch (OperationCanceledException)
                {
                    return;
                }
                catch (Exception e)
                {
                    if (token.IsCancellationRequested) return;
                    _lastError = e.Message;
                    try { await Task.Delay(250, token); } catch { return; }
                    continue;
                }

                // No cancellation token here: if it were already cancelled the task would never
                // run and the accepted socket would leak until the GC collected it.
                _ = Task.Run(() => HandleClient(client));
            }
        }

        private async Task HandleClient(TcpClient client)
        {
            using (client)
            {
                try
                {
                    client.ReceiveTimeout = 5000;
                    client.SendTimeout = 5000;
                    var stream = client.GetStream();

                    var headResult = await ReadHead(stream);
                    if (headResult == null) return;
                    var (head, rest) = headResult.Value;

                    var lines = head.Split("\r\n");
                    var requestLine = lines[0].Split(' ');
                    if (requestLine.Length < 2)
                    {
                        await Respond(stream, 400, Error("bad request"));
                        return;
                    }

                    var method = requestLine[0];
                    var path = requestLine[1].Split('?')[0];

                    var headers = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
                    for (var i = 1; i < lines.Length; i++)
                    {
                        var idx = lines[i].IndexOf(':');
                        if (idx <= 0) continue;
                        headers[lines[i][..idx].Trim()] = lines[i][(idx + 1)..].Trim();
                    }

                    if (headers.TryGetValue("Expect", out var expect) &&
                        expect.Contains("100-continue", StringComparison.OrdinalIgnoreCase))
                    {
                        await stream.WriteAsync(Encoding.ASCII.GetBytes("HTTP/1.1 100 Continue\r\n\r\n"));
                    }

                    // Drain the body before answering: unread bytes in the socket turn the
                    // close into a TCP reset, so the phone would see a network error instead
                    // of our 401/429 status.
                    var body = await ReadBody(stream, headers, rest);
                    if (body == null)
                    {
                        await Respond(stream, 413, Error("payload too large"));
                        return;
                    }

                    if (method == "GET" && (path == "/status" || path == "/health"))
                    {
                        await Respond(stream, 200, StatusJson().ToJsonString());
                        return;
                    }

                    if (method != "POST" || path != "/update")
                    {
                        await Respond(stream, 404, Error("not found"));
                        return;
                    }

                    if (!Authorized(headers))
                    {
                        await Respond(stream, 401, Error("unauthorized"));
                        return;
                    }

                    if (RateLimited())
                    {
                        await Respond(stream, 429, Error("too many requests"));
                        return;
                    }

                    JsonNode? node;
                    try { node = JsonNode.Parse(body); }
                    catch
                    {
                        await Respond(stream, 400, Error("invalid json"));
                        return;
                    }

                    if (node is not JsonObject payload)
                    {
                        await Respond(stream, 400, Error("invalid json"));
                        return;
                    }

                    _presence.Update(payload);
                    await Respond(stream, 200, new JsonObject { ["ok"] = true }.ToJsonString());
                }
                catch (Exception e)
                {
                    _lastError = e.Message;
                    Log(e);
                }
            }
        }

        private static void Log(Exception e)
        {
            try
            {
                System.IO.Directory.CreateDirectory(Config.Dir);
                System.IO.File.AppendAllText(
                    System.IO.Path.Combine(Config.Dir, "error.log"),
                    "[" + DateTime.Now.ToString("u") + "] request: " + e + Environment.NewLine);
            }
            catch
            {
                // logging must never break the server
            }
        }

        private JsonObject StatusJson() => new()
        {
            ["ok"] = true,
            ["enabled"] = _cfg.Enabled,
            ["port"] = _cfg.Port,
            ["lastError"] = _lastError
        };

        private bool Authorized(Dictionary<string, string> headers)
        {
            if (!headers.TryGetValue("Authorization", out var header)) return false;
            if (!header.StartsWith("Bearer ", StringComparison.OrdinalIgnoreCase)) return false;

            var given = Encoding.UTF8.GetBytes(header[7..].Trim());
            var wanted = Encoding.UTF8.GetBytes(_cfg.Token);
            return given.Length == wanted.Length && CryptographicOperations.FixedTimeEquals(given, wanted);
        }

        private bool RateLimited()
        {
            // Requests are handled on several threads, so the window is updated atomically.
            var now = Environment.TickCount64;
            if (now - Interlocked.Read(ref _windowStart) > 10_000)
            {
                Interlocked.Exchange(ref _windowStart, now);
                Interlocked.Exchange(ref _windowCount, 0);
            }
            return Interlocked.Increment(ref _windowCount) > 200;
        }

        /// <summary>
        /// Reads the request head. Returns the head plus any body bytes that arrived in the same
        /// chunk: a chunked read can pull in part of the body, and dropping those bytes would make
        /// ReadBody wait for data that was already consumed (POST then timed out).
        /// </summary>
        private static async Task<(string Head, byte[] Extra)?> ReadHead(NetworkStream stream)
        {
            var buffer = new byte[8192];
            var count = 0;
            while (count < buffer.Length)
            {
                var read = await stream.ReadAsync(buffer.AsMemory(count, buffer.Length - count));
                if (read <= 0) return null;

                var from = Math.Max(0, count - 3); // an earlier chunk may hold the first CR of CRLFCRLF
                count += read;

                for (var i = from; i + 3 < count; i++)
                {
                    if (buffer[i] == 13 && buffer[i + 1] == 10 && buffer[i + 2] == 13 && buffer[i + 3] == 10)
                    {
                        var head = Encoding.ASCII.GetString(buffer, 0, i);
                        var rest = new byte[count - (i + 4)];
                        if (rest.Length > 0) Array.Copy(buffer, i + 4, rest, 0, rest.Length);
                        return (head, rest);
                    }
                }
            }
            return null;
        }

        private static async Task<string?> ReadBody(NetworkStream stream, Dictionary<string, string> headers, byte[] rest)
        {
            if (!headers.TryGetValue("Content-Length", out var raw) || !int.TryParse(raw, out var length) || length <= 0)
                return "";

            if (length > 128 * 1024) return null;

            var buffer = new byte[length];
            var read = Math.Min(rest.Length, length);
            if (read > 0) Array.Copy(rest, buffer, read);

            while (read < length)
            {
                var n = await stream.ReadAsync(buffer.AsMemory(read, length - read));
                if (n <= 0) break;
                read += n;
            }
            return Encoding.UTF8.GetString(buffer, 0, read);
        }

        private static async Task Respond(NetworkStream stream, int code, string json)
        {
            var body = Encoding.UTF8.GetBytes(json);
            var head =
                "HTTP/1.1 " + code + " " + Reason(code) + "\r\n" +
                "Content-Type: application/json; charset=utf-8\r\n" +
                "Content-Length: " + body.Length + "\r\n" +
                "Cache-Control: no-store\r\n" +
                "Access-Control-Allow-Origin: *\r\n" +
                "Connection: close\r\n\r\n";

            await stream.WriteAsync(Encoding.ASCII.GetBytes(head));
            await stream.WriteAsync(body);
            await stream.FlushAsync();
        }

        private static string Reason(int code) => code switch
        {
            200 => "OK",
            400 => "Bad Request",
            401 => "Unauthorized",
            404 => "Not Found",
            413 => "Payload Too Large",
            429 => "Too Many Requests",
            _ => "Error"
        };

        private static string Error(string message) => new JsonObject { ["error"] = message }.ToJsonString();

        private void StartDiscovery(CancellationToken token)
        {
            try
            {
                var udp = new UdpClient();
                udp.Client.SetSocketOption(SocketOptionLevel.Socket, SocketOptionName.ReuseAddress, true);
                udp.Client.Bind(new IPEndPoint(IPAddress.Any, DiscoveryPort));
                _udp = udp;

                _ = Task.Run(async () =>
                {
                    while (!token.IsCancellationRequested)
                    {
                        try
                        {
                            var request = await udp.ReceiveAsync(token);
                            var text = Encoding.ASCII.GetString(request.Buffer);
                            if (!text.StartsWith("NEOPRESENCE_DISCOVER", StringComparison.Ordinal)) continue;

                            var reply = Encoding.ASCII.GetBytes("NEOPRESENCE:" + _cfg.Port);
                            await udp.SendAsync(reply, reply.Length, request.RemoteEndPoint);
                        }
                        catch (OperationCanceledException)
                        {
                            return;
                        }
                        catch
                        {
                            try { await Task.Delay(250, token); } catch { return; }
                        }
                    }
                }, token);
            }
            catch (Exception e)
            {
                // Discovery is optional: the phone then uses the last known address or the internet.
                _lastError = "Wifi-detectie: " + e.Message;
            }
        }
    }
}
