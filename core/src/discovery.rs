//! Optional LAN discovery over mDNS/DNS-SD. Hosts advertise their device id,
//! name and signaling URL; nothing security-relevant depends on it (all
//! sessions are still authenticated with paired keys).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::Serialize;

const SERVICE: &str = "_mist-play._udp.local.";

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct DiscoveredHost {
    pub device_id: String,
    pub name: String,
    pub signaling_url: String,
    pub addresses: Vec<String>,
    /// Certificate fingerprint of the host's built-in server, when it runs
    /// one (the client pins it before pairing).
    pub fingerprint: Option<String>,
}

pub struct Advertiser {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Advertiser {
    /// `server`: the host's built-in signaling server, if it runs one;
    /// otherwise clients are told to use `signaling_url`.
    pub fn start(
        device_id: &str,
        name: &str,
        signaling_url: &str,
        server: Option<&crate::builtin_server::ServerInfo>,
    ) -> anyhow::Result<Self> {
        let daemon = ServiceDaemon::new()?;
        let host = format!("mist-{}.local.", &device_id[..12]);
        let mut props = vec![("id", device_id.to_owned()), ("name", name.to_owned()), ("v", "1".to_owned())];
        let port = match server {
            Some(s) => {
                props.push(("fp", s.fingerprint.clone()));
                s.port
            }
            None => {
                props.push(("sig", signaling_url.to_owned()));
                9 // informational only: without a built-in server nothing listens
            }
        };
        let info = ServiceInfo::new(SERVICE, &format!("mist-{}", &device_id[..12]), &host, "", port, &props[..])?
            .enable_addr_auto();
        let fullname = info.get_fullname().to_string();
        daemon.register(info)?;
        tracing::info!("advertising host on the LAN via mDNS");
        Ok(Advertiser { daemon, fullname })
    }
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

/// Browse for hosts for `wait`.
pub fn browse(wait: Duration) -> anyhow::Result<Vec<DiscoveredHost>> {
    let daemon = ServiceDaemon::new()?;
    let rx = daemon.browse(SERVICE)?;
    let deadline = Instant::now() + wait;
    let mut found: HashMap<String, DiscoveredHost> = HashMap::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                let get = |k: &str| info.get_property_val_str(k).map(str::to_owned);
                let (Some(id), Some(name)) = (get("id"), get("name")) else { continue };
                if crate::protocol::identity::DeviceId::parse(&id).is_err() {
                    continue;
                }
                let mut v4: Vec<std::net::Ipv4Addr> = info
                    .get_addresses()
                    .iter()
                    .filter_map(|a| match a {
                        std::net::IpAddr::V4(v) if !v.is_loopback() && !v.is_link_local() => Some(*v),
                        _ => None,
                    })
                    .collect();
                v4.sort_by_key(|ip| (!ip.is_private(), *ip));
                let fingerprint = get("fp").filter(|f| f.len() == 64 && f.chars().all(|c| c.is_ascii_hexdigit()));
                let signaling_url = match (&fingerprint, get("sig"), v4.first()) {
                    // Built-in server: reach it at the host's LAN address.
                    (Some(_), _, Some(ip)) => format!("wss://{ip}:{}", info.get_port()),
                    (None, Some(sig), _) => sig,
                    _ => continue,
                };
                let addresses = info.get_addresses().iter().map(|a| a.to_string()).collect();
                found.insert(id.clone(), DiscoveredHost { device_id: id, name, signaling_url, addresses, fingerprint });
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = daemon.shutdown();
    Ok(found.into_values().collect())
}
