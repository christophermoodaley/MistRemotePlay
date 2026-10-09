//! The streaming client runs as a child `mist connect` process: a native
//! window gives the lowest-latency rendering and raw input capture. Its
//! JSON-line events are forwarded to the UI as `client-event`; control
//! commands go over its stdin.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;

use mist_app_core::AppContext;
use mist_protocol::identity::DeviceId;
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter};

pub struct ClientProcess {
    child: Child,
    stdin: Option<ChildStdin>,
}

/// Locate the `mist` CLI: explicit override, bundled sidecar next to the
/// app executable, or PATH.
pub fn mist_binary() -> PathBuf {
    if let Some(p) = std::env::var_os("MIST_CLI_PATH") {
        return PathBuf::from(p);
    }
    let exe_name = if cfg!(windows) { "mist.exe" } else { "mist" };
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(exe_name);
            if candidate.exists() {
                return candidate;
            }
        }
    }
    PathBuf::from(exe_name)
}

impl ClientProcess {
    pub fn spawn(
        app: AppHandle,
        ctx: &AppContext,
        host: &DeviceId,
        slot: Arc<Mutex<Option<ClientProcess>>>,
    ) -> anyhow::Result<Self> {
        let mut cmd = Command::new(mist_binary());
        cmd.arg("--json")
            .arg("--config-dir")
            .arg(&ctx.dir)
            .arg("connect")
            .arg(host.as_str())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("cannot start the streaming client ({}): {e}", mist_binary().display()))?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stdin = child.stdin.take();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                match serde_json::from_str::<serde_json::Value>(&line) {
                    Ok(v) => {
                        let _ = app.emit("client-event", v);
                    }
                    Err(_) => tracing::debug!("client: {line}"),
                }
            }
            // Process ended: report its exit status.
            let code = slot.lock().as_mut().and_then(|c| c.child.wait().ok()).and_then(|s| s.code());
            let _ = app.emit("client-exit", serde_json::json!({ "code": code }));
        });
        Ok(ClientProcess { child, stdin })
    }

    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn send(&mut self, line: &str) -> std::io::Result<()> {
        match self.stdin.as_mut() {
            Some(s) => {
                s.write_all(line.as_bytes())?;
                s.write_all(b"\n")?;
                s.flush()
            }
            None => Err(std::io::Error::other("client stdin closed")),
        }
    }
}

impl Drop for ClientProcess {
    fn drop(&mut self) {
        let _ = self.send("disconnect");
    }
}

pub async fn run_ice_check(ctx: &AppContext) -> anyhow::Result<serde_json::Value> {
    let out = tokio::process::Command::new(mist_binary())
        .arg("--json")
        .arg("--config-dir")
        .arg(&ctx.dir)
        .arg("ice-check")
        .output()
        .await?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let last = stdout.lines().rev().find(|l| l.trim_start().starts_with('{')).unwrap_or("{}");
    Ok(serde_json::from_str(last)?)
}
