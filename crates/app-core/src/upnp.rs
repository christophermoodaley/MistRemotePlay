//! Opens the built-in server's port on the router with UPnP (IGD), so other
//! PCs can reach this host over the internet without a manual port forward.
//! Only the signaling port needs it: media finds its own way through NAT
//! (STUN). The mapping has a lease that is renewed while hosting and is
//! removed when hosting stops; if the app is killed, the router drops it
//! when the lease runs out.

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::Duration;

use igd_next::aio::tokio::{search_gateway, Tokio};
use igd_next::aio::Gateway;
use igd_next::{AddPortError, PortMappingProtocol, SearchOptions};
use serde::Serialize;

const DESCRIPTION: &str = "Mist Remote Play";
const LEASE_SECS: u32 = 3600;
const RENEW_EVERY: Duration = Duration::from_secs(20 * 60);
/// After a failure, try again now and then: the network or the router's
/// settings may change while hosting.
const RETRY_EVERY: Duration = Duration::from_secs(2 * 60);
const SEARCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Whether the built-in server's port is open on the router.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PortMapping {
    /// Automatic port opening is switched off.
    #[default]
    Off,
    /// Asking the router.
    Pending,
    /// Open: other PCs reach this host at `external_address` (`ip:port`).
    Open { external_address: String },
    /// Not open; `reason` says why and what to do, for people.
    Failed { reason: String },
}

/// Keeps the port open while it lives; dropping it closes the port.
pub struct PortMapper {
    task: tokio::task::JoinHandle<()>,
}

impl Drop for PortMapper {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Start opening `port` (TCP) on the router. `on_change` is called with
/// every change of state. Must be called within a Tokio runtime.
pub fn start(port: u16, on_change: impl Fn(PortMapping) + Send + 'static) -> PortMapper {
    PortMapper { task: tokio::spawn(run(port, on_change)) }
}

/// Removes the mapping when dropped, which includes the task being aborted.
struct Mapped {
    gateway: Gateway<Tokio>,
    port: u16,
}

impl Drop for Mapped {
    fn drop(&mut self) {
        let (gateway, port) = (self.gateway.clone(), self.port);
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                match gateway.remove_port(PortMappingProtocol::TCP, port).await {
                    Ok(()) => tracing::info!(port, "UPnP: closed the port on the router"),
                    Err(e) => tracing::debug!(port, "UPnP: could not close the port: {e}"),
                }
            });
        }
    }
}

async fn run(port: u16, on_change: impl Fn(PortMapping)) {
    on_change(PortMapping::Pending);
    let mut last = PortMapping::Pending;
    let mut mapped: Option<Mapped> = None;
    loop {
        let (state, wait) = match open(port).await {
            Ok((gateway, external_address)) => {
                // Keep the guard on renewal: replacing it would remove the
                // mapping just renewed.
                mapped.get_or_insert(Mapped { gateway, port });
                (PortMapping::Open { external_address }, RENEW_EVERY)
            }
            Err(reason) => {
                mapped = None;
                (PortMapping::Failed { reason }, RETRY_EVERY)
            }
        };
        if state != last {
            match &state {
                PortMapping::Open { external_address } => {
                    tracing::info!(port, %external_address, "UPnP: opened the port on the router")
                }
                PortMapping::Failed { reason } => tracing::warn!(port, "UPnP: {reason}"),
                _ => {}
            }
            on_change(state.clone());
            last = state;
        }
        tokio::time::sleep(wait).await;
    }
}

/// Map `port` to this PC and return the gateway and the public `ip:port`.
/// Adding a mapping that already exists for this PC renews it.
async fn open(port: u16) -> Result<(Gateway<Tokio>, String), String> {
    let mut opts = SearchOptions::default();
    opts.timeout = Some(SEARCH_TIMEOUT);
    let gateway = search_gateway(opts).await.map_err(|e| {
        tracing::debug!("UPnP search: {e}");
        "No router answered UPnP requests. Turn on UPnP in your router's settings, or forward the port by hand."
            .to_string()
    })?;
    let local_ip = local_ip_towards(gateway.addr)
        .ok_or_else(|| "Could not tell which address this PC has on the router's network.".to_string())?;
    let local = SocketAddr::new(local_ip, port);
    let mut added = gateway.add_port(PortMappingProtocol::TCP, port, local, LEASE_SECS, DESCRIPTION).await;
    if matches!(added, Err(AddPortError::OnlyPermanentLeasesSupported)) {
        added = gateway.add_port(PortMappingProtocol::TCP, port, local, 0, DESCRIPTION).await;
    }
    added.map_err(|e| match e {
        AddPortError::PortInUse => format!(
            "Your router already forwards port {port} somewhere. If that forward points to this PC, other PCs can \
             already connect; otherwise pick another port under Settings → Host."
        ),
        AddPortError::ActionNotAuthorized => {
            "Your router refused to open the port. Allow UPnP changes in its settings, or forward the port by hand."
                .to_string()
        }
        e => format!("Your router could not open the port ({e})."),
    })?;
    let external = match gateway.get_external_ip().await {
        Ok(ip) => ip,
        Err(e) => {
            let _ = gateway.remove_port(PortMappingProtocol::TCP, port).await;
            return Err(format!("Your router opened the port but did not say its internet address ({e})."));
        }
    };
    if !is_public(external) {
        // Useless behind another NAT; don't leave it open.
        let _ = gateway.remove_port(PortMappingProtocol::TCP, port).await;
        return Err(format!(
            "Your router's internet address ({external}) is not a public one: there is another router in front of \
             it, or your provider shares one address between customers (CGNAT). Forward the port on the outer \
             router too, or use a relay server."
        ));
    }
    Ok((gateway, SocketAddr::new(external, port).to_string()))
}

/// The local address the OS would use to reach `gateway` (no packets sent).
fn local_ip_towards(gateway: SocketAddr) -> Option<IpAddr> {
    let socket = UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    socket.connect(gateway).ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_unspecified()).then_some(ip)
}

/// Reachable from the internet: not private, loopback, link-local or
/// carrier-grade NAT (100.64.0.0/10).
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            let cgnat = a == 100 && (64..128).contains(&b);
            !(v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_unspecified() || cgnat)
        }
        IpAddr::V6(v6) => !(v6.is_loopback() || v6.is_unspecified()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_addresses() {
        for ip in ["155.93.192.77", "8.8.8.8", "100.128.0.1"] {
            assert!(is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["192.168.1.1", "10.0.0.1", "172.16.5.4", "100.64.0.1", "100.127.255.255", "169.254.1.1", "0.0.0.0"] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn serializes_for_the_ui() {
        let open = PortMapping::Open { external_address: "1.2.3.4:47900".into() };
        assert_eq!(
            serde_json::to_value(open).unwrap(),
            serde_json::json!({ "state": "open", "external_address": "1.2.3.4:47900" })
        );
    }
}
