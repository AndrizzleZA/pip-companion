//! What Pip leaves behind when it dies.
//!
//! A stranger's crash is invisible unless the program writes it down itself, and the hook runs at
//! the one moment the program is known to be broken. So everything it needs — where to write, what
//! to say about this machine — is worked out here at startup, while things still work. The hook
//! then only formats and appends. In particular it never reopens `Storage`: a panic raised from
//! inside storage would panic a second time on the way out and abort with nothing written at all.

use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

struct Ground {
    log: PathBuf,
    os: String,
}

static GROUND: OnceLock<Ground> = OnceLock::new();

/// Remembers where to write and what this machine is, then takes over panics. Called once, early.
pub fn install(log: PathBuf, os: String) {
    let _ = GROUND.set(Ground { log, os });
    std::panic::set_hook(Box::new(|info| {
        // Captured in here rather than handed in: the backtrace is the only part of a report that
        // says *how* it got there, and it has to be taken while those frames still exist.
        let trace = std::backtrace::Backtrace::force_capture();
        record(&info.to_string(), &trace.to_string());
    }));
}

/// The report goes in `pip.log` beside the ordinary lines, not in a file of its own, because what
/// Pip was doing in the minutes before matters as much as the panic itself.
fn record(summary: &str, trace: &str) {
    let Some(g) = GROUND.get() else { return };
    let when = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
    let text = format!(
        "
──── crash ────
{when}  Pip {ver}  ·  {os}  ·  thread {thread:?}
{summary}
{body}
──── end ────
",
        ver = env!("CARGO_PKG_VERSION"),
        os = g.os,
        body = readable(trace),
    );
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&g.log) {
        let _ = f.write_all(text.as_bytes());
        let _ = f.flush();
    }
}

/// A backtrace is worth printing only when it names Pip's own functions.
///
/// Windows keeps debug symbols in a separate `pip.pdb`, which is five times the size of Pip and so
/// is archived rather than shipped. On the machine that built it every frame resolves; on anyone
/// else's, every frame of ours comes back `<unknown>` - not even an address - and thirty lines of
/// that would bury the one line that does help, which is where it panicked. So: print the frames
/// when they mean something, say plainly why there are none when they do not, and in both cases
/// drop the panic machinery, which is the same six frames every time and never the cause.
fn readable(trace: &str) -> String {
    let noise = ["std::backtrace", "std::panicking", "core::panicking", "std::sys::backtrace", "pip::crash"];
    let mut out: Vec<&str> = Vec::new();
    let mut keeping = false;
    for line in trace.lines() {
        let t = line.trim_start();
        let header = t.split_once(": ").is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
        if header {
            keeping = !noise.iter().any(|n| t.contains(n));
        }
        if keeping {
            out.push(line);
        }
    }
    if out.iter().any(|l| l.contains("pip::")) {
        out.join("
")
    } else {
        "no usable backtrace: this build's symbols (pip.pdb) are not on this machine, so the location above is the evidence".into()
    }
}
