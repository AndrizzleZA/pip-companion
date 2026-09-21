//! Small secrets (calendar subscription links, later OAuth refresh tokens) encrypted with DPAPI:
//! only this Windows user on this machine can decrypt them. Stored in `secrets.dat`, never in the
//! readable settings file.

use std::collections::BTreeMap;
use std::path::Path;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::*;

const ENTROPY: &[u8] = b"Pip companion secrets v1";

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
}

fn protect(plain: &[u8]) -> Option<Vec<u8>> {
    unsafe {
        let input = blob(plain);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        if CryptProtectData(&input, std::ptr::null(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out) == 0 {
            return None;
        }
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData as _);
        Some(v)
    }
}

fn unprotect(cipher: &[u8]) -> Option<Vec<u8>> {
    unsafe {
        let input = blob(cipher);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        if CryptUnprotectData(&input, std::ptr::null_mut(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out) == 0 {
            return None;
        }
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        // Wipe the plaintext copy DPAPI allocated before freeing it.
        std::ptr::write_bytes(out.pbData, 0, out.cbData as usize);
        LocalFree(out.pbData as _);
        Some(v)
    }
}

pub fn load(dir: &Path) -> BTreeMap<String, String> {
    std::fs::read(dir.join("secrets.dat"))
        .ok()
        .and_then(|c| unprotect(&c))
        .and_then(|p| serde_json::from_slice(&p).ok())
        .unwrap_or_default()
}

pub fn save(dir: &Path, map: &BTreeMap<String, String>) -> bool {
    let Ok(plain) = serde_json::to_vec(map) else { return false };
    let Some(cipher) = protect(&plain) else { return false };
    let tmp = dir.join("secrets.dat.tmp");
    std::fs::write(&tmp, cipher).and_then(|_| std::fs::rename(&tmp, dir.join("secrets.dat"))).is_ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn roundtrip_and_not_plaintext() {
        let dir = std::env::temp_dir().join(format!("pip-secrets-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut m = std::collections::BTreeMap::new();
        m.insert("cal:1".to_string(), "https://calendar.google.com/private-SECRET/basic.ics".to_string());
        assert!(super::save(&dir, &m));
        let raw = std::fs::read(dir.join("secrets.dat")).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("SECRET"));
        assert_eq!(super::load(&dir), m);
        std::fs::remove_dir_all(&dir).ok();
    }
}
