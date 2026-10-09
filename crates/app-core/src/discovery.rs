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
}

pub struct Advertiser {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Advertiser {
    pub fn start(device_id: &str, name: &str, signaling_url: &str) -> anyhow::Result<Self> {
        let daemon = ServiceDaemon::new()?;
        let host = format!("mist-{}.local.", &device_id[..12]);
        let props = [("id", device_id), ("name", name), ("sig", signaling_url), ("v", "1")];
        // The port is informational only; Mist does not listen on the LAN.
        let info = ServiceInfo::new(SERVICE, &format!("mist-{}", &device_id[..12]), &host, "", 9, &props[..])?
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
                let (Some(id), Some(name), Some(sig)) = (get("id"), get("name"), get("sig")) else { continue };
                if mist_protocol::identity::DeviceId::parse(&id).is_err() {
                    continue;
                }
                let addresses = info.get_addresses().iter().map(|a| a.to_string()).collect();
                found.insert(id.clone(), DiscoveredHost { device_id: id, name, signaling_url: sig, addresses });
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = daemon.shutdown();
    Ok(found.into_values().collect())
}
