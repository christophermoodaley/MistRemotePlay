//! Can other PCs reach this host over the internet? Finds the public address
//! the internet sees (STUN) and tests the built-in server's port step by
//! step for the Host page's "Test connection".

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use serde::Serialize;
use tokio::net::UdpSocket;

use crate::builtin_server::DEFAULT_STUN;

const STUN_TIMEOUT: Duration = Duration::from_secs(3);
const MAGIC_COOKIE: u32 = 0x2112_A442;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Ok,
    Warn,
    Fail,
    Skipped,
}

#[derive(Clone, Debug, Serialize)]
pub struct TestStep {
    pub label: String,
    pub status: StepStatus,
    /// What was found and what to do, for people.
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConnectionTest {
    /// The `ip:port` other PCs should type, when known.
    pub share_address: Option<String>,
    /// This network's public IPv4 address as the internet sees it.
    pub public_ip: Option<String>,
    pub steps: Vec<TestStep>,
}

fn step(label: &str, status: StepStatus, detail: impl Into<String>) -> TestStep {
    TestStep { label: label.into(), status, detail: detail.into() }
}

/// This network's public IPv4 address, asked of the default STUN servers.
pub async fn public_ip() -> Option<Ipv4Addr> {
    for url in DEFAULT_STUN {
        let Some(host) = url.strip_prefix("stun:") else { continue };
        let Ok(addrs) = tokio::net::lookup_host(host).await else { continue };
        for addr in addrs.filter(SocketAddr::is_ipv4) {
            match tokio::time::timeout(STUN_TIMEOUT, stun_binding(addr)).await {
                Ok(Ok(ip)) => return Some(ip),
                Ok(Err(e)) => tracing::debug!(%addr, "STUN: {e}"),
                Err(_) => tracing::debug!(%addr, "STUN: timed out"),
            }
        }
    }
    None
}

/// One STUN Binding request (RFC 5389); returns the mapped IPv4 address.
async fn stun_binding(server: SocketAddr) -> std::io::Result<Ipv4Addr> {
    let socket = UdpSocket::bind(("0.0.0.0", 0)).await?;
    socket.connect(server).await?;
    let mut request = [0u8; 20];
    request[0..2].copy_from_slice(&0x0001u16.to_be_bytes());
    request[4..8].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
    let id: [u8; 12] = rand_id();
    request[8..20].copy_from_slice(&id);
    let mut buf = [0u8; 576];
    for _ in 0..3 {
        socket.send(&request).await?;
        if let Ok(Ok(n)) = tokio::time::timeout(Duration::from_millis(800), socket.recv(&mut buf)).await {
            if let Some(ip) = parse_binding_response(&buf[..n], &id) {
                return Ok(ip);
            }
        }
    }
    Err(std::io::Error::other("no usable answer"))
}

fn rand_id() -> [u8; 12] {
    let mut id = [0u8; 12];
    let _ = getrandom::getrandom(&mut id);
    id
}

fn parse_binding_response(msg: &[u8], id: &[u8; 12]) -> Option<Ipv4Addr> {
    if msg.len() < 20 || msg[0..2] != [0x01, 0x01] || msg[4..8] != MAGIC_COOKIE.to_be_bytes() || &msg[8..20] != id {
        return None;
    }
    let len = u16::from_be_bytes([msg[2], msg[3]]) as usize;
    let attrs = msg.get(20..20 + len)?;
    let (mut i, mut mapped) = (0, None);
    while i + 4 <= attrs.len() {
        let kind = u16::from_be_bytes([attrs[i], attrs[i + 1]]);
        let alen = u16::from_be_bytes([attrs[i + 2], attrs[i + 3]]) as usize;
        let value = attrs.get(i + 4..i + 4 + alen)?;
        // IPv4 family; XOR-MAPPED-ADDRESS wins over MAPPED-ADDRESS.
        if value.len() >= 8 && value[1] == 0x01 {
            let raw = [value[4], value[5], value[6], value[7]];
            match kind {
                0x0020 => {
                    let x = u32::from_be_bytes(raw) ^ MAGIC_COOKIE;
                    return Some(Ipv4Addr::from(x));
                }
                0x0001 => mapped = Some(Ipv4Addr::from(raw)),
                _ => {}
            }
        }
        i += 4 + alen.div_ceil(4) * 4;
    }
    mapped
}

/// Test whether the built-in server on `port` (certificate `fingerprint`)
/// can be reached from the internet. `upnp`: automatic port opening is on.
pub async fn test(port: u16, fingerprint: &str, upnp: bool) -> ConnectionTest {
    let mut steps = Vec::new();
    let public_ip = public_ip().await;
    steps.push(match public_ip {
        Some(ip) => step("Public address", StepStatus::Ok, format!("The internet sees this network as {ip}.")),
        None => step(
            "Public address",
            StepStatus::Fail,
            "Could not reach the internet to find this network's public address.",
        ),
    });

    let router = if upnp {
        let r = crate::upnp::check(port).await;
        steps.push(match &r {
            Ok(addr) => step("Router (UPnP)", StepStatus::Ok, format!("Your router opened port {port} at {addr}.")),
            Err(reason) => step("Router (UPnP)", StepStatus::Fail, reason.clone()),
        });
        r.ok()
    } else {
        steps.push(step(
            "Router (UPnP)",
            StepStatus::Skipped,
            "Automatic port opening is off (Settings → Host). Forward the port by hand.",
        ));
        None
    };
    let router_ip = router.as_deref().and_then(|a| a.parse::<SocketAddr>().ok()).map(|a| a.ip());
    if let (Some(router_ip), Some(public)) = (router_ip, public_ip) {
        if router_ip != IpAddr::V4(public) {
            steps.push(step(
                "Same address",
                StepStatus::Warn,
                format!(
                    "Your router opened the port on {router_ip}, but your internet traffic leaves through {public} \
                     (two internet lines, or another router in front). Other PCs should use {router_ip}."
                ),
            ));
        }
    }

    let share_address = router.clone().or_else(|| public_ip.map(|ip| format!("{ip}:{port}")));
    steps.push(match &share_address {
        None => step("Reachable from outside", StepStatus::Skipped, "No public address to test."),
        Some(addr) => match crate::signaling::server_fingerprint(&format!("wss://{addr}")).await {
            Ok(fp) if fp.eq_ignore_ascii_case(fingerprint) => {
                step("Reachable from outside", StepStatus::Ok, format!("Reached this PC's server at {addr}."))
            }
            Ok(_) => step(
                "Reachable from outside",
                StepStatus::Fail,
                format!("Another device answered at {addr}: the port forward points somewhere else."),
            ),
            Err(_) => step(
                "Reachable from outside",
                StepStatus::Warn,
                format!(
                    "Could not reach {addr} from this PC. Many routers don't loop connections back to their own \
                     network, so this may still work for others: ask a friend to connect to {addr}.{}",
                    if router.is_none() { " If you haven't forwarded the port, do that first." } else { "" }
                ),
            ),
        },
    });
    ConnectionTest { share_address, public_ip: public_ip.map(|ip| ip.to_string()), steps }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xor_mapped_address() {
        let id = [7u8; 12];
        let mut msg = vec![0x01, 0x01, 0x00, 0x0C];
        msg.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        msg.extend_from_slice(&id);
        // XOR-MAPPED-ADDRESS, IPv4, port and 155.93.192.77 XOR the cookie.
        let ip = u32::from(Ipv4Addr::new(155, 93, 192, 77)) ^ MAGIC_COOKIE;
        msg.extend_from_slice(&[0x00, 0x20, 0x00, 0x08, 0x00, 0x01, 0x00, 0x00]);
        msg.extend_from_slice(&ip.to_be_bytes());
        assert_eq!(parse_binding_response(&msg, &id), Some(Ipv4Addr::new(155, 93, 192, 77)));
        assert_eq!(parse_binding_response(&msg, &[0u8; 12]), None);
    }

    /// Needs the internet; read-only.
    #[tokio::test]
    #[ignore]
    async fn finds_public_ip() {
        println!("public IP: {:?}", public_ip().await);
    }
}
