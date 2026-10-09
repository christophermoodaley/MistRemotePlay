//! Owner-only storage for secret material (device private key, access
//! tokens).
//!
//! * Unix: file mode 0600 in a 0700 directory.
//! * Windows: contents encrypted with DPAPI (CryptProtectData, bound to the
//!   current user) in the user's profile directory.

use std::io;
use std::path::Path;

#[cfg(unix)]
pub fn write(path: &Path, data: &[u8]) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    std::fs::rename(tmp, path)
}

#[cfg(unix)]
pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(path)?;
    if meta.permissions().mode() & 0o077 != 0 {
        tracing::warn!("{} is readable by other users; tightening permissions to 0600", path.display());
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::read(path)
}

#[cfg(windows)]
mod dpapi {
    use std::io;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
    }

    fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        // SAFETY: the API allocated `cbData` bytes at `pbData` with LocalAlloc.
        let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        unsafe {
            let _ = LocalFree(Some(HLOCAL(out.pbData as *mut _)));
        }
        v
    }

    pub fn protect(data: &[u8]) -> io::Result<Vec<u8>> {
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: input blob points to live data; output is written by the API.
        unsafe { CryptProtectData(&blob(data), None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out) }
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(take(out))
    }

    pub fn unprotect(data: &[u8]) -> io::Result<Vec<u8>> {
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: as above.
        unsafe { CryptUnprotectData(&blob(data), None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out) }
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(take(out))
    }
}

#[cfg(windows)]
pub fn write(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, dpapi::protect(data)?)?;
    std::fs::rename(tmp, path)
}

#[cfg(windows)]
pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    dpapi::unprotect(&std::fs::read(path)?)
}

#[cfg(test)]
mod tests {
    #[test]
    fn roundtrip_and_permissions() {
        let dir = std::env::temp_dir().join(format!("mist-secure-{}", std::process::id()));
        let p = dir.join("s.bin");
        super::write(&p, b"secret").unwrap();
        assert_eq!(super::read(&p).unwrap(), b"secret");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
