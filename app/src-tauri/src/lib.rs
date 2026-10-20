//! Tauri shell. The web UI calls these commands; all privileged work
//! (capture, injection, networking, secrets) stays in Rust (`mist-core`).
//! The UI never receives secrets: the access token is write-only from its
//! side.

mod client_process;
mod overlay;

use std::sync::Arc;
use std::time::Duration;

use mist_core::config::AppConfig;
use mist_core::diagnostics::LogBuffer;
use mist_core::host::{self, HostHandle, HostSnapshot};
use mist_core::protocol::identity::DeviceId;
use mist_core::sessions::trust::PairedDevice;
use mist_core::sessions::{SessionManager, SessionRole};
use mist_core::AppContext;
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use client_process::ClientProcess;

struct HostRuntime {
    handle: HostHandle,
    forwarder: tauri::async_runtime::JoinHandle<()>,
    _advertiser: Option<mist_core::discovery::Advertiser>,
}

pub struct AppState {
    ctx: Mutex<AppContext>,
    logs: LogBuffer,
    host: Mutex<Option<HostRuntime>>,
    client: Arc<Mutex<Option<ClientProcess>>>,
}

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[derive(Serialize)]
struct AppInfo {
    version: String,
    device_id: String,
    device_name: String,
    fingerprint: String,
    config_dir: String,
    has_access_token: bool,
    has_github_token: bool,
    platform: String,
}

#[tauri::command]
fn app_info(state: State<'_, AppState>) -> AppInfo {
    let ctx = state.ctx.lock();
    AppInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        device_id: ctx.identity.device_id().to_string(),
        device_name: ctx.config.device_name.clone(),
        fingerprint: ctx.identity.device_id().fingerprint(),
        config_dir: ctx.dir.display().to_string(),
        has_access_token: ctx.secrets.signaling_access_token.is_some(),
        has_github_token: ctx.secrets.github_token.is_some(),
        platform: std::env::consts::OS.into(),
    }
}

#[tauri::command]
fn get_config(state: State<'_, AppState>) -> AppConfig {
    state.ctx.lock().config.clone()
}

#[tauri::command]
fn save_config(app: AppHandle, state: State<'_, AppState>, config: AppConfig) -> CmdResult<()> {
    config.validate().map_err(err)?;
    let mut ctx = state.ctx.lock();
    ctx.config = config;
    ctx.save_config().map_err(err)?;
    // The connection overlay may have been switched on or off. Off the main
    // thread: it may create a window.
    tauri::async_runtime::spawn(async move { refresh_overlay(&app) });
    Ok(())
}

/// Write-only: the UI can set or clear the token but never read it back.
#[tauri::command]
fn set_access_token(state: State<'_, AppState>, token: Option<String>) -> CmdResult<()> {
    let mut ctx = state.ctx.lock();
    ctx.secrets.signaling_access_token = token.filter(|t| !t.trim().is_empty());
    ctx.secrets.save(&ctx.dir).map_err(err)
}

// ---------------------------------------------------------------- updates

/// Write-only, like the access token: stored DPAPI-protected, never sent back.
#[tauri::command]
fn set_github_token(state: State<'_, AppState>, token: Option<String>) -> CmdResult<()> {
    let mut ctx = state.ctx.lock();
    ctx.secrets.github_token = token.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
    ctx.secrets.save(&ctx.dir).map_err(err)
}

