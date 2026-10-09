//! Keeps a signaling connection alive with backoff and re-delivers its
//! events. Network changes (e.g. Wi-Fi switch) surface as a disconnect
//! followed by an automatic reconnect.

use std::sync::Arc;
use std::time::Duration;

use mist_protocol::identity::DeviceIdentity;
use mist_protocol::signaling::{ClientToServer, IceServer};
use mist_session_manager::reconnect::ReconnectPolicy;
use mist_signaling_client::{connect, SignalingClient, SignalingConfig, SignalingEvent};
use parking_lot::Mutex;
use tokio::sync::{mpsc, watch};

#[derive(Debug)]
pub enum LinkEvent {
    Connected { ice_servers: Vec<IceServer> },
    Disconnected { reason: String, retry_in: Option<Duration> },
    Signal(SignalingEvent),
}

#[derive(Clone)]
pub struct LinkHandle {
    current: Arc<Mutex<Option<SignalingClient>>>,
    _shutdown: Arc<watch::Sender<bool>>,
}

impl LinkHandle {
    pub async fn send(&self, msg: ClientToServer) -> anyhow::Result<()> {
        let client = self.current.lock().clone();
        match client {
            Some(c) => Ok(c.send(msg).await?),
            None => anyhow::bail!("not connected to the signaling server"),
        }
    }

    pub fn connected(&self) -> bool {
        self.current.lock().as_ref().map(|c| !c.is_closed()).unwrap_or(false)
    }

    pub fn ice_servers(&self) -> Vec<IceServer> {
        self.current.lock().as_ref().map(|c| c.ice_servers.clone()).unwrap_or_default()
    }
}

/// Start the link; it stops when every `LinkHandle` clone is dropped.
pub fn spawn(cfg: SignalingConfig, identity: Arc<DeviceIdentity>) -> (LinkHandle, mpsc::Receiver<LinkEvent>) {
    let (tx, rx) = mpsc::channel(128);
    let current = Arc::new(Mutex::new(None));
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let handle = LinkHandle { current: current.clone(), _shutdown: Arc::new(shutdown_tx) };
    tokio::spawn(async move {
        let mut policy = ReconnectPolicy::signaling();
        loop {
            let attempt = tokio::select! {
                r = connect(&cfg, identity.clone()) => r,
                _ = shutdown_rx.changed() => return,
            };
            match attempt {
                Ok((client, mut events)) => {
                    policy.reset();
                    let ice_servers = client.ice_servers.clone();
                    *current.lock() = Some(client);
                    if tx.send(LinkEvent::Connected { ice_servers }).await.is_err() {
                        return;
                    }
                    let reason = loop {
                        tokio::select! {
                            ev = events.recv() => match ev {
                                Some(SignalingEvent::Disconnected { reason }) => break reason,
                                Some(ev) => {
                                    if tx.send(LinkEvent::Signal(ev)).await.is_err() {
                                        return;
                                    }
                                }
                                None => break "signaling task ended".to_string(),
                            },
                            _ = shutdown_rx.changed() => return,
                        }
                    };
                    *current.lock() = None;
                    let delay = policy.next_delay();
                    let _ = tx.send(LinkEvent::Disconnected { reason, retry_in: delay }).await;
                    match delay {
                        Some(d) => tokio::time::sleep(d).await,
                        None => return,
                    }
                }
                Err(e) => {
                    let delay = policy.next_delay();
                    tracing::warn!("signaling connection failed: {e}");
                    if tx.send(LinkEvent::Disconnected { reason: e.to_string(), retry_in: delay }).await.is_err() {
                        return;
                    }
                    match delay {
                        Some(d) => {
                            tokio::select! {
                                _ = tokio::time::sleep(d) => {}
                                _ = shutdown_rx.changed() => return,
                            }
                        }
                        None => return,
                    }
                }
            }
        }
    });
    (handle, rx)
}
