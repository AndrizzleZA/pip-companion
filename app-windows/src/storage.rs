//! Local persistence: two small JSON files, written atomically and only when something changed.

use pip_core::{Settings, State};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub struct Storage {
    pub dir: PathBuf,
    /// Set when a file existed but would not open. While it is set, nothing is written.
    blocked: AtomicBool,
}

/// Takes the data folder off every account but this one, once.
///
/// `state.json` keeps sender addresses and subject lines and `settings.json` keeps reminder titles,
/// both in plain text, and a new folder under `%APPDATA%` inherits whatever that tree allows. On
/// the machine this was written on that inheritance handed read access to two local sandbox
/// accounts (§37). Pip cannot assume that is unusual, so it stops relying on what it inherits:
/// inheritance off, and full control to this user, SYSTEM and the administrators — by well-known
/// SID, since those two are named differently in a localised Windows.
///
/// `icacls` rather than building a DACL by hand: this runs once in the life of an installation, and
/// forty lines of security-descriptor plumbing to save one process would be the worse trade — the
/// same reasoning that writes the Start-menu shortcut through the shell (§16). The marker file is
/// what keeps it to once; `Storage::open` is called often enough that a stat is the right cost, and
/// a failed attempt deliberately leaves no marker so the next launch tries again.
fn lock_to_this_user(dir: &Path) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let marker = dir.join(".folder-locked");
    if marker.exists() {
        return;
    }
    let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
        (Ok(d), Ok(u)) if !d.is_empty() && !u.is_empty() => format!("{d}\\{u}"),
        (_, Ok(u)) if !u.is_empty() => u,
        _ => return,
    };
    let icacls = |args: Vec<String>| {
        std::process::Command::new("icacls")
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    let d = dir.display().to_string();

    // The folder only. Emphatically not `/T`: that strips the inherited entries from every file
    // inside as well, and a file whose permissions were *only* inherited is then left reachable by
    // nobody — including Pip, which on the next launch reads nothing, finds nothing, and saves a
    // brand-new empty state over the real one. That is not a thought experiment; it is what the
    // first version of this function did to a real install.
    let folder = icacls(vec![
        d.clone(),
        "/inheritance:r".into(),
        "/grant:r".into(),
        format!("{user}:(OI)(CI)F"),
        "/grant:r".into(),
        "*S-1-5-18:(OI)(CI)F".into(), // SYSTEM
        "/grant:r".into(),
        "*S-1-5-32-544:(OI)(CI)F".into(), // Administrators
        "/Q".into(),
    ]);

    // Now the files already in there: reset each one to inherit, which is the folder's new and
    // narrower list. The wildcard matters — `icacls <dir> /reset` would reset the folder too and
    // undo the line above.
    let children = icacls(vec![format!("{d}\\*"), "/reset".into(), "/T".into(), "/C".into(), "/Q".into()]);

    if folder && children {
        let _ = std::fs::write(&marker, "This folder is readable only by you, SYSTEM and administrators.\n");
    }
}

impl Storage {
    /// `%APPDATA%\Pip`, or `PIP_DATA_DIR` (used by tests so they never touch real data).
    pub fn open() -> Storage {
        let dir = std::env::var_os("PIP_DATA_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Pip")))
            .unwrap_or_else(|| PathBuf::from("."));
        let _ = std::fs::create_dir_all(&dir);
        lock_to_this_user(&dir);
        Storage { dir, blocked: AtomicBool::new(false) }
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
        let (mut st, existed): (State, bool) = self.load("state.json");
        if !existed {
            return State::new(now);
        }
        // Loading does not mark the state dirty, so an upgraded file would otherwise sit
        // unwritten until something else happened to save. Write it back here, once.
        let upgraded = st.version < pip_core::state::VERSION;
        st.migrate();
        if upgraded {
            self.save_state(&st);
        }
        st
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
            // The file isn't there: a first run, or a setting never saved. Defaults are right.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (T::default(), false),
            // The file is there and will not open — no permission, a lock, a failing disk. This is
            // emphatically not the same as having no data, and treating it as such is how §38
            // destroyed a real install: defaults in memory, then saved straight over the file
            // nobody could read. Refuse to write anything from here on instead.
            Err(e) => {
                self.blocked.store(true, Ordering::Relaxed);
                self.log(&format!("{name} is there but could not be read ({e}); nothing will be saved over it"));
                (T::default(), false)
            }
        }
    }

    /// True when a file existed and could not be read, so saving is off. What is in memory are
    /// defaults, and writing them would destroy whatever could not be opened.
    pub fn blocked(&self) -> bool {
        self.blocked.load(Ordering::Relaxed)
    }

    pub fn save_settings(&self, s: &Settings) {
        self.save("settings.json", s);
    }

    pub fn save_state(&self, st: &State) {
        self.save("state.json", st);
    }

    fn save<T: serde::Serialize>(&self, name: &str, v: &T) {
        // Something here could not be read, so what is in memory may be defaults standing in for
        // real data. Writing them would be the destructive act, not the cautious one.
        if self.blocked() {
            return;
        }
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

#[cfg(test)]
mod unreadable_is_not_absent {
    use super::*;

    fn store(dir: &Path) -> Storage {
        Storage { dir: dir.to_path_buf(), blocked: AtomicBool::new(false) }
    }

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pip-unreadable-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A file that is simply not there is an ordinary first run: defaults, and saving is fine.
    #[test]
    fn a_missing_file_is_still_just_a_fresh_start() {
        let d = scratch("missing");
        let s = store(&d);
        let (_, existed): (State, bool) = s.load("state.json");
        assert!(!existed);
        assert!(!s.blocked(), "nothing was unreadable, so saving must stay on");
        s.save_state(&State::new(0));
        assert!(d.join("state.json").is_file(), "and a first run does write its state");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A file that exists and will not open must not be mistaken for one that isn't there.
    /// A directory standing where the file belongs is the portable way to force that error.
    #[test]
    fn a_file_that_will_not_open_stops_every_save() {
        let d = scratch("blocked");
        std::fs::create_dir_all(d.join("state.json")).unwrap();
        let s = store(&d);
        let (_, existed): (State, bool) = s.load("state.json");
        assert!(!existed, "nothing could be loaded");
        assert!(s.blocked(), "but it must know the difference between that and an empty folder");

        s.save_state(&State::new(0));
        s.save_settings(&Settings::default());
        assert!(d.join("state.json").is_dir(), "the unreadable thing is left exactly as it was");
        assert!(!d.join("settings.json").exists(), "and nothing else is written either");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Unparseable is a different case and keeps its old behaviour: quarantine, then start fresh.
    #[test]
    fn nonsense_in_a_file_is_still_quarantined_rather_than_locking_saves() {
        let d = scratch("corrupt");
        std::fs::write(d.join("state.json"), b"this is not json").unwrap();
        let s = store(&d);
        let (_, existed): (State, bool) = s.load("state.json");
        assert!(!existed);
        assert!(!s.blocked(), "a readable file we cannot parse is recoverable, so saving stays on");
        assert!(d.join("state.json.corrupt").is_file(), "the original is kept for inspection");
        let _ = std::fs::remove_dir_all(&d);
    }
}
