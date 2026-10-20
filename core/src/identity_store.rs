//! Loads or creates this installation's device identity.

use std::path::Path;
use std::sync::Arc;

use crate::protocol::identity::DeviceIdentity;

pub fn load_or_create(dir: &Path) -> anyhow::Result<Arc<DeviceIdentity>> {
    let path = dir.join("identity.key");
    match crate::secure_file::read(&path) {
        Ok(bytes) => {
            let id = DeviceIdentity::from_secret_bytes(&bytes)
                .map_err(|e| anyhow::anyhow!("identity key is corrupt: {e}"))?;
            Ok(Arc::new(id))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let id = DeviceIdentity::generate();
            crate::secure_file::write(&path, &id.secret_bytes())?;
            tracing::info!(device = %id.device_id(), "created new device identity");
            Ok(Arc::new(id))
        }
        Err(e) => Err(anyhow::anyhow!("cannot read identity key {}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn stable_across_loads() {
        let dir = std::env::temp_dir().join(format!("mist-id-{}", std::process::id()));
        let a = super::load_or_create(&dir).unwrap();
        let b = super::load_or_create(&dir).unwrap();
        assert_eq!(a.device_id(), b.device_id());
        let _ = std::fs::remove_dir_all(dir);
    }
}
