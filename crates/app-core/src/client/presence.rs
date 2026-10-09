//! One-shot presence query for saved hosts (online/offline in the UI).

use std::time::Duration;

use mist_protocol::identity::DeviceId;
use mist_protocol::signaling::{ClientToServer, Role};
use mist_signaling_client::{connect, SignalingConfig, SignalingEvent};
use serde::Serialize;

use crate::AppContext;

#[derive(Debug, Clone, Serialize, Default)]
pub struct Presence {
    pub online: Vec<String>,
    pub offline: Vec<String>,
}

pub async fn query(ctx: &AppContext, ids: Vec<DeviceId>) -> anyhow::Result<Presence> {
    if ids.is_empty() {
        return Ok(Presence::default());
    }
    let cfg = SignalingConfig {
        url: ctx.config.network.signaling_url.clone(),
        device_name: ctx.config.device_name.clone(),
        role: Role::Client,
        access_token: ctx.secrets.signaling_access_token.clone(),
        extra_ca_pem: ctx.config.network.extra_ca_pem.clone(),
    };
    let (client, mut events) = connect(&cfg, ctx.identity.clone()).await?;
    client.send(ClientToServer::QueryPresence { device_ids: ids }).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(SignalingEvent::Presence { online, offline })) => {
                return Ok(Presence {
                    online: online.into_iter().map(|d| d.to_string()).collect(),
                    offline: offline.into_iter().map(|d| d.to_string()).collect(),
                })
            }
            Ok(Some(_)) => continue,
            Ok(None) => anyhow::bail!("signaling connection closed"),
            Err(_) => anyhow::bail!("presence query timed out"),
        }
    }
}
