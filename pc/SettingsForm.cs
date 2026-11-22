using System;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Windows.Forms;
using QRCoder;

namespace NeoPresence
{
    public sealed class SettingsForm : Form
    {
        private static readonly Color Orange = Color.FromArgb(255, 136, 0);
        private static readonly Color Back = Color.FromArgb(18, 18, 24);
        private static readonly Color Muted = Color.FromArgb(170, 170, 185);

        private readonly Config _cfg;
        private readonly DiscordIpc _ipc;
        private readonly Server _server;
        private readonly PresenceManager _presence;

        private readonly FlowLayoutPanel _flow = new()
        {
            Dock = DockStyle.Fill,
            FlowDirection = FlowDirection.TopDown,
            WrapContents = false,
            AutoScroll = true,
            Padding = new Padding(18, 12, 18, 18),
            BackColor = Back
        };

        private readonly Label _discordValue = Value();
        private readonly Label _serverValue = Value();
        private readonly Label _lastValue = Value();
        private readonly Label _nowValue = Value();
        private readonly PictureBox _qr = new()
        {
            Size = new Size(220, 220),
            SizeMode = PictureBoxSizeMode.Zoom,
            BackColor = Color.White,
            Margin = new Padding(0, 4, 0, 8)
        };

        private readonly TextBox _publicUrl = Input("https://presence.neofastftldev.nl");
        private readonly NumericUpDown _port = new() { Minimum = 1024, Maximum = 65535, Width = 100, BackColor = Color.FromArgb(43, 45, 49), ForeColor = Color.White };
        private readonly TextBox _appId = Input("Application ID");
        private readonly TextBox _largeImage = Input("leeg = geen grote afbeelding");
        private readonly NumericUpDown _pauseClear = new() { Minimum = 0, Maximum = 3600, Width = 100, BackColor = Color.FromArgb(43, 45, 49), ForeColor = Color.White };
        private readonly CheckBox _enabled = new() { Text = "Status op Discord tonen", AutoSize = true, ForeColor = Color.White, Margin = new Padding(0, 6, 0, 6) };
        private readonly CheckBox _autoStart = new() { Text = "Automatisch starten bij inloggen", AutoSize = true, ForeColor = Color.White, Margin = new Padding(0, 0, 0, 6) };
        private readonly TextBox _pairLink = Input("");

        private string _toast = "";
        private DateTime _toastUntil = DateTime.MinValue;

        public SettingsForm(Config cfg, DiscordIpc ipc, Server server, PresenceManager presence)
        {
            _cfg = cfg;
            _ipc = ipc;
            _server = server;
            _presence = presence;

            Text = "NeoPresence";
            BackColor = Back;
            ForeColor = Color.White;
            ClientSize = new Size(540, 780);
            StartPosition = FormStartPosition.CenterScreen;
            AutoScaleMode = AutoScaleMode.Dpi;
            MinimumSize = new Size(520, 560);

            BuildUi();
            LoadValues();
            RefreshQr();
            RefreshStatus();
        }

