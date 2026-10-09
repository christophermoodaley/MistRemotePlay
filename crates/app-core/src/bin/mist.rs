//! `mist` command-line interface: host, pair, connect and diagnostics.
//! The desktop app launches `mist connect` for the streaming window.

use std::io::BufRead;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use mist_app_core::client::session::{self, SessionOptions};
use mist_app_core::client::sinks::{HeadlessSink, LatestFrameSink};
use mist_app_core::client::window::{self, UiEvent, WindowOptions};
use mist_app_core::client::ClientShared;
use mist_app_core::config::{AudioChoice, ControllerChoice};
use mist_app_core::events::{ClientEvent, HostEvent};
use mist_app_core::host::{self, HostHandle};
use mist_app_core::AppContext;
use mist_capture_api::CaptureKind;
use mist_input_api::InputBackendKind;
use mist_media_engine::{FrameSink, IceConfig};
use mist_protocol::identity::DeviceId;
use mist_protocol::signaling::{IceServer, Role};
use mist_session_manager::SessionManager;
use tokio::sync::{broadcast, mpsc};

#[derive(Parser)]
#[command(name = "mist", version, about = "Mist Remote Play — stream games between Windows and Linux PCs")]
struct Cli {
    /// Configuration directory (default: platform config dir, or MIST_CONFIG_DIR).
    #[arg(long, global = true, env = "MIST_CONFIG_DIR")]
    config_dir: Option<PathBuf>,
    /// Override the signaling server URL for this run.
    #[arg(long, global = true)]
    signaling: Option<String>,
    /// Extra CA certificate (PEM) to trust for the signaling server.
    #[arg(long, global = true)]
    ca: Option<PathBuf>,
    /// Emit machine-readable JSON lines on stdout.
    #[arg(long, global = true)]
    json: bool,
    /// Force all media through TURN.
    #[arg(long, global = true)]
    relay_only: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum InputArg {
    Auto,
    Mock,
    Xtest,
    Uinput,
    Sendinput,
}

#[derive(Clone, Copy, ValueEnum)]
enum PadArg {
    Platform,
    Mock,
    Off,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the host: stream this computer to paired clients.
    Host {
        /// Display id (see `mist diagnose`); default: primary.
        #[arg(long)]
        display: Option<String>,
        /// Stream a synthetic test pattern instead of the screen (testing).
        #[arg(long)]
        test_pattern: bool,
        /// System audio: `default`, `off`, `test-tone` or a device id.
        #[arg(long, default_value = "default")]
        audio: String,
        #[arg(long, value_enum, default_value = "auto")]
        input: InputArg,
        #[arg(long, value_enum, default_value = "platform")]
        controllers: PadArg,
        /// Start sessions with remote input disabled.
        #[arg(long)]
        no_input: bool,
        /// Approve session requests from already-paired devices without prompting.
        #[arg(long)]
        trust_paired_sessions: bool,
        /// Show a pairing code immediately.
        #[arg(long)]
        pair: bool,
        /// Preferred encoder element (e.g. x264enc, nvh264enc, vah264enc).
        #[arg(long)]
        encoder: Option<String>,
        /// Advertise this host on the LAN (mDNS).
        #[arg(long)]
        advertise: bool,
    },
    /// Pair with a host using the code it displays.
    Pair { code: String },
    /// List paired devices.
    Devices,
    /// Remove a paired device.
    Forget { device: String },
    /// Connect to a paired host and open the streaming window.
    Connect {
        /// Host device id (or prefix) or name; default: most recently used.
        host: Option<String>,
        /// No window: count frames and report (tests/benchmarks).
        #[arg(long)]
        headless: bool,
        /// Disconnect after this many seconds.
        #[arg(long)]
        duration: Option<u64>,
        #[arg(long)]
        fullscreen: bool,
        #[arg(long)]
        width: Option<u32>,
        #[arg(long)]
        height: Option<u32>,
        #[arg(long)]
        fps: Option<u32>,
        #[arg(long)]
        max_bitrate: Option<u32>,
        /// Disable controller forwarding.
        #[arg(long)]
        no_controllers: bool,
        /// Testing aid (headless only): once streaming, send a fixed input
        /// sequence and leave Left Shift held to exercise host-side cleanup.
        #[arg(long, hide = true)]
        input_selftest: bool,
    },
    /// Find hosts on the local network (mDNS).
    Discover {
        #[arg(long, default_value_t = 3)]
        secs: u64,
    },
    /// Print detected capabilities (capture, encoders, audio, input).
    Diagnose,
    /// Gather ICE candidates through the configured STUN/TURN servers.
    IceCheck,
    /// Show or change configuration.
    Config {
        #[command(subcommand)]
        action: ConfigCmd,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    Show,
    SetSignaling {
        url: String,
    },
    SetAccessToken {
        token: String,
    },
    SetCa {
        path: PathBuf,
    },
    SetDeviceName {
        name: String,
    },
    /// Add an ICE server, e.g. `turn:turn.example.org:3478?transport=udp --username u --credential p`.
    AddIceServer {
        url: String,
        #[arg(long)]
        username: Option<String>,
        #[arg(long)]
        credential: Option<String>,
    },
    ClearIceServers,
    SetRelayOnly {
        enabled: bool,
    },
}

fn load(cli: &Cli) -> anyhow::Result<AppContext> {
    let dir = cli.config_dir.clone().unwrap_or_else(mist_app_core::config::config_dir);
    let mut ctx = AppContext::load_from(dir)?;
    if let Some(s) = &cli.signaling {
        ctx.config.network.signaling_url = s.clone();
    }
    if let Some(ca) = &cli.ca {
        ctx.config.network.extra_ca_pem = Some(ca.clone());
    }
    if cli.relay_only {
        ctx.config.network.relay_only = true;
    }
    Ok(ctx)
}

fn out<T: serde::Serialize>(json: bool, value: &T, human: impl FnOnce() -> String) {
    if json {
        println!("{}", serde_json::to_string(value).unwrap_or_default());
    } else {
        println!("{}", human());
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let _logs = mist_diagnostics::init(if cli.json {
        mist_diagnostics::LogFormat::Json
    } else {
        mist_diagnostics::LogFormat::Text
    });
    match &cli.cmd {
        Cmd::Connect { headless: false, .. } => connect_window(&cli),
        _ => tokio::runtime::Runtime::new()?.block_on(run(&cli)),
    }
}

async fn run(cli: &Cli) -> anyhow::Result<()> {
    let mut ctx = load(cli)?;
    match &cli.cmd {
        Cmd::Host {
            display,
            test_pattern,
            audio,
            input,
            controllers,
            no_input,
            trust_paired_sessions,
            pair,
            encoder,
            advertise,
        } => {
            let h = &mut ctx.config.host;
            h.display_id = display.clone();
            if *test_pattern {
                h.capture = Some(CaptureKind::TestPattern);
            }
            h.audio = match audio.as_str() {
                "default" => AudioChoice::Default,
                "off" => AudioChoice::Off,
                "test-tone" => AudioChoice::TestTone,
                d => AudioChoice::Device(d.into()),
            };
            h.input_backend = match input {
                InputArg::Auto => InputBackendKind::Auto,
                InputArg::Mock => InputBackendKind::Mock,
                InputArg::Xtest => InputBackendKind::X11Xtest,
                InputArg::Uinput => InputBackendKind::Uinput,
                InputArg::Sendinput => InputBackendKind::Win32SendInput,
            };
            h.controllers = match controllers {
                PadArg::Platform => ControllerChoice::Platform,
                PadArg::Mock => ControllerChoice::Mock,
                PadArg::Off => ControllerChoice::Off,
            };
            h.input_allowed = !no_input;
            h.trust_paired_sessions |= *trust_paired_sessions;
            if encoder.is_some() {
                h.encoder = encoder.clone();
            }
            run_host(ctx, cli.json, *pair, *advertise).await
        }
        Cmd::Pair { code } => {
            let dev = mist_app_core::client::pair(&ctx, code).await?;
            out(
                cli.json,
                &ClientEvent::Paired { host_name: dev.name.clone(), device_id: dev.device_id.to_string() },
                || {
                    format!(
                        "Paired with \"{}\" ({}). Connect with: mist connect {}",
                        dev.name,
                        dev.device_id.fingerprint(),
                        &dev.device_id.as_str()[..8]
                    )
                },
            );
            Ok(())
        }
        Cmd::Devices => {
            let list = ctx.trust.lock().list();
            out(cli.json, &list, || {
                if list.is_empty() {
                    return "No paired devices.".into();
                }
                list.iter()
                    .map(|d| format!("{}  {}  ({})", d.device_id, d.name, d.device_id.fingerprint()))
                    .collect::<Vec<_>>()
                    .join("\n")
            });
            Ok(())
        }
        Cmd::Forget { device } => {
            let id = resolve_device(&ctx, Some(device))?;
            ctx.trust.lock().remove(&id)?;
            println!("Removed {id}");
            Ok(())
        }
        Cmd::Connect { .. } => connect_headless(cli, ctx).await,
        Cmd::Discover { secs } => {
            let secs = *secs;
            let hosts =
                tokio::task::spawn_blocking(move || mist_app_core::discovery::browse(Duration::from_secs(secs)))
                    .await??;
            out(cli.json, &hosts, || {
                if hosts.is_empty() {
                    return "No hosts found on the local network.".into();
                }
                hosts
                    .iter()
                    .map(|h| format!("{}  {}  via {}", h.device_id, h.name, h.signaling_url))
                    .collect::<Vec<_>>()
                    .join("\n")
            });
            Ok(())
        }
        Cmd::Diagnose => {
            let report = tokio::task::spawn_blocking(move || mist_app_core::report::system_report(&ctx)).await?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        Cmd::IceCheck => ice_check(cli, ctx).await,
        Cmd::Config { action } => {
            match action {
                ConfigCmd::Show => {
                    println!("{}", serde_json::to_string_pretty(&ctx.config)?);
                    println!("config dir: {}", ctx.dir.display());
                    println!("device id: {}", ctx.identity.device_id());
                    return Ok(());
                }
                ConfigCmd::SetSignaling { url } => ctx.config.network.signaling_url = url.clone(),
                ConfigCmd::SetAccessToken { token } => {
                    ctx.secrets.signaling_access_token = Some(token.clone());
                    ctx.secrets.save(&ctx.dir)?;
                }
                ConfigCmd::SetCa { path } => ctx.config.network.extra_ca_pem = Some(path.clone()),
                ConfigCmd::SetDeviceName { name } => ctx.config.device_name = name.clone(),
                ConfigCmd::AddIceServer { url, username, credential } => {
                    ctx.config.network.ice_servers.push(IceServer {
                        urls: vec![url.clone()],
                        username: username.clone(),
                        credential: credential.clone(),
                    })
                }
                ConfigCmd::ClearIceServers => ctx.config.network.ice_servers.clear(),
                ConfigCmd::SetRelayOnly { enabled } => ctx.config.network.relay_only = *enabled,
            }
            ctx.save_config()?;
            println!("saved");
            Ok(())
        }
    }
}

fn resolve_device(ctx: &AppContext, query: Option<&String>) -> anyhow::Result<DeviceId> {
    let list = ctx.trust.lock().list();
    let found = match query {
        None => list.first().cloned(),
        Some(q) => {
            let q = q.to_lowercase();
            let matches: Vec<_> = list
                .iter()
                .filter(|d| d.device_id.as_str().starts_with(&q) || d.name.to_lowercase() == q)
                .cloned()
                .collect();
            if matches.len() > 1 {
                anyhow::bail!("`{q}` matches several devices; use more of the device id");
            }
            matches.into_iter().next()
        }
    };
    found.map(|d| d.device_id).ok_or_else(|| anyhow::anyhow!("no matching paired device (see `mist devices`)"))
}

// ----------------------------------------------------------------- host

async fn run_host(ctx: AppContext, json: bool, pair: bool, advertise: bool) -> anyhow::Result<()> {
    let device_id = ctx.identity.device_id().to_string();
    let _advertiser = if advertise {
        mist_app_core::discovery::Advertiser::start(
            &device_id,
            &ctx.config.device_name,
            &ctx.config.network.signaling_url,
        )
        .map_err(|e| tracing::warn!("LAN advertisement unavailable: {e}"))
        .ok()
    } else {
        None
    };
    let handle = host::start(ctx);
    let mut events = handle.subscribe();
    if pair {
        handle.generate_pairing_code();
    }
    if !json {
        println!("Hosting as {device_id}. Commands: pair | y / n (answer the latest request) | input on|off | kick | status | quit");
    }
    let (line_tx, mut line_rx) = mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if line_tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut last_request: Option<(bool, String)> = None; // (is_pairing, id)
    loop {
        tokio::select! {
            ev = events.recv() => match ev {
                Ok(ev) => {
                    match &ev {
                        HostEvent::PairingRequest { request_id, .. } => last_request = Some((true, request_id.clone())),
                        HostEvent::SessionRequest { session_id, .. } => last_request = Some((false, session_id.clone())),
                        _ => {}
                    }
                    print_host_event(json, &ev);
                }
                Err(broadcast::error::RecvError::Lagged(n)) => tracing::warn!("dropped {n} events"),
                Err(_) => break,
            },
            line = line_rx.recv() => {
                let Some(line) = line else { continue };
                let words: Vec<&str> = line.split_whitespace().collect();
                match words.as_slice() {
                    ["pair"] => handle.generate_pairing_code(),
                    ["y" | "yes"] | ["n" | "no"] => {
                        let approve = matches!(words[0], "y" | "yes");
                        match last_request.take() {
                            Some((true, id)) => { let _ = handle.decide_pairing(&id, approve); }
                            Some((false, id)) => {
                                let r = if approve { handle.approve(&id) } else { handle.reject(&id, "declined by the host") };
                                if let Err(e) = r { eprintln!("{e}"); }
                            }
                            None => eprintln!("nothing to answer"),
                        }
                    }
                    ["input", "on"] => handle.set_input_allowed(true),
                    ["input", "off"] => handle.set_input_allowed(false),
                    ["kick"] => handle.disconnect_all(),
                    ["status"] => println!("{}", serde_json::to_string_pretty(&handle.snapshot()).unwrap_or_default()),
                    ["quit" | "exit"] => break,
                    [] => {}
                    _ => eprintln!("unknown command"),
                }
            }
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    handle.disconnect_all();
    tokio::time::sleep(Duration::from_millis(500)).await;
    drop::<HostHandle>(handle);
    tokio::time::sleep(Duration::from_millis(200)).await;
    Ok(())
}

fn print_host_event(json: bool, ev: &HostEvent) {
    if json {
        println!("{}", serde_json::to_string(ev).unwrap_or_default());
        return;
    }
    match ev {
        HostEvent::Signaling { connected, detail } => {
            println!("[signaling] {} — {detail}", if *connected { "online" } else { "offline" })
        }
        HostEvent::PairingCode { code, expires_in_secs } => {
            println!("Pairing code: {code}  (valid {expires_in_secs}s)")
        }
        HostEvent::PairingCodeExpired => println!("Pairing code expired."),
        HostEvent::PairingRequest { client_name, fingerprint, .. } => {
            println!("Pairing request from \"{client_name}\" (fingerprint {fingerprint}). Approve? [y/n]")
        }
        HostEvent::Paired { client_name, .. } => println!("Paired with \"{client_name}\"."),
        HostEvent::PairingRejected { client_name, reason } => {
            println!("Pairing with \"{client_name}\" rejected: {reason}")
        }
        HostEvent::SessionRequest { client_name, fingerprint, settings, .. } => println!(
            "\"{client_name}\" ({fingerprint}) wants to connect: {}x{}@{}fps. Allow? [y/n]",
            settings.width, settings.height, settings.fps
        ),
        HostEvent::Session { client_name, state, .. } => println!("[session] {client_name}: {state:?}"),
        HostEvent::InputPermission { allowed } => {
            println!("Remote input {}", if *allowed { "enabled" } else { "DISABLED" })
        }
        HostEvent::Stats(s) => println!(
            "[stats] {} {}x{} | capture {:.0} fps | encode {:.1} ms | target {} kbps | rtt {} | loss {}",
            s.host.encoder,
            s.host.width,
            s.host.height,
            s.host.capture_fps,
            s.host.encode_ms_avg,
            s.host.target_bitrate_kbps,
            s.transport.remote_rtt_ms.map(|r| format!("{r:.0} ms")).unwrap_or_else(|| "n/a".into()),
            s.transport.remote_fraction_lost.map(|l| format!("{:.1}%", l * 100.0)).unwrap_or_else(|| "n/a".into()),
        ),
        HostEvent::Warning { message } => println!("warning: {message}"),
        HostEvent::Error { message } => println!("error: {message}"),
    }
}

// --------------------------------------------------------------- client

fn apply_stream_overrides(ctx: &mut AppContext, cli: &Cli) {
    if let Cmd::Connect { width, height, fps, max_bitrate, fullscreen, .. } = &cli.cmd {
        let s = &mut ctx.config.client.stream;
        if let Some(w) = width {
            s.width = *w;
        }
        if let Some(h) = height {
            s.height = *h;
        }
        if let Some(f) = fps {
            s.fps = *f;
        }
        if let Some(b) = max_bitrate {
            s.max_bitrate_kbps = *b;
            s.min_bitrate_kbps = s.min_bitrate_kbps.min(*b);
        }
        if *fullscreen {
            ctx.config.client.fullscreen = true;
        }
    }
}

fn print_client_event(json: bool, ev: &ClientEvent) {
    if json {
        println!("{}", serde_json::to_string(ev).unwrap_or_default());
        return;
    }
    match ev {
        ClientEvent::State { state, detail } => println!("[{state:?}] {detail}"),
        ClientEvent::Stats(s) => println!(
            "[stats] {:.0} fps | {} | latency {} | rtt {} | decode {:.1} ms | lost {} | dropped {} | {}",
            s.stats.rendered_fps,
            s.video_bitrate_kbps.map(|k| format!("{:.1} Mbps", k / 1000.0)).unwrap_or_else(|| "? Mbps".into()),
            s.stats.latency_ms.map(|l| format!("{l:.1} ms")).unwrap_or_else(|| "n/a".into()),
            s.rtt_ms.map(|r| format!("{r:.1} ms")).unwrap_or_else(|| "n/a".into()),
            s.stats.decode_ms_avg,
            s.stats.packets_lost,
            s.stats.frames_dropped,
            match s.relayed {
                Some(true) => "TURN relay",
                Some(false) => "direct",
                None => "path unknown",
            }
        ),
        ClientEvent::Warning { message } => println!("warning: {message}"),
        ClientEvent::Error { message } => println!("error: {message}"),
        other => println!("{}", serde_json::to_string(other).unwrap_or_default()),
    }
}

async fn connect_headless(cli: &Cli, mut ctx: AppContext) -> anyhow::Result<()> {
    let Cmd::Connect { host, duration, no_controllers, input_selftest, .. } = &cli.cmd else { unreachable!() };
    apply_stream_overrides(&mut ctx, cli);
    let host_id = resolve_device(&ctx, host.as_ref())?;
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let shared = ClientShared::new(cmd_tx.clone());
    let sink = HeadlessSink::new(shared.clone());
    let (events, mut rx) = broadcast::channel(256);
    let json = cli.json;
    let selftest = *input_selftest;
    let script_shared = shared.clone();
    let printer = tokio::spawn(async move {
        let mut streaming = false;
        let mut done = false;
        while let Ok(ev) = rx.recv().await {
            if let ClientEvent::State { state, .. } = &ev {
                streaming = *state == mist_app_core::events::ClientState::Streaming;
            }
            print_client_event(json, &ev);
            if selftest && streaming && !done {
                done = true;
                let s = script_shared.clone();
                tokio::spawn(async move { input_selftest_sequence(s).await });
            }
        }
    });
    if let Some(secs) = duration {
        let tx = cmd_tx.clone();
        let secs = *secs;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(secs)).await;
            let _ = tx.send(mist_app_core::client::ClientCmd::Disconnect);
        });
    }
    let ctrl = cmd_tx.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = ctrl.send(mist_app_core::client::ClientCmd::Disconnect);
    });
    let opts = SessionOptions { host: host_id, controllers: !no_controllers };
    let result = session::run(ctx, opts, shared.clone(), sink.clone() as Arc<dyn FrameSink>, events, cmd_rx).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    printer.abort();
    let report = sink.report();
    let summary = serde_json::json!({
        "event": "summary",
        "result": result.as_ref().map(|r| r.clone()).map_err(|e| e.to_string()),
        "frames": report.frames,
        "distinct_frames": report.distinct_frames,
        "frames_with_capture_timestamp": report.frames_with_capture_timestamp,
        "width": report.width,
        "height": report.height,
        "mean_latency_ms_last_2s": shared.recent_latency_ms(),
    });
    println!("{summary}");
    result.map(|_| ())
}

async fn input_selftest_sequence(s: Arc<ClientShared>) {
    use mist_protocol::input::{InputMessage, MouseButton};
    let key = |code: &str| mist_protocol::keys::by_code(code).expect("known key").hid;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let steps = [
        InputMessage::MouseMoveAbs { x: 32768, y: 32768 },
        InputMessage::Key { usage: key("KeyA"), down: true, repeat: false },
        InputMessage::Key { usage: key("KeyA"), down: false, repeat: false },
        InputMessage::MouseButton { button: MouseButton::Left, down: true },
        InputMessage::MouseButton { button: MouseButton::Left, down: false },
        InputMessage::MouseMove { dx: 25, dy: -10 },
        InputMessage::Scroll { dx: 0, dy: 120 },
        // Deliberately left held: the host must release it on disconnect.
        InputMessage::Key { usage: key("ShiftLeft"), down: true, repeat: false },
    ];
    for m in steps {
        s.send_input(m);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tracing::info!("input self-test sequence sent");
}

fn connect_window(cli: &Cli) -> anyhow::Result<()> {
    let mut ctx = load(cli)?;
    apply_stream_overrides(&mut ctx, cli);
    let Cmd::Connect { host, duration, no_controllers, .. } = &cli.cmd else { unreachable!() };
    let host_id = resolve_device(&ctx, host.as_ref())?;
    let host_name = ctx.trust.lock().get(&host_id).map(|d| d.name.clone()).unwrap_or_default();
    let event_loop = window::build_event_loop()?;
    let proxy = event_loop.create_proxy();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let shared = ClientShared::new(cmd_tx.clone());
    let frames = LatestFrameSink::new(shared.clone(), window::proxy_waker(proxy.clone()));
    let win_opts = WindowOptions {
        title: format!("Mist — {host_name}"),
        fullscreen: ctx.config.client.fullscreen,
        capture_mouse: ctx.config.client.capture_mouse,
    };
    let json = cli.json;
    let duration = *duration;
    let opts = SessionOptions { host: host_id, controllers: !no_controllers };
    let shared2 = shared.clone();
    let sink: Arc<dyn FrameSink> = frames.clone();
    // Line commands on stdin let a parent process (the desktop app) control
    // the streaming window portably: disconnect | mute | volume <0-2> |
    // overlay | fullscreen.
    {
        let cmd_tx = cmd_tx.clone();
        let proxy = proxy.clone();
        std::thread::spawn(move || {
            use mist_app_core::client::ClientCmd;
            for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                let words: Vec<&str> = line.split_whitespace().collect();
                let _ = match words.as_slice() {
                    ["disconnect"] => cmd_tx.send(ClientCmd::Disconnect),
                    ["mute"] => cmd_tx.send(ClientCmd::ToggleMute),
                    ["overlay"] => cmd_tx.send(ClientCmd::ToggleOverlay),
                    ["volume", v] => match v.parse::<f64>() {
                        Ok(v) => cmd_tx.send(ClientCmd::SetVolume(v)),
                        Err(_) => Ok(()),
                    },
                    ["fullscreen"] => {
                        let _ = proxy.send_event(UiEvent::ToggleFullscreen);
                        Ok(())
                    }
                    _ => Ok(()),
                };
            }
        });
    }
    let worker = std::thread::spawn(move || -> anyhow::Result<String> {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(async move {
            let (events, mut rx) = broadcast::channel(256);
            let printer = tokio::spawn(async move {
                while let Ok(ev) = rx.recv().await {
                    print_client_event(json, &ev);
                }
            });
            if let Some(secs) = duration {
                let tx = cmd_tx.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(secs)).await;
                    let _ = tx.send(mist_app_core::client::ClientCmd::Disconnect);
                });
            }
            let r = session::run(ctx, opts, shared2, sink, events, cmd_rx).await;
            let _ = proxy.send_event(UiEvent::Exit);
            tokio::time::sleep(Duration::from_millis(100)).await;
            printer.abort();
            r
        })
    });
    window::run(event_loop, win_opts, shared, frames)?;
    match worker.join() {
        Ok(r) => r.map(|_| ()),
        Err(_) => anyhow::bail!("session thread panicked"),
    }
}

