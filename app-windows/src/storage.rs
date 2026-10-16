//! Local persistence: two small JSON files, written atomically and only when something changed.

use pip_core::{Settings, State};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

pub struct Storage {
    pub dir: PathBuf,
    /// Set when a file existed but would not open. While it is set, nothing is written.
    blocked: AtomicBool,
    /// Log lines that could not be written yet, in order, each already carrying its timestamp.
    pending_log: Mutex<Vec<String>>,
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
        Storage { dir, blocked: AtomicBool::new(false), pending_log: Mutex::new(Vec::new()) }
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

    /// Reads a file, retrying for a third of a second if it is there but will not open.
    ///
    /// Pip starts when you log on, in the middle of everything else the machine is doing, and a
    /// file held for a few milliseconds by a scanner or by the profile service is not the same
    /// thing as a file Pip may never write over again. The first version did not tell those apart:
    /// one transient failure at logon turned saving off for that whole session, and because the
    /// only warning was a bubble shown at 06:36, it went unnoticed for a week (§54).
    ///
    /// Every error except "not there" is retried, including permission errors. Being precise about
    /// which codes are transient would mean guessing which one logon actually produces, and the
    /// cost of guessing wrong is a day of someone's data; the cost of waiting 300 ms for a
    /// genuinely permanent error is 300 ms, once, on a path that already ends in a warning.
    fn read_retrying(&self, p: &Path) -> (std::io::Result<Vec<u8>>, u32) {
        const TRIES: u32 = 6;
        let mut attempt = 1;
        loop {
            match std::fs::read(p) {
                Ok(bytes) => return (Ok(bytes), attempt),
                // Not being there is an answer, not a failure. Never spend time retrying it: it is
                // the ordinary first run, and it is on the startup path of every launch.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Err(e), attempt),
                Err(e) if attempt >= TRIES => return (Err(e), attempt),
                Err(_) => {
                    std::thread::sleep(std::time::Duration::from_millis(60));
                    attempt += 1;
                }
            }
        }
    }

    fn load<T: serde::de::DeserializeOwned + Default>(&self, name: &str) -> (T, bool) {
        let p = self.path(name);
        let (read, attempts) = self.read_retrying(&p);
        if attempts > 1 && read.is_ok() {
            // Worth a line: it is the difference between "this machine is fine" and "this nearly
            // turned saving off", and it is the only way to see the problem receding.
            self.log(&format!("{name} would not open at first; read it on attempt {attempts}"));
        }
        match read {
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
                self.log(&format!(
                    "{name} is there but could not be read after {attempts} attempts ({e}); nothing will be saved over it"
                ));
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
    ///
    /// A line that cannot be written is kept and retried on the next call rather than dropped. The
    /// log is the only record of what went wrong, and the moments it matters most — logon, a locked
    /// file, a full disk — are exactly the moments the write itself fails. §54 stayed invisible for
    /// a week because the one message that explained it was quietly thrown away.
    pub fn log(&self, msg: &str) {
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let mut pending = match self.pending_log.lock() {
            Ok(p) => p,
            // A poisoned lock means another thread panicked while logging. The lines are still
            // intact and losing them is the one thing this function exists to prevent.
            Err(e) => e.into_inner(),
        };
        pending.push(format!("{ts}  {msg}\n"));
        // If writing stays broken, keep the newest. Whatever has been failing since is worth more
        // than the startup line, and an unbounded buffer would be a leak dressed as diligence.
        const KEEP: usize = 200;
        if pending.len() > KEEP {
            let over = pending.len() - KEEP;
            pending.drain(..over);
        }

        let p = self.path("pip.log");
        if std::fs::metadata(&p).map(|m| m.len() > 256 * 1024).unwrap_or(false) {
            let _ = std::fs::rename(&p, p.with_extension("log.old"));
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
            // One write for the backlog: a partial flush would reorder or duplicate lines, and
            // the backlog is small enough that all-or-nothing is the simpler guarantee.
            if f.write_all(pending.concat().as_bytes()).is_ok() {
                pending.clear();
            }
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
        Storage { dir: dir.to_path_buf(), blocked: AtomicBool::new(false), pending_log: Mutex::new(Vec::new()) }
    }

    /// Holds a real Windows lock on a file for a while, the way a scanner does at logon:
    /// `share_mode(0)` is what makes another process's open fail rather than succeed.
    fn hold_exclusively(path: PathBuf, ms: u64) -> std::thread::JoinHandle<()> {
        use std::os::windows::fs::OpenOptionsExt;
        let (tx, rx) = std::sync::mpsc::channel();
        let h = std::thread::spawn(move || {
            // Keep trying until it really is held. The first version signalled whether or not the
            // open had succeeded, so when something else had the file for a moment the test went
            // on believing it was locked and failed for the wrong reason — rarely, and only under
            // a full parallel run, which is the worst kind of test to own.
            let mut held = None;
            for _ in 0..50 {
                match std::fs::OpenOptions::new().read(true).write(true).share_mode(0).open(&path) {
                    Ok(f) => {
                        held = Some(f);
                        break;
                    }
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
                }
            }
            let f = held.expect("could not take an exclusive lock to test against");
            tx.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(ms));
            drop(f);
        });
        rx.recv().unwrap(); // the lock is genuinely held before the test goes on
        h
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

    /// The §54 bug itself: a file locked for a moment at logon must not turn saving off for the
    /// session. Held for 150 ms against a budget of six tries 60 ms apart.
    #[test]
    fn a_file_locked_for_a_moment_is_waited_for_rather_than_given_up_on() {
        let d = scratch("locked-briefly");
        let p = d.join("state.json");
        std::fs::write(&p, serde_json::to_vec(&State::new(0)).unwrap()).unwrap();
        let lock = hold_exclusively(p.clone(), 150);

        let s = store(&d);
        let (_, existed): (State, bool) = s.load("state.json");
        lock.join().unwrap();

        assert!(existed, "the file was there the whole time and had to be waited for, not skipped");
        assert!(!s.blocked(), "a lock that lasts 150 ms must never cost a session of saving");
        s.save_state(&State::new(0));
        assert!(p.is_file(), "and saving still works afterwards");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The other half of §54: the message explaining the failure must survive not being writable.
    #[test]
    fn a_log_line_that_cannot_be_written_is_kept_and_written_later() {
        let d = scratch("log-locked");
        let p = d.join("pip.log");
        std::fs::write(&p, b"").unwrap();
        let lock = hold_exclusively(p.clone(), 250);

        let s = store(&d);
        s.log("the line that explains everything");
        // Opening the contents is what the lock forbids; the size is still readable.
        assert_eq!(std::fs::metadata(&p).unwrap().len(), 0, "it could not be written yet");

        lock.join().unwrap();
        s.log("something later");
        let written = std::fs::read_to_string(&p).unwrap();
        assert!(written.contains("the line that explains everything"), "the kept line was written: {written:?}");
        assert!(written.contains("something later"));
        assert!(
            written.find("explains everything") < written.find("something later"),
            "and in the order it happened: {written:?}"
        );
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
