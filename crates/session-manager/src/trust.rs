//! Paired-device store. Hosts remember authorized clients; clients remember
//! hosts they paired with. Only public keys are stored here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mist_protocol::identity::{DeviceId, PublicKeyB64};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedDevice {
    pub device_id: DeviceId,
    pub name: String,
    pub public_key: PublicKeyB64,
    pub paired_at_ms: u64,
    pub last_connected_ms: Option<u64>,
    /// Signaling server through which the device was paired (clients use it
    /// to reach the host again).
    #[serde(default)]
    pub signaling_url: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct TrustStore {
    devices: BTreeMap<DeviceId, PairedDevice>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum TrustError {
    #[error("trust store I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("trust store is corrupt: {0}")]
    Corrupt(String),
    #[error("device {0} presents a different key than when it was paired")]
    KeyMismatch(DeviceId),
}

impl TrustStore {
    pub fn load(path: &Path) -> Result<Self, TrustError> {
        let mut store = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice::<TrustStore>(&bytes).map_err(|e| TrustError::Corrupt(e.to_string()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => TrustStore::default(),
            Err(e) => return Err(e.into()),
        };
        // Re-validate on load: a tampered entry must not be trusted.
        store.devices.retain(|id, d| d.public_key.device_id().map(|derived| &derived == id).unwrap_or(false));
        store.path = Some(path.to_owned());
        Ok(store)
    }

    pub fn in_memory() -> Self {
        TrustStore::default()
    }

    pub fn save(&self) -> Result<(), TrustError> {
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).expect("serializable"))?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Trusted device whose key matches; `None` if unknown.
    pub fn verify(&self, id: &DeviceId, key: &PublicKeyB64) -> Result<Option<&PairedDevice>, TrustError> {
        match self.devices.get(id) {
            None => Ok(None),
            Some(d) if &d.public_key == key => Ok(Some(d)),
            Some(_) => Err(TrustError::KeyMismatch(id.clone())),
        }
    }

    pub fn get(&self, id: &DeviceId) -> Option<&PairedDevice> {
        self.devices.get(id)
    }

    pub fn add(&mut self, device: PairedDevice) -> Result<(), TrustError> {
        self.devices.insert(device.device_id.clone(), device);
        self.save()
    }

    pub fn remove(&mut self, id: &DeviceId) -> Result<bool, TrustError> {
        let removed = self.devices.remove(id).is_some();
        self.save()?;
        Ok(removed)
    }

    pub fn touch(&mut self, id: &DeviceId, now_ms: u64) -> Result<(), TrustError> {
        if let Some(d) = self.devices.get_mut(id) {
            d.last_connected_ms = Some(now_ms);
            self.save()?;
        }
        Ok(())
    }

    pub fn list(&self) -> Vec<PairedDevice> {
        let mut v: Vec<_> = self.devices.values().cloned().collect();
        v.sort_by_key(|d| std::cmp::Reverse(d.last_connected_ms.unwrap_or(d.paired_at_ms)));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mist_protocol::identity::DeviceIdentity;

    fn device(id: &DeviceIdentity) -> PairedDevice {
        PairedDevice {
            device_id: id.device_id(),
            name: "pc".into(),
            public_key: id.public_key_b64(),
            paired_at_ms: 1,
            last_connected_ms: None,
            signaling_url: None,
        }
    }

    #[test]
    fn persist_and_verify() {
        let dir = std::env::temp_dir().join(format!("mist-trust-{}-{}", std::process::id(), line!()));
        let path = dir.join("trust.json");
        let a = DeviceIdentity::generate();
        let b = DeviceIdentity::generate();
        {
            let mut s = TrustStore::load(&path).unwrap();
            s.add(device(&a)).unwrap();
        }
        let s = TrustStore::load(&path).unwrap();
        assert!(s.verify(&a.device_id(), &a.public_key_b64()).unwrap().is_some());
        assert!(s.verify(&b.device_id(), &b.public_key_b64()).unwrap().is_none());
        assert!(matches!(s.verify(&a.device_id(), &b.public_key_b64()), Err(TrustError::KeyMismatch(_))));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tampered_entries_dropped_on_load() {
        let dir = std::env::temp_dir().join(format!("mist-trust-{}-{}", std::process::id(), line!()));
        let path = dir.join("trust.json");
        let a = DeviceIdentity::generate();
        let b = DeviceIdentity::generate();
        let mut s = TrustStore::load(&path).unwrap();
        let mut d = device(&a);
        d.public_key = b.public_key_b64(); // key does not match the id
        s.add(d).unwrap();
        let s = TrustStore::load(&path).unwrap();
        assert!(s.list().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn remove_revokes() {
        let a = DeviceIdentity::generate();
        let mut s = TrustStore::in_memory();
        s.add(device(&a)).unwrap();
        assert!(s.remove(&a.device_id()).unwrap());
        assert!(s.verify(&a.device_id(), &a.public_key_b64()).unwrap().is_none());
    }
}
