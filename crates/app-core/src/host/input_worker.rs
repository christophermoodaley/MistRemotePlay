//! Owns the `InputDispatcher` on a dedicated thread. Dropping the worker
//! (session end) releases every held input and removes virtual controllers.

use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use mist_controller_api::{ControllerBackend, MockController, Rumble, UnavailableController};
use mist_input_api::{create_backend, InputDispatcher, InputStats, TargetRect};

use crate::config::{ControllerChoice, HostConfig};

enum Cmd {
    Message(String),
    SetAllowed(bool),
    ReleaseAll,
    Stats(mpsc::Sender<(InputStats, &'static str, &'static str)>),
    Stop,
}

/// How often virtual controllers are polled for force feedback. Games block
/// in their (Linux) effect upload until it is answered, so keep this short.
const FEEDBACK_POLL: Duration = Duration::from_millis(4);

pub struct InputWorker {
    tx: mpsc::Sender<Cmd>,
    thread: Option<JoinHandle<()>>,
}

impl InputWorker {
    /// `on_rumble` receives force feedback from games (on the worker thread).
    pub fn start(
        cfg: &HostConfig,
        target: TargetRect,
        allowed: bool,
        on_rumble: impl Fn(Rumble) + Send + 'static,
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
            loop {
                match rx.recv_timeout(FEEDBACK_POLL) {
                    Ok(Cmd::Message(text)) => {
                        d.handle_text(&text);
                    }
                    Ok(Cmd::SetAllowed(a)) => d.set_allowed(a),
                    Ok(Cmd::ReleaseAll) => d.release_all(),
                    Ok(Cmd::Stats(reply)) => {
                        let _ = reply.send((d.stats(), d.backend_name(), d.controller_backend_name()));
                    }
                    Ok(Cmd::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                for r in d.poll_feedback() {
                    on_rumble(r);
                }
            }
            d.disconnect();
            tracing::info!("input released and virtual devices removed");
        })?;
        Ok(InputWorker { tx, thread: Some(thread) })
    }

    pub fn message(&self, text: String) {
        let _ = self.tx.send(Cmd::Message(text));
    }

    pub fn set_allowed(&self, allowed: bool) {
        let _ = self.tx.send(Cmd::SetAllowed(allowed));
    }

    /// Release held keys/buttons/gamepad inputs without ending the session.
    pub fn release_all(&self) {
        let _ = self.tx.send(Cmd::ReleaseAll);
    }

    pub fn stats(&self) -> Option<(InputStats, &'static str, &'static str)> {
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
