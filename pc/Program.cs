using System;
using System.Drawing;
using System.IO;
using System.Threading;
using System.Windows.Forms;

namespace MediaPresence
{
    internal static class Program
    {
        private static Mutex? _mutex;

        [STAThread]
        private static void Main(string[] args)
        {
            _mutex = new Mutex(true, @"Local\MediaPresence.SingleInstance", out var isFirstInstance);
            if (!isFirstInstance) return;

            Application.SetHighDpiMode(HighDpiMode.SystemAware);
            Application.EnableVisualStyles();
            Application.SetCompatibleTextRenderingDefault(false);

            var cfg = Config.Load();
            var ipc = new DiscordIpc(cfg);
            var presence = new PresenceManager(cfg, ipc);
            var server = new Server(cfg, presence);

            AppDomain.CurrentDomain.UnhandledException += (_, e) =>
            {
                if (e.ExceptionObject is Exception ex) Log(cfg, ex);
            };
            Application.ThreadException += (_, e) => Log(cfg, e.Exception);

            Application.Run(new TrayContext(cfg, ipc, presence, server, args));
        }

        private static void Log(Config cfg, Exception ex)
        {
            try
            {
                Directory.CreateDirectory(Config.Dir);
                File.AppendAllText(Path.Combine(Config.Dir, "error.log"),
                    $"[{DateTime.Now:u}] {ex.GetType().Name}: {ex.Message}{Environment.NewLine}{ex.StackTrace}{Environment.NewLine}");
                cfg.LastError = ex.Message;
            }
            catch
            {
                // nothing else we can do
            }
        }
    }

    internal sealed class TrayContext : ApplicationContext
    {
        private readonly Config _cfg;
        private readonly DiscordIpc _ipc;
        private readonly PresenceManager _presence;
        private readonly Server _server;
        private readonly NotifyIcon _tray;
        private readonly ToolStripMenuItem _toggle;
        private readonly System.Windows.Forms.Timer _tick;
        private SettingsForm? _form;

        public TrayContext(Config cfg, DiscordIpc ipc, PresenceManager presence, Server server, string[] args)
        {
            _cfg = cfg;
            _ipc = ipc;
            _presence = presence;
            _server = server;

            _toggle = new ToolStripMenuItem("Status tonen op Discord", null, (_, _) => SetEnabled(!_cfg.Enabled))
            {
                Checked = cfg.Enabled
            };

            var menu = new ContextMenuStrip();
            menu.Items.Add(new ToolStripMenuItem("Instellingen...", null, (_, _) => ShowForm()));
            menu.Items.Add(_toggle);
            menu.Items.Add(new ToolStripSeparator());
            menu.Items.Add(new ToolStripMenuItem("Afsluiten", null, (_, _) => Quit()));

            _tray = new NotifyIcon
            {
                Icon = SystemIcons.Application,
                Text = "MediaPresence",
                Visible = true,
                ContextMenuStrip = menu
            };
            _tray.DoubleClick += (_, _) => ShowForm();

            ipc.Start();
            server.Start();

            _tick = new System.Windows.Forms.Timer { Interval = 1000 };
            _tick.Tick += (_, _) => Tick();
            _tick.Start();

            var forceShow = Array.IndexOf(args, "--show") >= 0;
            if (forceShow || !cfg.StartHidden) ShowForm();
        }

        private void Tick()
        {
            var tip = "MediaPresence";
            if (_ipc.Connected)
                tip += " - Discord: " + (_ipc.UserName.Length > 0 ? _ipc.UserName : "verbonden");
            else
                tip += " - Discord niet verbonden";
            _tray.Text = tip.Length > 63 ? tip[..63] : tip;

            _toggle.Checked = _cfg.Enabled;
            _form?.RefreshStatus();
        }

        private void SetEnabled(bool enabled)
        {
            _cfg.Enabled = enabled;
            _cfg.Save();
            _toggle.Checked = enabled;

            if (enabled) _presence.Refresh();
            else _presence.Clear();

            _form?.RefreshStatus();
        }

        private void ShowForm()
        {
            if (_form == null || _form.IsDisposed)
            {
                _form = new SettingsForm(_cfg, _ipc, _server, _presence);
                _form.FormClosed += (_, _) => _form = null;
            }

            _form.Show();
            _form.WindowState = FormWindowState.Normal;
            _form.Activate();
        }

        private void Quit()
        {
            _tick.Stop();
            _tray.Visible = false;
            _presence.Dispose();
            _server.Dispose();
            _ipc.Dispose();
            if (_form != null && !_form.IsDisposed) _form.Dispose();
            ExitThread();
        }
    }
}
