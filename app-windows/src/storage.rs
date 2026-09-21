//! Local persistence: two small JSON files, written atomically and only when something changed.

use pip_core::{Settings, State};
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct Storage {
    pub dir: PathBuf,
}

impl Storage {
    /// `%APPDATA%\Pip`, or `PIP_DATA_DIR` (used by tests so they never touch real data).
    pub fn open() -> Storage {
        let dir = std::env::var_os("PIP_DATA_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Pip")))
            .unwrap_or_else(|| PathBuf::from("."));
        let _ = std::fs::create_dir_all(&dir);
        Storage { dir }
    }

    pub fn is_custom_dir() -> bool {
        std::env::var_os("PIP_DATA_DIR").is_some()
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    pub fn load_settings(&self) -> (Settings, bool) {
        let (mut s, existed): (Settings, bool) = self.load("settings.json");
        s.sanitize();
        (s, existed)
    }

    pub fn load_state(&self, now: i64) -> State {
        let (st, existed): (State, bool) = self.load("state.json");
        if existed { st } else { State::new(now) }
    }

    fn load<T: serde::de::DeserializeOwned + Default>(&self, name: &str) -> (T, bool) {
        let p = self.path(name);
        match std::fs::read(&p) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(v) => (v, true),
                Err(e) => {
                    // Keep the broken file for inspection; never crash on bad data.
                    let _ = std::fs::rename(&p, p.with_extension("json.corrupt"));
                    self.log(&format!("{name} unreadable ({e}); using defaults"));
                    (T::default(), false)
                }
            },
            Err(_) => (T::default(), false),
        }
    }

    pub fn save_settings(&self, s: &Settings) {
        self.save("settings.json", s);
    }

    pub fn save_state(&self, st: &State) {
        self.save("state.json", st);
    }

    fn save<T: serde::Serialize>(&self, name: &str, v: &T) {
        let Ok(json) = serde_json::to_vec_pretty(v) else { return };
        let target = self.path(name);
        let tmp = target.with_extension("json.tmp");
        let res = (|| -> std::io::Result<()> {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&json)?;
            f.sync_all()?;
            drop(f);
            std::fs::rename(&tmp, &target) // atomic replace on NTFS
        })();
        if let Err(e) = res {
            self.log(&format!("saving {name} failed: {e}"));
        }
    }

    /// Tiny append-only log (startup timing, errors). Rotated at 256 KB.
    pub fn log(&self, msg: &str) {
        let p = self.path("pip.log");
        if std::fs::metadata(&p).map(|m| m.len() > 256 * 1024).unwrap_or(false) {
            let _ = std::fs::rename(&p, p.with_extension("log.old"));
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
            let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            let _ = writeln!(f, "{ts}  {msg}");
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}