/// Open a GitHub page (token creation, release notes) in the browser. Only
/// https://github.com/ URLs are accepted.
#[tauri::command]
fn open_external(url: String) -> CmdResult<()> {
    let ok = url::Url::parse(&url).ok().is_some_and(|u| u.scheme() == "https" && u.host_str() == Some("github.com"));
    if !ok {
        return Err("only github.com links can be opened".into());
    }
    #[cfg(windows)]
    let r = {
        use std::os::windows::process::CommandExt;
        // rundll32 opens the default browser without a console window or shell parsing.
        std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", &url])
            .creation_flags(0x0800_0000)
            .spawn()
    };
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(&url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(&url).spawn();
    r.map(|_| ()).map_err(err)
}

#[tauri::command]
async fn update_check(state: State<'_, AppState>) -> CmdResult<Option<mist_core::updater::UpdateInfo>> {
    let token = state.ctx.lock().secrets.github_token.clone();
    mist_core::updater::check(token.as_deref(), None).await.map_err(err)
}

#[derive(Clone, Serialize)]
struct UpdateProgress {
    stage: &'static str,
    done: u64,
    total: u64,
}

/// Download and verify the update, stop hosting/streaming, start the
/// installer and quit; the installer starts the new version.
#[tauri::command]
async fn update_install(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    use mist_core::updater;
    if !cfg!(windows) {
        return Err(updater::UpdateError::Unsupported.to_string());
    }
    let token = state.ctx.lock().secrets.github_token.clone();
    let info = updater::check(token.as_deref(), None).await.map_err(err)?.ok_or("already up to date")?;
    let app2 = app.clone();
    let mut last = std::time::Instant::now() - Duration::from_secs(1);
    let path = updater::download(&info, token.as_deref(), &updater::download_dir(), move |done, total| {
        // A few progress events per second are plenty for the UI.
        if last.elapsed() >= Duration::from_millis(150) || done == total {
            last = std::time::Instant::now();
            let _ = app2.emit("update-progress", UpdateProgress { stage: "downloading", done, total });
        }
    })
    .await
    .map_err(err)?;
    let _ = app.emit("update-progress", UpdateProgress { stage: "installing", done: 1, total: 1 });
    // End sessions cleanly before the installer closes the app.
    if let Some(rt) = state.host.lock().take() {
        rt.handle.disconnect_all();
    }
    if let Some(c) = state.client.lock().as_mut() {
        let _ = c.send("disconnect");
    }
    tokio::time::sleep(Duration::from_millis(700)).await;
    updater::launch_installer(&path).map_err(err)?;
    app.exit(0);
    Ok(())
}

#[tauri::command]
async fn system_report(state: State<'_, AppState>) -> CmdResult<serde_json::Value> {
    let ctx = state.ctx.lock().clone();
    let report =
        tauri::async_runtime::spawn_blocking(move || mist_core::report::system_report(&ctx)).await.map_err(err)?;
    serde_json::to_value(report).map_err(err)
}

#[tauri::command]
fn get_logs(state: State<'_, AppState>) -> Vec<mist_core::diagnostics::LogLine> {
    state.logs.snapshot()
}

// ------------------------------------------------------------------ host

/// Match the connection overlay to who is connected now.
fn refresh_overlay(app: &AppHandle) {
    let state = app.state::<AppState>();
    let enabled = state.ctx.lock().config.host.connection_overlay;
    let snapshot = state.host.lock().as_ref().map(|h| h.handle.snapshot());
    let viewers = match snapshot {
        Some(s) if enabled => overlay::viewers(&s),
        _ => Vec::new(),
    };
    overlay::sync(app, viewers);
}

#[tauri::command]
async fn host_start(app: AppHandle, state: State<'_, AppState>) -> CmdResult<HostSnapshot> {
    if let Some(h) = state.host.lock().as_ref() {
        return Ok(h.handle.snapshot());
    }
    let ctx = state.ctx.lock().clone();
    let (device_id, name, sig_url, advertise) = (
        ctx.identity.device_id().to_string(),
        ctx.config.device_name.clone(),
        ctx.config.network.signaling_url.clone(),
        ctx.config.network.lan_discovery,
    );
    let handle = host::start_hosting(ctx).map_err(|e| e.to_string())?;
    let advertiser = if advertise {
        mist_core::discovery::Advertiser::start(&device_id, &name, &sig_url, handle.snapshot().server.as_ref())
            .map_err(|e| tracing::warn!("LAN advertisement unavailable: {e}"))
            .ok()
    } else {
        None
    };
    let mut rx = handle.subscribe();
    let app2 = app.clone();
    let forwarder = tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let _ = app2.emit("host-event", &ev);
                    refresh_overlay(&app2);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
    let snapshot = handle.snapshot();
    *state.host.lock() = Some(HostRuntime { handle, forwarder, _advertiser: advertiser });
    Ok(snapshot)
}

#[tauri::command]
async fn host_stop(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    let rt = state.host.lock().take();
    if let Some(rt) = rt {
        rt.handle.disconnect_all();
        tokio::time::sleep(Duration::from_millis(300)).await;
        rt.forwarder.abort();
    }
    refresh_overlay(&app);
    Ok(())
}

fn with_host<T>(state: &State<'_, AppState>, f: impl FnOnce(&HostHandle) -> T) -> CmdResult<T> {
    state.host.lock().as_ref().map(|h| f(&h.handle)).ok_or_else(|| "the host service is not running".to_string())
}

#[tauri::command]
fn host_snapshot(state: State<'_, AppState>) -> Option<HostSnapshot> {
    state.host.lock().as_ref().map(|h| h.handle.snapshot())
}

/// Check that other PCs can reach this host's built-in server over the
/// internet (see `mist_core::reachability`).
#[tauri::command]
async fn host_test_connection(state: State<'_, AppState>) -> CmdResult<mist_core::reachability::ConnectionTest> {
    let server = state.host.lock().as_ref().and_then(|h| h.handle.snapshot().server);
    let server = server.ok_or("start hosting with Mist's own server first")?;
    let upnp = state.ctx.lock().config.host.upnp;
    Ok(mist_core::reachability::test(server.port, &server.fingerprint, upnp).await)
}

#[tauri::command]
fn host_generate_pairing_code(state: State<'_, AppState>) -> CmdResult<()> {
    with_host(&state, |h| h.generate_pairing_code())
}

#[tauri::command]
fn host_cancel_pairing(state: State<'_, AppState>) -> CmdResult<()> {
    with_host(&state, |h| h.cancel_pairing())
}

#[tauri::command]
fn host_decide_pairing(state: State<'_, AppState>, request_id: String, approve: bool) -> CmdResult<()> {
    with_host(&state, |h| h.decide_pairing(&request_id, approve))?.map_err(err)
}

#[tauri::command]
fn host_decide_session(
    state: State<'_, AppState>,
    session_id: String,
    approve: bool,
    role: Option<SessionRole>,
) -> CmdResult<()> {
    with_host(&state, |h| {
        if approve {
            h.approve_as(&session_id, role.unwrap_or_default())
        } else {
            h.reject(&session_id, "declined by the host")
        }
    })?
    .map_err(err)
}

#[tauri::command]
fn host_end_session(state: State<'_, AppState>, session_id: String) -> CmdResult<()> {
    with_host(&state, |h| h.end(&session_id))?.map_err(err)
}

#[tauri::command]
fn host_set_role(state: State<'_, AppState>, session_id: String, role: SessionRole) -> CmdResult<()> {
    with_host(&state, |h| h.set_role(&session_id, role))?.map_err(err)
}

#[tauri::command]
fn host_set_keyboard_mouse(state: State<'_, AppState>, session_id: String, allowed: bool) -> CmdResult<()> {
    with_host(&state, |h| h.set_keyboard_mouse(&session_id, allowed))?.map_err(err)
}

#[tauri::command]
fn host_set_input_allowed(state: State<'_, AppState>, allowed: bool) -> CmdResult<()> {
    with_host(&state, |h| h.set_input_allowed(allowed))
}

#[tauri::command]
fn host_disconnect_all(state: State<'_, AppState>) -> CmdResult<()> {
    with_host(&state, |h| h.disconnect_all())
}

#[tauri::command]
fn host_revoke_device(state: State<'_, AppState>, device_id: String) -> CmdResult<()> {
    let id = DeviceId::parse(&device_id).map_err(err)?;
    let running = state.host.lock().as_ref().map(|h| h.handle.revoke_device(id.clone())).is_some();
    if !running {
        state.ctx.lock().trust.lock().remove(&id).map_err(err)?;
    }
    Ok(())
}

#[tauri::command]
async fn list_displays() -> CmdResult<serde_json::Value> {
    let v = tauri::async_runtime::spawn_blocking(host::available_capture_backends).await.map_err(err)?;
    serde_json::to_value(
        v.into_iter()
            .map(|(backend, displays, error)| serde_json::json!({ "backend": backend, "displays": displays, "error": error }))
            .collect::<Vec<_>>(),
    )
    .map_err(err)
}

// ---------------------------------------------------------------- client

/// Physical controllers currently visible to this computer (for the
/// mapping editor). Briefly opens the platform controller API.
/// Programs playing sound now (for choosing which apps the host streams).
#[tauri::command]
async fn audio_apps() -> CmdResult<Vec<mist_core::audio::apps::AudioApp>> {
    tauri::async_runtime::spawn_blocking(mist_core::audio::apps::audio_apps).await.map_err(err)?.map_err(err)
}

#[tauri::command]
async fn detect_controllers(
    state: State<'_, AppState>,
) -> CmdResult<Vec<mist_core::controller::source::ConnectedController>> {
    let mapping = state.ctx.lock().config.client.controller_mapping.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (tx, _rx) = std::sync::mpsc::channel();
        let src = mist_core::controller::source::ControllerSource::start(mapping, tx).map_err(err)?;
        // Windows lists controllers, Bluetooth ones especially, a moment
        // after the controller library starts.
        std::thread::sleep(Duration::from_millis(1500));
        Ok(src.controllers())
    })
    .await
    .map_err(err)?
}

