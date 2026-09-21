# Pip — a tiny companion that lives on your computer

A small creature that sits quietly on your desktop and gently reminds you about the things humans
forget: water, stand-ups, vitamins, calling someone back. It is built to be *barely noticeable as
software*: one ~1 MB executable, one process, a few MB of RAM, and no CPU while nothing is happening.

See [docs/DESIGN.md](docs/DESIGN.md) for the research, architecture decision, performance targets
and roadmap.

## Using it

- **Drag** the pet anywhere; it remembers the spot and snaps onto the taskbar edge.
- **Hover** to see the time. **Click** for today's card (time, date, water, next reminder).
- **Double-click** to log a glass of water.
- **Right-click** (pet or tray icon) for: log water / undo, Do not disturb, Always on top, Settings, Quit.
- **Tray icon** click shows/hides the pet. When hidden, reminders arrive as Windows notifications.
- Reminders appear as a speech bubble: **Done / Snooze / Dismiss**. Ignored bubbles fold into a
  small badge on the pet after a few minutes; click the pet to bring it back.

**Dates & calendars.** Add birthdays, anniversaries and deadlines in Settings › Dates (with an optional
heads-up a few days before). South African public holidays are built in. In Settings › Calendars (or right-click › Add a
calendar…), pick Google, Outlook or iCloud: Pip opens the right page and picks the link up as soon as
you copy it — no pasting needed. Then events show on today's card, and important ones (own alert, important calendar or a
keyword like "dentist") get a gentle alert. A quiet morning summary lists what's on.

The pet naps during quiet hours, when you're away (5 min without input) and while the PC is locked,
and holds reminders during fullscreen games, videos and presentations.

## Building

Requires Rust (stable) and the MSVC build tools.

```bash
cargo test -p pip-core              # core logic tests
cargo build --release               # target/release/pip.exe (~1.1 MB, no dependencies)
cargo build --features devtools     # dev build with the automation/snapshot command channel
```

Run tools against a throw-away profile by setting `PIP_DATA_DIR`; the app then never touches your
real settings or the Windows startup entry.

```bash
powershell -ExecutionPolicy Bypass -File tools/measure.ps1 -Minutes 10   # resource targets
```

## Layout

```
core/          pip-core: settings, reminder engine, quiet hours/DND policy, mood, copy (no OS code)
app-windows/   Windows shell: layered pet window, renderer, tray, settings window, persistence
docs/          design document
tools/         measurement script
```

Data lives in `%APPDATA%\Pip` (`settings.json`, `state.json`, `pip.log`). Nothing is sent anywhere.
