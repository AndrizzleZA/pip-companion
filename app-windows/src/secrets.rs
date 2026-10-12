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

/// Overwrites a buffer before it is dropped.
///
/// `write_volatile` through the pointer, because a plain loop writing bytes that are never read
/// again is exactly the work an optimiser is entitled to delete. This is the same courtesy the
/// buffer DPAPI allocates already gets; the copies made from it were going to the allocator
/// unwiped, leaving refresh tokens and the sync key in freed heap for as long as it stayed unused
/// (§37). It does not reach copies serde has already made inside the strings it parsed — those
/// live as long as the map does, which is the life of the process, and chasing them would mean a
/// secret-string type rather than a `BTreeMap<String, String>`.
fn wipe(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        unsafe { std::ptr::write_volatile(b, 0) };
    }
}

pub fn load(dir: &Path) -> BTreeMap<String, String> {
    let Ok(cipher) = std::fs::read(dir.join("secrets.dat")) else { return BTreeMap::new() };
    let Some(mut plain) = unprotect(&cipher) else { return BTreeMap::new() };
    let map = serde_json::from_slice(&plain).unwrap_or_default();
    wipe(&mut plain);
    map
}

pub fn save(dir: &Path, map: &BTreeMap<String, String>) -> bool {
    let Ok(mut plain) = serde_json::to_vec(map) else { return false };
    let cipher = protect(&plain);
    wipe(&mut plain);
    let Some(cipher) = cipher else { return false };
    let tmp = dir.join("secrets.dat.tmp");
    std::fs::write(&tmp, cipher).and_then(|_| std::fs::rename(&tmp, dir.join("secrets.dat"))).is_ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_wiped_buffer_keeps_nothing() {
        let mut buf = b"refresh-token-abc123".to_vec();
        super::wipe(&mut buf);
        assert!(buf.iter().all(|b| *b == 0), "the plaintext copy must not go to the allocator readable");
    }

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

#[cfg(test)]
mod the_claim_section_four_makes {
    /// §4 claimed for weeks that secrets lived in the Windows Credential Manager, which this
    /// project has never used — finding 5 of §37, fixed in the same commit that named it.
    ///
    /// The corrected claim is that they are DPAPI-encrypted in `secrets.dat`, and it is pinned
    /// here so that moving to a different store has to notice the sentence it makes false (§51).
    #[test]
    fn secrets_are_dpapi_and_not_the_credential_manager() {
        // Only the module above the tests: this file's own assertions name the API they forbid,
        // and the first version of this test failed on itself.
        let source = include_str!("secrets.rs").split("#[cfg(test)]").next().unwrap_or_default();
        assert!(source.contains("CryptProtectData"), "§4 says DPAPI");
        assert!(source.contains("CryptUnprotectData"), "§4 says DPAPI");
        assert!(
            !source.contains("CredWriteW") && !source.contains("CredReadW"),
            "§4 says explicitly that the Credential Manager is not used; it is being used"
        );
    }
}