// ------------------------------------------------------------ ice check

async fn ice_check(cli: &Cli, ctx: AppContext) -> anyhow::Result<()> {
    let mut servers = ctx.config.network.ice_servers.clone();
    let cfg = mist_signaling_client::SignalingConfig {
        url: ctx.config.network.signaling_url.clone(),
        device_name: ctx.config.device_name.clone(),
        role: Role::Client,
        access_token: ctx.secrets.signaling_access_token.clone(),
        extra_ca_pem: ctx.config.network.extra_ca_pem.clone(),
    };
    match mist_signaling_client::connect(&cfg, ctx.identity.clone()).await {
        Ok((c, _)) => servers.extend(c.ice_servers.clone()),
        Err(e) => eprintln!("signaling unreachable ({e}); checking locally configured servers only"),
    }
    let configured_turn = servers.iter().filter(|s| s.is_turn()).map(|s| s.to_string()).collect::<Vec<_>>();
    let ice = IceConfig { servers, relay_only: ctx.config.network.relay_only };
    let result =
        tokio::task::spawn_blocking(move || mist_media_engine::icecheck::check_ice(&ice, Duration::from_secs(10)))
            .await??;
    let verdict = if configured_turn.is_empty() {
        "no TURN server configured"
    } else if result.relay_candidates > 0 {
        "TURN server WORKS (relay address allocated)"
    } else {
        "TURN server configured but NOT working (no relay candidates)"
    };
    out(
        cli.json,
        &serde_json::json!({ "configured_turn": configured_turn, "result": result, "verdict": verdict }),
        || {
            format!(
                "host: {}  srflx (STUN): {}  relay (TURN): {} {:?}\nconfigured TURN: {:?}\n=> {verdict}",
                result.host_candidates,
                result.srflx_candidates,
                result.relay_candidates,
                result.relay_transports,
                configured_turn
            )
        },
    );
    Ok(())
}
