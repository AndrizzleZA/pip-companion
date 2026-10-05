# Pip — a tiny companion that lives on your computer

A small creature that sits quietly on your desktop and gently reminds you about the things humans
forget: water, stand-ups, vitamins, calling someone back. It is built to be *barely noticeable as
software*: one ~1 MB executable, one process, a few MB of RAM, and no CPU while nothing is happening.

See [docs/DESIGN.md](docs/DESIGN.md) for the research, architecture decision, performance targets
and roadmap.

## Installing

Double-click `pip.exe` and choose **Yes** when it offers to install. Pip copies itself to
`%LOCALAPPDATA%\Programs\Pip`, adds a Start-menu entry, starts with Windows and appears in
**Apps & features** like any other program — all per-user, so no administrator rights and no UAC
prompt. Choose **No** to run it from where it is (portable). Settings › About has the same
**Install** button, and **Remove Pip** when it is installed.

## Using it

- **Drag** the pet anywhere; it remembers the spot and snaps onto the taskbar edge.
- **Hover** to see the time. **Click** for today's card (time, date, water, next reminder).
- **Double-click** to log a glass of water.
- **Right-click** (pet or tray icon) for: log water / undo, Do not disturb, Always on top, Settings, Quit.
- **Tray icon** click shows/hides the pet. When hidden, reminders arrive as Windows notifications.
- Reminders appear as a speech bubble: **Done / Snooze / Dismiss**. Ignored bubbles fold into a
  small badge on the pet after a few minutes; click the pet to bring it back.

**Dates & calendars.** Add birthdays, anniversaries and deadlines in Settings › Calendar (with an optional
heads-up a few days before). South African public holidays are built in. Calendars come from Google
(Settings › Connections): events show on today’s card, important ones (own alert, important calendar
or a keyword like “dentist”) get a gentle alert, and a quiet morning summary lists what’s on.

**Connections.** Settings › Connections has three entries, and they all work the same way: click *Continue
with Google*, choose your account, Allow, and you’re back in Pip, connected. **Gmail** gives Pip the
sender, subject and labels of new mail — never the contents — and it can’t send or delete. **Google
Calendar** is read-only. Disconnect (asks once more) also withdraws Pip’s access at Google; if Google
ever signs Pip out, the entry shows **Reconnect**. Important senders and subject words live in
Settings › Email, along with two one-tap rules: mail Gmail marks Important, and mail that is already
starred when it arrives. When Pip nudges you about a single email, the bubble offers **Always from
this sender** — one click and that address always gets through.

**Reminders.** A reminder has a name, one or more times of day, and the days it repeats — so
"take pills" at 08:00, 14:00 and 20:00 is one reminder, not three. On the phone they behave like the
phone’s own alarm: exact, out of Doze, and over the lock screen with Snooze and Done. While you are
using the phone the same two buttons arrive as a notification instead of taking the screen. You can
add and edit reminders on either device.

**Your other devices.** Sync keeps reminders, dates, water history and preferences the same on every
device, through one encrypted file in your own Google Drive app folder — Pip has no server. Sign in
with the same Google account on the next device and it asks to join; this one shows *“… wants to
sync — Approve”*. One tap, nothing to type, and the key is sealed so only that device can open it.

The pet naps during quiet hours, when you're away (5 min without input) and while the PC is locked,
and holds reminders during fullscreen games, videos and presentations.

## Building

Requires Rust (stable) and the MSVC build tools.

```bash
cargo test -p pip-core              # core logic tests
cargo build --release               # target/release/pip.exe (~1.6 MB, no dependencies)
cargo build --features devtools     # dev build with the automation/snapshot command channel
```

Run tools against a throw-away profile by setting `PIP_DATA_DIR`; the app then never touches your
real settings or the Windows startup entry.

**Google sign-in identity.** One-click "Connect Gmail" needs Pip's own OAuth client (type *Desktop
app*) from a Google Cloud project with the Gmail API enabled and the consent screen published.
Download its JSON from the Google Cloud console and save it as `app-windows/google-client.json`
(git-ignored) before building; `build.rs` compiles it in. For installed apps Google treats this as an
app identifier, not a secret. This is a one-time developer step: users never see it. Without it, Pip still builds
but can’t connect Google accounts (release builds print a warning). `node tools/mock-google.js` + a devtools build test the whole flow offline.

```bash
powershell -ExecutionPolicy Bypass -File tools/measure.ps1 -Minutes 10   # resource targets
```

## The phone app

`app-android/` is Pip on Android: the same Rust brain (`app-android/rust`, a small JNI bridge over
`pip-core`) behind a Compose screen. Sign in with the same Google account as the PC, tap Approve on
the PC when it asks, and the same reminders appear.

Building it needs a JDK 17, the Android SDK (platform 34, build-tools 34, NDK 26) and `cargo-ndk`:

```bash
rustup target add aarch64-linux-android
cargo install cargo-ndk
cd app-android && ./gradlew assembleDebug        # app/build/outputs/apk/debug/app-debug.apk
```

Gradle runs `cargo ndk` first, so the `.so` in `jniLibs` is always built from the Rust you have —
nothing binary is checked in. `local.properties` points at the SDK; `ANDROID_NDK_HOME` must point at
the NDK.

**Google sign-in identity.** Like the desktop, the phone needs Pip's own OAuth client — type
*Android*, with the package name `com.pip.companion` and the signing certificate's SHA-1. Save the
client id as `app-android/google-client-android.json` (git-ignored); the build compiles it in.
Android clients carry no secret: Google checks the app's signature instead.

## Layout

```
core/          pip-core: settings, reminder engine, quiet hours/DND policy, mood, copy (no OS code)
app-windows/   Windows shell: layered pet window, renderer, tray, settings window, persistence
app-android/   Android shell: Compose screen, Google sign-in, sync; rust/ is the JNI bridge to pip-core
docs/          design document
tools/         measurement script, mock Google server for sign-in tests
```

Data lives in `%APPDATA%\Pip` (`settings.json`, `state.json`, `pip.log`). Nothing is sent anywhere.