#[tauri::command]
fn paired_devices(state: State<'_, AppState>) -> Vec<PairedDevice> {
    state.ctx.lock().trust.lock().list()
}

#[tauri::command]
fn forget_device(state: State<'_, AppState>, device_id: String) -> CmdResult<()> {
    let id = DeviceId::parse(&device_id).map_err(err)?;
    state.ctx.lock().trust.lock().remove(&id).map(|_| ()).map_err(err)
}

#[tauri::command]
async fn pair_with_code(
    state: State<'_, AppState>,
    code: String,
    signaling_url: Option<String>,
) -> CmdResult<PairedDevice> {
    let mut ctx = state.ctx.lock().clone();
    if let Some(url) = signaling_url.filter(|u| !u.trim().is_empty()) {
        ctx.config.network.signaling_url = mist_core::known_servers::normalize_address(&url).map_err(err)?;
    }
    mist_core::client::pair(&ctx, &code).await.map_err(|e| {
        // The UI recognises this form and asks whether to trust the server.
        let untrusted = e.chain().find_map(|c| match c.downcast_ref::<mist_core::signaling::SignalingError>() {
            Some(mist_core::signaling::SignalingError::UntrustedCertificate { fingerprint }) => {
                Some(fingerprint.clone())
            }
            _ => None,
        });
        match untrusted {
            Some(fp) => format!("UNTRUSTED_CERTIFICATE {fp} {}", ctx.config.network.signaling_url),
            None => err(e),
        }
    })
}

