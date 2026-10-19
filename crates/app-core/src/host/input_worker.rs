//! Owns the `InputDispatcher` on a dedicated thread, shared by every
//! session. Dropping the worker (hosting stopped) releases every held input
//! and removes virtual controllers; a session leaving releases only its own.

use std::collections::HashMap;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use mist_controller_api::{ControllerBackend, MockController, Rumble, UnavailableController};
use mist_input_api::{create_backend, ClientId, InputDispatcher, InputPermissions, InputStats, TargetRect};

use crate::config::{ControllerChoice, HostConfig};

enum Cmd {
    Message(ClientId, String),
    AddClient(ClientId, InputPermissions),
    RemoveClient(ClientId),
    SetPermissions(ClientId, InputPermissions),
    SetAllowed(bool),
    ReleaseClient(ClientId),
    Stats(mpsc::Sender<WorkerStats>),
    Stop,
}

/// Input counters and controller assignments, for the host UI.
pub struct WorkerStats {
    pub keyboard_mouse_backend: &'static str,
    pub controller_backend: &'static str,
    /// Per client: its counters and the host controller slots it occupies.
    pub clients: HashMap<ClientId, (InputStats, Vec<u8>)>,
}

/// How often virtual controllers are polled for force feedback. Games block
/// in their (Linux) effect upload until it is answered, so keep this short.
const FEEDBACK_POLL: Duration = Duration::from_millis(4);

pub struct InputWorker {
    tx: mpsc::Sender<Cmd>,
    thread: Option<JoinHandle<()>>,
}

impl InputWorker {
    /// `on_rumble` receives force feedback from games for a client's pad
    /// (its own pad index), on the worker thread.
    pub fn start(
        cfg: &HostConfig,
        target: TargetRect,
        allowed: bool,
        on_rumble: impl Fn(ClientId, Rumble) + Send + 'static,
    ) -> anyhow::Result<Self> {
        let kbm = create_backend(cfg.input_backend, target)?;
        let pads: Box<dyn ControllerBackend> = match cfg.controllers {
            ControllerChoice::Platform => mist_controller_api::platform_backend(),
            ControllerChoice::Mock => Box::new(MockController::default()),
            ControllerChoice::Off => Box::new(UnavailableController("controllers disabled by host settings".into())),
        };
        tracing::info!(keyboard_mouse = kbm.name(), controllers = pads.name(), "input backends ready");
        let (tx, rx) = mpsc::channel::<Cmd>();
        let thread = std::thread::Builder::new().name("input-injection".into()).spawn(move || {
            let mut d = InputDispatcher::new(kbm, pads);
            d.set_allowed(allowed);
            let mut clients: Vec<ClientId> = Vec::new();
            loop {
                match rx.recv_timeout(FEEDBACK_POLL) {
                    Ok(Cmd::Message(client, text)) => {
                        d.handle_text(client, &text);
                    }
                    Ok(Cmd::AddClient(client, perms)) => {
                        d.add_client(client, perms);
                        if !clients.contains(&client) {
                            clients.push(client);
                        }
                    }
                    Ok(Cmd::RemoveClient(client)) => {
                        d.remove_client(client);
                        clients.retain(|c| *c != client);
                    }
                    Ok(Cmd::SetPermissions(client, perms)) => d.set_permissions(client, perms),
                    Ok(Cmd::SetAllowed(a)) => d.set_allowed(a),
                    Ok(Cmd::ReleaseClient(client)) => d.release_client(client),
                    Ok(Cmd::Stats(reply)) => {
                        let stats = WorkerStats {
                            keyboard_mouse_backend: d.backend_name(),
                            controller_backend: d.controller_backend_name(),
                            clients: clients
                                .iter()
                                .filter_map(|&c| d.client_stats(c).map(|s| (c, (s, d.client_pads(c)))))
                                .collect(),
                        };
                        let _ = reply.send(stats);
                    }
                    Ok(Cmd::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                for (client, r) in d.poll_feedback() {
                    on_rumble(client, r);
                }
            }
            d.disconnect();
            tracing::info!("input released and virtual devices removed");
        })?;
        Ok(InputWorker { tx, thread: Some(thread) })
    }

    pub fn message(&self, client: ClientId, text: String) {
        let _ = self.tx.send(Cmd::Message(client, text));
    }

    pub fn add_client(&self, client: ClientId, perms: InputPermissions) {
        let _ = self.tx.send(Cmd::AddClient(client, perms));
    }

    /// Release the client's input and remove its virtual pads.
    pub fn remove_client(&self, client: ClientId) {
        let _ = self.tx.send(Cmd::RemoveClient(client));
    }

    pub fn set_permissions(&self, client: ClientId, perms: InputPermissions) {
        let _ = self.tx.send(Cmd::SetPermissions(client, perms));
    }

    /// Host-wide switch over everyone's input.
    pub fn set_allowed(&self, allowed: bool) {
        let _ = self.tx.send(Cmd::SetAllowed(allowed));
    }

    /// Release one client's held keys/buttons/gamepad inputs without ending
    /// its session.
    pub fn release_client(&self, client: ClientId) {
        let _ = self.tx.send(Cmd::ReleaseClient(client));
    }

    pub fn stats(&self) -> Option<WorkerStats> {
        let (tx, rx) = mpsc::channel();
        self.tx.send(Cmd::Stats(tx)).ok()?;
        rx.recv_timeout(std::time::Duration::from_millis(200)).ok()
    }
}

impl Drop for InputWorker {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