        private void BuildUi()
        {
            var title = new Label
            {
                Text = "NeoPresence",
                AutoSize = true,
                Font = new Font("Segoe UI", 20f, FontStyle.Bold),
                ForeColor = Orange,
                Margin = new Padding(0, 0, 0, 2)
            };
            var subtitle = Label("Laat zien wat je in VLC luistert op je Discord-profiel.", Muted);

            _enabled.CheckedChanged += (_, _) => { };
            _qr.Cursor = Cursors.Hand;
            _qr.Click += (_, _) => Clipboard.SetText(_cfg.PairUri);

            var tokenBox = Input("");
            tokenBox.ReadOnly = true;
            tokenBox.Text = _cfg.Token;
            tokenBox.Click += (_, _) => tokenBox.SelectAll();

            _pairLink.ReadOnly = true;
            _pairLink.Text = _cfg.PairUri;

            var copyToken = Button("Kopieer koppelcode", () =>
            {
                Clipboard.SetText(_cfg.Token);
                Toast("Koppelcode gekopieerd");
            });

            var newToken = Button("Nieuwe koppelcode", () =>
            {
                if (MessageBox.Show(this, "De huidige koppeling met je telefoon stopt. Doorgaan?",
                        "NeoPresence", MessageBoxButtons.YesNo, MessageBoxIcon.Question) != DialogResult.Yes) return;
                _cfg.Token = Config.NewToken();
                _cfg.Save();
                tokenBox.Text = _cfg.Token;
                _pairLink.Text = _cfg.PairUri;
                RefreshQr();
                Toast("Nieuwe koppelcode gemaakt - scan opnieuw");
            });

            var copyLink = Button("Kopieer koppel-link", () =>
            {
                Clipboard.SetText(_cfg.PairUri);
                Toast("Koppelkoppeling gekopieerd");
            });

            var save = Button("Opslaan", Save);
            var openFolder = Button("Configuratiemap openen", () =>
                System.Diagnostics.Process.Start("explorer.exe", Config.Dir));
            var hide = Button("Verbergen", Hide);

            _flow.Controls.Add(title);
            _flow.Controls.Add(subtitle);

            _flow.Controls.Add(Header("Status"));
            _flow.Controls.Add(Row("Discord", _discordValue));
            _flow.Controls.Add(Row("Server", _serverValue));
            _flow.Controls.Add(Row("Nu", _nowValue));
            _flow.Controls.Add(Row("Laatste update", _lastValue));

            _flow.Controls.Add(Header("Koppelen met je telefoon"));
            _flow.Controls.Add(Label("Scan deze code met de knop \"QR-code van je pc scannen\" in de NeoPresence-app op je telefoon.", Muted));
            _flow.Controls.Add(_qr);
            _flow.Controls.Add(Label("Koppelcode", Muted));
            _flow.Controls.Add(tokenBox);
            _flow.Controls.Add(copyToken);
            _flow.Controls.Add(Label("Koppel-link (als scannen niet lukt, plak dit in de app)", Muted));
            _flow.Controls.Add(_pairLink);
            _flow.Controls.Add(copyLink);
            _flow.Controls.Add(newToken);

            _flow.Controls.Add(Header("Verbinding"));
            _flow.Controls.Add(Label("Openbare adres (tunnel) waarmee de telefoon je pc bereikt buiten je wifi", Muted));
            _flow.Controls.Add(_publicUrl);
            _flow.Controls.Add(Label("Poort op je wifi", Muted));
            _flow.Controls.Add(_port);
            _flow.Controls.Add(_enabled);
            _flow.Controls.Add(_autoStart);

            _flow.Controls.Add(Header("Discord"));
            _flow.Controls.Add(Label("Application ID van je Discord-app", Muted));
            _flow.Controls.Add(_appId);
            _flow.Controls.Add(Label("Grote afbeelding (assetsleutel)", Muted));
            _flow.Controls.Add(_largeImage);
            _flow.Controls.Add(Label("Seconden wachten met wissen nadat je pauzeert (0 = nooit wissen)", Muted));
            _flow.Controls.Add(_pauseClear);

            _flow.Controls.Add(save);
            _flow.Controls.Add(openFolder);
            _flow.Controls.Add(hide);

            Controls.Add(_flow);
        }

        private void LoadValues()
        {
            _publicUrl.Text = _cfg.PublicUrl;
            _port.Value = Math.Clamp(_cfg.Port, 1024, 65535);
            _appId.Text = _cfg.DiscordAppId;
            _largeImage.Text = _cfg.LargeImage;
            _pauseClear.Value = Math.Clamp(_cfg.PauseClearSeconds, 0, 3600);
            _enabled.Checked = _cfg.Enabled;
            _autoStart.Checked = _cfg.AutoStart;
        }

        private void Save()
        {
            var url = _publicUrl.Text.Trim().TrimEnd('/');
            if (!url.StartsWith("https://", StringComparison.OrdinalIgnoreCase))
            {
                MessageBox.Show(this, "Het openbare adres moet met https:// beginnen, bijvoorbeeld https://presence.example.nl",
                    "NeoPresence", MessageBoxButtons.OK, MessageBoxIcon.Warning);
                return;
            }

            _cfg.PublicUrl = url;
            _publicUrl.Text = url;
            _cfg.Port = (int)_port.Value;
            _cfg.DiscordAppId = _appId.Text.Trim();
            _cfg.LargeImage = _largeImage.Text.Trim();
            _cfg.PauseClearSeconds = (int)_pauseClear.Value;
            _cfg.Enabled = _enabled.Checked;
            _cfg.AutoStart = _autoStart.Checked;

            Config.ApplyAutoStart(_cfg.AutoStart);
            _cfg.Save();
            _server.Restart();

            _pairLink.Text = _cfg.PairUri;
            RefreshQr();
            RefreshStatus();
            Toast(_cfg.LastError.Length > 0 ? _cfg.LastError : "Instellingen opgeslagen");
        }

