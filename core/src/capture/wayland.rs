//! Wayland capture through xdg-desktop-portal ScreenCast + PipeWire.
//!
//! The compositor shows its own picker; the user chooses the monitor there.
//! The portal session is kept alive on a dedicated thread for as long as the
//! returned [`PortalLease`] exists. A restore token is persisted (when a path
//! is configured) so later sessions can skip the picker if the compositor
//! supports it.

use std::os::fd::{AsRawFd, OwnedFd};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};

use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::PersistMode;

use crate::capture::{CaptureBackend, CaptureError, CaptureKind, CaptureLease, CaptureSourceSpec, DisplayInfo, Result};

#[derive(Default)]
pub struct PortalCapture {
    pub restore_token_path: Option<PathBuf>,
}

/// Keeps the PipeWire fd open and the portal session alive.
pub struct PortalLease {
    _fd: OwnedFd,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl std::fmt::Debug for PortalLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PortalLease")
    }
}

impl CaptureLease for PortalLease {}

impl Drop for PortalLease {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

struct Started {
    fd: OwnedFd,
    node_id: u32,
    size: Option<(i32, i32)>,
    stop: tokio::sync::oneshot::Sender<()>,
}

impl PortalCapture {
    fn run_portal(&self) -> Result<Started> {
        let token_path = self.restore_token_path.clone();
        let (tx, rx) = mpsc::channel::<Result<Started>>();
        std::thread::Builder::new()
            .name("portal-screencast".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = tx.send(Err(CaptureError::Backend(e.to_string())));
                        return;
                    }
                };
                rt.block_on(async move {
                    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
                    let restore = token_path.as_ref().and_then(|p| std::fs::read_to_string(p).ok());
                    let result = async {
                        let proxy = Screencast::new().await.map_err(portal_err)?;
                        let session = proxy.create_session(Default::default()).await.map_err(portal_err)?;
                        proxy
                            .select_sources(
                                &session,
                                SelectSourcesOptions::default()
                                    .set_cursor_mode(CursorMode::Embedded)
                                    .set_sources(ashpd::enumflags2::BitFlags::from(SourceType::Monitor))
                                    .set_multiple(false)
                                    .set_persist_mode(PersistMode::ExplicitlyRevoked)
                                    .set_restore_token(restore.as_deref().map(str::trim)),
                            )
                            .await
                            .map_err(portal_err)?;
                        let response = proxy
                            .start(&session, None, Default::default())
                            .await
                            .map_err(portal_err)?
                            .response()
                            .map_err(portal_err)?;
                        if let (Some(path), Some(token)) = (&token_path, response.restore_token()) {
                            if let Err(e) = std::fs::write(path, token) {
                                tracing::warn!("could not persist portal restore token: {e}");
                            }
                        }
                        let stream = response
                            .streams()
                            .first()
                            .ok_or_else(|| CaptureError::Backend("portal returned no streams".into()))?;
                        let node_id = stream.pipe_wire_node_id();
                        let size = stream.size();
                        let fd = proxy.open_pipe_wire_remote(&session, Default::default()).await.map_err(portal_err)?;
                        Ok::<_, CaptureError>((session, fd, node_id, size))
                    }
                    .await;
                    match result {
                        Ok((session, fd, node_id, size)) => {
                            let _ = tx.send(Ok(Started { fd, node_id, size, stop: stop_tx }));
                            // Hold the session until the lease is dropped.
                            let _ = stop_rx.await;
                            if let Err(e) = session.close().await {
                                tracing::debug!("closing portal session: {e}");
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Err(e));
                        }
                    }
                });
            })
            .map_err(|e| CaptureError::Backend(e.to_string()))?;
        rx.recv().map_err(|_| CaptureError::Backend("portal thread exited".into()))?
    }
}

fn portal_err(e: ashpd::Error) -> CaptureError {
    match e {
        ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled) => CaptureError::Cancelled,
        e => CaptureError::Backend(format!("xdg-desktop-portal: {e}")),
    }
}

impl CaptureBackend for PortalCapture {
    fn kind(&self) -> CaptureKind {
        CaptureKind::WaylandPortal
    }

    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        Ok(vec![DisplayInfo {
            id: "portal".into(),
            name: "Choose in system dialog (Wayland)".into(),
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            primary: true,
            backend: CaptureKind::WaylandPortal,
        }])
    }

    fn open(&self, _display_id: &str) -> Result<CaptureSourceSpec> {
        let started = self.run_portal()?;
        let raw = started.fd.as_raw_fd();
        let lease = PortalLease { _fd: started.fd, stop: Some(started.stop) };
        Ok(CaptureSourceSpec::PipeWire {
            fd: raw,
            node_id: started.node_id,
            width: started.size.map(|s| s.0 as u32),
            height: started.size.map(|s| s.1 as u32),
            lease: Arc::new(lease),
        })
    }
}
