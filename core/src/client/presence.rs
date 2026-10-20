//! One-shot presence query for saved hosts (online/offline in the UI).

use std::time::Duration;

use crate::protocol::identity::DeviceId;
use crate::protocol::signaling::{ClientToServer, Role};
use crate::signaling::{connect, SignalingEvent};
use serde::Serialize;

use crate::AppContext;

#[derive(Debug, Clone, Serialize, Default)]
pub struct Presence {
    pub online: Vec<String>,
    pub offline: Vec<String>,
}

pub async fn query(ctx: &AppContext, ids: Vec<DeviceId>) -> anyhow::Result<Presence> {
    // Hosts with a built-in server each answer for themselves, so ask each
    // device's own server (one connection per distinct server).
    let mut by_url: std::collections::BTreeMap<String, Vec<DeviceId>> = Default::default();
    {
        let trust = ctx.trust.lock();
        for id in ids {
            let url = trust
                .get(&id)
                .and_then(|d| d.signaling_url.clone())
                .unwrap_or_else(|| ctx.config.network.signaling_url.clone());
            by_url.entry(url).or_default().push(id);
        }
    }
    let mut all = Presence::default();
    let mut last_err = None;
    let results = futures::future::join_all(by_url.into_iter().map(|(url, ids)| async move {
        let r = tokio::time::timeout(Duration::from_secs(4), query_one(ctx, &url, ids.clone()))
            .await
            .unwrap_or_else(|_| Err(anyhow::anyhow!("no answer from {url}")));
        (url, ids, r)
    }))
    .await;
    for (url, ids, r) in results {
        match r {
            Ok(p) => {
                all.online.extend(p.online);
                all.offline.extend(p.offline);
            }
            // An unreachable built-in server means that host is offline.
            Err(e) => {
                tracing::debug!(%url, "presence query failed: {e}");
                all.offline.extend(ids.iter().map(|d| d.to_string()));
                last_err = Some(e);
            }
        }
    }
    if all.online.is_empty() && all.offline.is_empty() {
        if let Some(e) = last_err {
            return Err(e);
        }
    }
    Ok(all)
}

async fn query_one(ctx: &AppContext, url: &str, ids: Vec<DeviceId>) -> anyhow::Result<Presence> {
    if ids.is_empty() {
        return Ok(Presence::default());
    }
    let cfg = ctx.signaling_config(url, Role::Client);
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