/// Pin a server certificate the user accepted (see known_servers).
#[tauri::command]
fn trust_server(state: State<'_, AppState>, url: String, fingerprint: String) -> CmdResult<()> {
    let dir = state.ctx.lock().dir.clone();
    let url = mist_core::known_servers::normalize_address(&url).map_err(err)?;
    mist_core::known_servers::trust(&dir, &url, &fingerprint).map_err(err)
}

#[tauri::command]
async fn discover_lan(secs: Option<u64>) -> CmdResult<Vec<mist_core::discovery::DiscoveredHost>> {
    let secs = secs.unwrap_or(3).clamp(1, 10);
    tauri::async_runtime::spawn_blocking(move || mist_core::discovery::browse(Duration::from_secs(secs)))
        .await
        .map_err(err)?
        .map_err(err)
}

#[tauri::command]
async fn host_presence(state: State<'_, AppState>) -> CmdResult<mist_core::client::presence::Presence> {
    let ctx = state.ctx.lock().clone();
    let ids = ctx.trust.lock().list().into_iter().map(|d| d.device_id).collect();
    mist_core::client::presence::query(&ctx, ids).await.map_err(err)
}

#[tauri::command]
async fn ice_check(state: State<'_, AppState>) -> CmdResult<serde_json::Value> {
    let ctx = state.ctx.lock().clone();
    client_process::run_ice_check(&ctx).await.map_err(err)
}

