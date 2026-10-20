//! Client side of pairing: prove knowledge of the code shown on the host,
//! wait for the host user's approval, verify the host's counter-proof and
//! remember the host's key.

use std::time::Duration;

use crate::protocol::identity::random_hex;
use crate::protocol::pairing::PairingCode;
use crate::protocol::peer::{now_ms, PeerMessage, SignedEnvelope};
use crate::protocol::signaling::{ClientToServer, ErrorCode, Role};
use crate::sessions::trust::PairedDevice;
use crate::signaling::{connect, SignalingEvent};

use crate::AppContext;

/// Host approval can take a while (a human must click "Approve").
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(130);

pub async fn pair(ctx: &AppContext, code_input: &str) -> anyhow::Result<PairedDevice> {
    let code = PairingCode::parse(code_input).map_err(|e| anyhow::anyhow!("{e}"))?;
    let cfg = ctx.signaling_config(&ctx.config.network.signaling_url, Role::Client);
    let (client, mut events) = connect(&cfg, ctx.identity.clone()).await?;
    let my_pk = ctx.identity.public_key_b64();
    let nonce = random_hex(16);
    let proof = code.request_proof(&my_pk.0, &nonce);
    let env = SignedEnvelope::seal(
        &ctx.identity,
        None,
        PeerMessage::PairRequest {
            client_name: ctx.config.device_name.clone(),
            client_public_key: my_pk.clone(),
            nonce: nonce.clone(),
            proof,
        },
    );
    client.send(ClientToServer::PairRequest { lookup: code.lookup(), envelope: env }).await?;
    tracing::info!("pairing request sent; waiting for the host to approve");

    let me = ctx.identity.device_id();
    let deadline = tokio::time::Instant::now() + APPROVAL_TIMEOUT;
    loop {
        let ev = tokio::time::timeout_at(deadline, events.recv())
            .await
            .map_err(|_| anyhow::anyhow!("the host did not respond in time"))?
            .ok_or_else(|| anyhow::anyhow!("signaling connection closed"))?;
        match ev {
            SignalingEvent::ServerError { code: ErrorCode::UnknownPairingCode, .. } => {
                anyhow::bail!("unknown or expired pairing code (check the code shown on the host)")
            }
            SignalingEvent::ServerError { code: ErrorCode::RateLimited, message } => anyhow::bail!("{message}"),
            SignalingEvent::ServerError { code, message } => tracing::warn!(?code, "{message}"),
            SignalingEvent::Disconnected { reason } => anyhow::bail!("signaling connection lost: {reason}"),
            SignalingEvent::Relay { from, from_public_key, envelope } => {
                let body = match envelope.open(&from_public_key, Some(&me)) {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::warn!("ignoring invalid envelope: {e}");
                        continue;
                    }
                };
                match body.message {
                    PeerMessage::PairAccepted { host_name, host_public_key, proof } => {
                        if host_public_key != from_public_key {
                            anyhow::bail!("host key mismatch; pairing aborted");
                        }
                        if !code.verify_accept_proof(&host_public_key.0, &my_pk.0, &nonce, &proof) {
                            anyhow::bail!("host could not prove knowledge of the pairing code; pairing aborted");
                        }
                        let device = PairedDevice {
                            device_id: from,
                            name: host_name,
                            public_key: host_public_key,
                            paired_at_ms: now_ms(),
                            last_connected_ms: None,
                            signaling_url: Some(ctx.config.network.signaling_url.clone()),
                        };
                        ctx.trust.lock().add(device.clone())?;
                        tracing::info!(host = %device.device_id, name = %device.name, "paired with host");
                        return Ok(device);
                    }
                    PeerMessage::PairRejected { reason } => anyhow::bail!("pairing rejected: {reason}"),
                    other => tracing::debug!("ignoring {other:?} while pairing"),
                }
            }
            _ => {}
        }
    }
}