        public void RefreshQr()
        {
            try
            {
                using var data = new QRCodeGenerator().CreateQrCode(_cfg.PairUri, QRCodeGenerator.ECCLevel.M);
                var png = new PngByteQRCode(data).GetGraphic(6);
                using var stream = new System.IO.MemoryStream(png);
                using var image = Image.FromStream(stream);
                var previous = _qr.Image;
                _qr.Image = new Bitmap(image);
                previous?.Dispose();
                _pairLink.Text = _cfg.PairUri;
            }
            catch (Exception e)
            {
                _cfg.LastError = "QR maken mislukt: " + e.Message;
            }
        }

        public void RefreshStatus()
        {
            _discordValue.Text = _ipc.Connected
                ? "Verbonden" + (_ipc.UserName.Length > 0 ? " als " + _ipc.UserName : "")
                : "Niet verbonden" + (_ipc.LastError.Length > 0 ? " (" + _ipc.LastError + ")" : "");

            _serverValue.Text = _server.Status +
                (_server.LastError.Length > 0 ? " - " + _server.LastError : "") +
                " (wifi: " + Config.LanIp() + ")";

            _nowValue.Text = _presence.NowPlaying.Length > 0 ? _presence.NowPlaying : "Niets";
            // A toast must stay readable; the 1-second refresh used to wipe it instantly.
            _lastValue.Text = _toast.Length > 0 && DateTime.Now < _toastUntil ? _toast : _presence.Status;

            _enabled.Checked = _cfg.Enabled;
        }

        public void Toast(string message)
        {
            _toast = message;
            _toastUntil = DateTime.Now.AddSeconds(6);
            _lastValue.Text = message;
        }

        protected override void OnFormClosing(FormClosingEventArgs e)
        {
            if (e.CloseReason == CloseReason.UserClosing)
            {
                e.Cancel = true;
                Hide();
                return;
            }
            base.OnFormClosing(e);
        }

        private static Label Label(string text, Color color) => new()
        {
            Text = text,
            AutoSize = true,
            ForeColor = color,
            MaximumSize = new Size(490, 0),
            Margin = new Padding(0, 2, 0, 4)
        };

        private static Label Value() => new()
        {
            AutoSize = true,
            ForeColor = Color.White,
            MaximumSize = new Size(490, 0),
            Margin = new Padding(0, 0, 0, 4)
        };

        private static Label Header(string text) => new()
        {
            Text = text,
            AutoSize = true,
            Font = new Font("Segoe UI", 12f, FontStyle.Bold),
            ForeColor = Orange,
            Margin = new Padding(0, 16, 0, 6)
        };

        private static Control Row(string name, Label value)
        {
            var panel = new FlowLayoutPanel
            {
                FlowDirection = FlowDirection.LeftToRight,
                WrapContents = false,
                AutoSize = true,
                Margin = new Padding(0, 0, 0, 2)
            };
            var caption = new Label
            {
                Text = name + ":",
                AutoSize = true,
                ForeColor = Muted,
                Width = 110,
                Margin = new Padding(0, 0, 8, 0)
            };
            panel.Controls.Add(caption);
            panel.Controls.Add(value);
            return panel;
        }

        private static TextBox Input(string hint) => new()
        {
            Width = 480,
            BackColor = Color.FromArgb(43, 45, 49),
            ForeColor = Color.White,
            BorderStyle = BorderStyle.FixedSingle,
            PlaceholderText = hint,
            Margin = new Padding(0, 2, 0, 8)
        };

        private static Button Button(string text, Action action)
        {
            var button = new Button
            {
                Text = text,
                AutoSize = true,
                BackColor = Color.FromArgb(43, 45, 49),
                ForeColor = Color.White,
                FlatStyle = FlatStyle.Flat,
                Margin = new Padding(0, 4, 0, 4)
            };
            button.FlatAppearance.BorderColor = Orange;
            button.Click += (_, _) => action();
            return button;
        }
    }
}