#[tauri::command]
fn client_connect(app: AppHandle, state: State<'_, AppState>, device_id: String) -> CmdResult<()> {
    let mut slot = state.client.lock();
    if slot.as_mut().map(|c| c.running()).unwrap_or(false) {
        return Err("a streaming session is already running".into());
    }
    let ctx = state.ctx.lock().clone();
    let id = DeviceId::parse(&device_id).map_err(err)?;
    if ctx.trust.lock().get(&id).is_none() {
        return Err("this host is not paired".into());
    }
    *slot = Some(ClientProcess::spawn(app, &ctx, &id, state.client.clone()).map_err(err)?);
    Ok(())
}

#[tauri::command]
fn client_command(state: State<'_, AppState>, command: String) -> CmdResult<()> {
    let allowed = ["disconnect", "mute", "overlay", "fullscreen"];
    let ok = allowed.contains(&command.as_str())
        || command
            .strip_prefix("volume ")
            .and_then(|v| v.parse::<f64>().ok())
            .map(|v| (0.0..=2.0).contains(&v))
            .unwrap_or(false);
    if !ok {
        return Err("unknown command".into());
    }
    match state.client.lock().as_mut() {
        Some(c) => c.send(&command).map_err(err),
        None => Err("no streaming session".into()),
    }
}

#[tauri::command]
fn client_running(state: State<'_, AppState>) -> bool {
    state.client.lock().as_mut().map(|c| c.running()).unwrap_or(false)
}

pub fn run() {
    let logs = mist_core::diagnostics::init(mist_core::diagnostics::LogFormat::Text);
    let ctx = AppContext::load().expect("cannot load configuration");
    tauri::Builder::default()
        .manage(overlay::OverlayState::default())
        .manage(AppState { ctx: Mutex::new(ctx), logs, host: Mutex::new(None), client: Arc::new(Mutex::new(None)) })
        .invoke_handler(tauri::generate_handler![
            app_info,
            get_config,
            save_config,
            set_access_token,
            set_github_token,
            update_check,
            update_install,
            open_external,
            system_report,
            get_logs,
            host_start,
            host_stop,
            host_snapshot,
            host_test_connection,
            host_generate_pairing_code,
            host_cancel_pairing,
            host_decide_pairing,
            host_decide_session,
            host_end_session,
            host_set_role,
            host_set_keyboard_mouse,
            host_set_input_allowed,
            host_disconnect_all,
            host_revoke_device,
            list_displays,
            audio_apps,
            detect_controllers,
            paired_devices,
            forget_device,
            pair_with_code,
            trust_server,
            discover_lan,
            host_presence,
            ice_check,
            client_connect,
            client_command,
            client_running,
            overlay::overlay_viewers,
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                if window.label() != "main" {
                    return;
                }
                // Closing the app ends hosting and streaming cleanly.
                let state = window.state::<AppState>();
                let host = state.host.lock().take();
                if let Some(rt) = host {
                    rt.handle.disconnect_all();
                }
                let mut client = state.client.lock();
                if let Some(c) = client.as_mut() {
                    let _ = c.send("disconnect");
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Mist Remote Play");
}
