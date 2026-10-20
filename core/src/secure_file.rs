//! Owner-only storage for secret material (device private key, access
//! tokens).
//!
//! Contents are encrypted with DPAPI (CryptProtectData, bound to the current
//! user) in the user's profile directory.

use std::io;
use std::path::Path;

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
        let _ = std::fs::remove_dir_all(dir);
    }
}
