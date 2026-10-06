# Pip — a tiny companion that lives on your computer

Design document. Written before any code, kept up to date as the project evolves.

---

## 1. What we are building (concept analysis)

A small creature that sits quietly on the desktop (and later the phone) and gently helps
the user remember the things humans forget: water, appointments, birthdays, important email.

It has two jobs, and the order matters:

1. **Be invisible as software.** Idle cost must be indistinguishable from zero. It should feel
   like a mouse utility, not an app.
2. **Be present as a character.** Blinks, naps, reacts, celebrates — but only when something
   happens, and never nags.

Everything else (calendar, email, sync, mobile) is a *source of events* feeding the same loop:

```
 sources (timers, calendar, email, user) → core decides (priority, quiet hours, DND, snooze)
                                         → creature expresses it (bubble, expression, tray/toast)
                                         → user responds (done / snooze / dismiss) → mood & stats
```

## 2. Research: what similar apps teach us

| App / category | What works | What to avoid |
|---|---|---|
| **Tamagotchi** | A tiny care loop creates attachment; states readable at a glance. | Punishing neglect (death, sulking). Our pet never dies and never guilt-trips. |
| **Finch** (self-care pet) | Warm, non-judgemental tone; tiny rewards; gentle streaks. | Heavy app with lots of content we don't need. |
| **Plant Nanny / WaterMinder** | Hydration tied to a character that visibly benefits. | Fixed-interval spam regardless of whether you just drank. |
| **Shimeji, Desktop Goose, BonziBuddy** | Desktop pets are charming; transparent windows, drag to move. | Chaos/intrusion (Goose); spyware reputation (Bonzi). Lesson: privacy first, never grab focus. |
| **Stretchly, Workrave, Big Stretch** | Configurable break reminders; *pause during fullscreen apps*; idle detection. | Forced full-screen breaks feel hostile. |
| **Windows Focus Assist / DND** | Respect "busy" states (games, presentations). | — |
| **Rainmeter** | Native widgets can be tiny... | ...until a skin updates every second — update rate *is* the cost. |
| **PowerToys, Electron tray apps** | Great UX. | 5–15 helper processes, 100–300 MB idle. The thing we are *not* building. |
| **Thunderbird / mail clients** | IMAP **IDLE** = server push over one idle TCP socket, no polling. | Downloading bodies you never read. |

**Why background apps get heavy**
1. A bundled browser runtime (Electron ~150–300 MB; Tauri/WebView2 ~30–50 MB across several processes).
2. Continuous render loops (60 fps idle animation = constant CPU/GPU).
3. Polling: a clock repainting every second, email every minute, "is the user idle?" every second.
4. Helper processes: updaters, crash reporters, services.
5. Decoded assets held in memory (GIF/video sprites, fonts, images at every scale).
6. Telemetry/logging writing to disk continuously.

## 3. Major technical challenges

1. **Transparent window that is click-through where empty and never steals focus** → per-pixel-alpha
   layered window (`UpdateLayeredWindow`) + `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`.
2. **Animation without a render loop** → animations are short, event-triggered bursts; the frame
   timer exists only while one plays. Idle = a blink every few seconds; nothing at all while asleep.
3. **Reliable reminders across sleep/wake, clock changes, lock screen** → compute from the wall clock
   (never accumulate intervals) and recompute on `WM_POWERBROADCAST`, `WM_TIMECHANGE`, unlock.
4. **Not annoying** → priorities, quiet hours, DND, snooze, "user is away", "user is in a fullscreen
   game" (defer, don't interrupt), one bubble at a time, never takes focus.
5. **Email without polling or passwords** → OAuth 2.0 PKCE in the system browser + IMAP IDLE on
   desktop; headers only; rules evaluated locally; tokens in the OS credential vault.
   *Research caveat:* Gmail's IMAP scope is "restricted". A personal Google Cloud project left in
   "Testing" gets refresh tokens that expire after 7 days, so Phase 3 must publish the personal
   project as "In production (unverified, <100 users)".
6. **Mobile background limits** → phones forbid long-running background work. The shared core
   computes *when*; the OS scheduler (Android AlarmManager/WorkManager, iOS UNUserNotificationCenter)
   delivers. No persistent service, no wake locks.
7. **Cross-platform without a heavy framework** → shared logic, thin native shells.
8. **Sync without complexity/battery** → local-first; every record has an id + `updated_at`;
   sync is an occasional merge of small documents, not a live connection.

## 4. Architecture decision

**Rust shared core + thin native shell per platform.**

```
 ┌─────────────────────────── pip-core (Rust, no OS dependencies) ───────────────────────────┐
 │ settings · reminder engine · quiet hours/DND/snooze policy · mood · messages               │
 │ (later) calendar model · email rules · sync merge                                           │
 └──────────────┬───────────────────────────┬──────────────────────────────┬──────────────────┘
                │ plain Rust                │ UniFFI → Kotlin              │ UniFFI → Swift
     ┌──────────▼───────────┐   ┌───────────▼───────────┐       ┌──────────▼──────────┐
     │ Windows shell (Rust) │   │ Android: Compose app, │       │ iOS: SwiftUI app,   │
     │ Win32 layered window,│   │ Glance widget,        │       │ WidgetKit widget,   │
     │ tray, OS events      │   │ AlarmManager          │       │ local notifications │
     └──────────────────────┘   └───────────────────────┘       └─────────────────────┘
```

| Option | Idle RAM (Windows) | Mobile story | Verdict |
|---|---|---|---|
| Electron | 150–300 MB, 4+ processes | none | Rejected — the opposite of the brief. |
| Tauri 2 (WebView2) | ~30–50 MB + WebView2 process tree | webview on mobile | Rejected — 10× our RAM target to show a pet. |
| Flutter | ~40–80 MB, engine always resident | good | Rejected — heavy for a background pet; transparent desktop windows awkward. |
| .NET WinForms/WPF | ~20–40 MB | MAUI (heavy) | Rejected — OK but not "barely noticeable"; weak sharing. |
| **Rust core + native shells** | **~2–6 MB, 1 process** | core shared via UniFFI (as Firefox mobile does) | **Chosen.** |

Windows shell specifics:
- **Rendering:** `tiny-skia` (pure-Rust anti-aliased 2D) draws the creature procedurally → crisp at
  any size/DPI, free recolouring, no image assets in memory. Text via `ab_glyph` over the system's
  Segoe UI, memory-mapped (shared pages, no private copy). One `UpdateLayeredWindow` per changed frame.
- **Event-driven:** one Win32 message loop; every timer is one-shot, set to "when is the next thing".
  Power, lock/unlock, time change, fullscreen apps (AppBar `ABN_FULLSCREENAPP`) and display changes
  arrive as OS messages. Nothing is polled except a 20 s "are you back?" check while the pet naps
  because you're away.
- **Notifications:** the speech bubble is the primary channel (subtle, never takes focus). When the pet
  is hidden, the tray's native notification (a Windows toast) is used for normal/important items.
- **No extra processes**, no services, no updater daemon. Autostart = one `HKCU\...\Run` value.
- **Storage:** two small JSON files in `%APPDATA%\Pip` (settings, state), atomic, debounced, written
  only on change.
- **Secrets (Phase 3):** Windows Credential Manager (DPAPI), never in our files.

Code layout:

```
pip/
  core/                 pip-core — pure logic, unit tested, reused on every platform
    settings.rs         user settings model
    state.rs            persisted runtime state (stats, last drink, snoozes, DND)
    schedule.rs         reminder engine: next due, quiet hours, days, missed items
    mood.rs             mood / expression model
    messages.rs         gentle copy
  app-windows/          Windows shell
    main.rs             entry, single instance, message loop, controller
    pet.rs              layered creature window: input, drag, bubble, animation driver
    render/             canvas, creature, text, widgets
    settings_ui.rs      custom-drawn settings window (exists only while open)
    tray.rs             tray icon + native menu
    platform.rs         autostart, idle time, fullscreen, DPI, sound
    storage.rs          atomic JSON persistence
  tools/measure.ps1     resource measurement
```

## 5. Performance targets (Windows) — measured, not assumed

| Metric | Target | Stretch |
|---|---|---|
| Processes | 1 | 1 |
| Idle private memory | ≤ 8 MB | ≤ 5 MB |
| Idle working set | ≤ 15 MB | ≤ 10 MB |
| Idle CPU (pet visible, blinking) | ≤ 1 s CPU per hour (0.03 % of one core) | ≤ 0.3 s/h |
| Idle CPU (away / locked) | ≤ 0.2 s per hour | ~0 |
| Wake-ups while idle | ≤ 1 per 4 s average | none while locked (except scheduled) |
| Startup: process start → pet visible | ≤ 150 ms | ≤ 60 ms |
| Reminder latency after due time | ≤ 1 s | — |
| Settings window | opens ≤ 150 ms, ≤ +10 MB, released on close | — |
| Executable | ≤ 3 MB, no runtime dependencies | ≤ 1.5 MB |
| Network (Phase 1) | 0 bytes | — |
| Disk writes while idle | 0 | — |

Mobile (Phase 4): no persistent background service; OS-scheduled notifications; background sync
limited to a few short runs per day; battery attribution "negligible".

## 6. Roadmap

**Phase 1 — MVP (desktop foundation)** ← current
- Procedural creature with expressions and event-driven animations (blink, hop, wave, celebrate,
  drink, look-around, nap/wake).
- Layered desktop window: drag, remembered position, optional always-on-top, hide to tray.
- Tray icon + native menu; single instance; start with Windows.
- Time on hover/click (info card); optional mini clock that ticks once a minute.
- Reminder engine: hydration (interval since last drink, active window, days, daily goal) and custom
  reminders (time + repeat days, important flag, missed-while-off).
- Notification policy: priorities, quiet hours, DND (1 h / until tomorrow), snooze, dismiss, complete,
  pause during fullscreen apps, hold while locked.
- Mood from hydration progress and interaction; gentle rotating copy.
- Custom-drawn settings (Pet, Water, Reminders, Quiet, About/diagnostics), applied instantly.
- Measurement script; targets verified.

**Phase 2 — Dates & calendar**: important dates (birthdays, anniversaries, deadlines) with lead time;
public holidays computed offline (South Africa first, pluggable); ICS/webcal subscriptions with
ETag/If-Modified-Since every 30–60 min (works with Google secret address, iCloud, Outlook); Google
Calendar (syncToken) and Microsoft Graph (delta) via OAuth PKCE; important vs informational; agenda card.

**Phase 3 — Email**: OAuth PKCE, refresh token in Credential Manager; IMAP IDLE push; headers only;
local rules (sender, domain, keywords, flagged, combinations); discreet notifications.

**Phase 4 — Mobile + sync**: UniFFI bindings; Android (Compose + Glance widget + AlarmManager);
iOS (SwiftUI + WidgetKit + local notifications); local-first sync with per-record `updated_at`,
last-writer-wins per field, tombstones. Transport chosen then — preference for the user's own cloud
app-folder (no server of ours holding personal data).

**Phase 5 — Polish**: customisation (colours, accessories, variants), personality presets, more
animations, per-user installer, signed binary, opt-in update check, local crash log, security review,
accessibility, final polish.

## 7. Guiding rules

- Every feature must justify its resource and maintenance cost.
- One process, one UI thread, no polling where the OS offers an event.
- Privacy by design: nothing leaves the machine in Phase 1; later integrations read only the metadata
  they need; no telemetry.
- The pet never grabs focus, never flashes full-screen, never guilt-trips.

## 8. Status — Phase 1 complete (2026-09-21)

Measured with `tools/measure.ps1 -Awake` (release build, pet forced awake = worst case, 10 min
steady-state window, CPU from exact cycle counts):

| Metric | Measured | Target |
|---|---|---|
| Processes | 1 | 1 ✅ |
| Private memory | 1.7 MB | ≤ 8 MB ✅ |
| Working set | 2.8 MB avg | ≤ 15 MB ✅ |
| Idle CPU | 0.96 s per hour (0.03 %) | ≤ 1 s/h ✅ |
| Startup | 17–32 ms | ≤ 150 ms ✅ |
| Reminder accuracy | fired 60.15 s after a 60 s interval | ≤ 1 s ✅ |
| Executable | 1.1 MB, no dependencies | ≤ 3 MB ✅ |

Lessons from measuring: blinks were first driven by the 30 fps animation clock (~7 frames each) —
now two frames; `GetProcessTimes` is tick-sampled and unreliable for sub-ms wakeups, so CPU is
measured in cycles; a one-time working-set trim after startup returns pages only used to boot.

Tested: scheduling end to end, snooze/done/dismiss, lock/unlock hold-and-deliver, resume, time
change, fullscreen hold (real "busy" state honoured), multi-monitor positions and clamping,
corrupt-file recovery, handle stability over repeated cycles, light/dark themes, all settings pages.

Known follow-ups: cache the bubble text layer (≈1 ms/frame while a bubble is open); per-user
installer + icon resource (Phase 5); test at 125/150 % scaling on a real high-DPI display.

## 9. Status — Phase 2 (dates & calendars), 2026-09-21

Built:
- **Important dates**: birthdays (age), anniversaries (years), yearly dates and deadlines, with
  an optional heads-up 1/3/7 days before; 29 Feb falls back to 28 Feb.
- **Public holidays (South Africa)** computed offline — Easter-based days and the Public Holidays
  Act's Sunday→Monday rule (e.g. Women's Day 2026 → Monday 10 Aug). One-off declared days
  (elections) can't be computed; subscribe to a holiday calendar to get those.
- **Calendar subscriptions (ICS/webcal)** from Google, Outlook, iCloud: WinHTTP (OS TLS, proxy
  and certificates; https only), ETag/Last-Modified, hourly with backoff (15→120 min), parsed and
  expanded on a short-lived worker thread; on-disk cache so calendars work offline and at startup;
  responses that aren't calendars never overwrite a good cache.
- **Privacy**: subscription links are secrets — DPAPI-encrypted in `secrets.dat`, masked in the UI,
  never in `settings.json`.
- **Important vs informational**: an event may interrupt only if it has its own alert, comes from a
  calendar marked important, or matches a keyword; everything else is agenda-only.
- **Morning summary** (low priority, silent, once a day, only when there's something to say) and
  today's agenda on the info card.

Measured: a sync costs ~46 ms CPU the first time (TLS + parse), ~13 ms on hourly re-syncs. WinHTTP's
worker threads retire within ~3 minutes; private memory settles at ~2.7 MB.

Still to do in Phase 2: tomorrow's all-day events in the summary, a date/time picker for one-off
deadlines further than a year out.

**Decision (2026-09-21):** calendars connect by subscription link, not OAuth sign-in. To make that
easy, "Add calendar" is a guided flow: pick Google / Outlook / iCloud / other → Pip opens the exact
settings page → the user copies the link → Pip picks it up from the clipboard automatically
(clipboard change notifications, listened to only during the 10-minute add window; reads are
deferred 200 ms so they never collide with the copying app). Copied links are classified, and the
common mistakes (a private Google calendar's *public* address, Outlook's HTML link, a website link,
plain http) get a plain-language fix instead of a silent failure.

## 10. Status — Phase 3 (important email), 2026-09-21

Decision: **Gmail only** (user's choice), via the Gmail API with the **`gmail.metadata`** scope —
Pip gets sender, subject and labels of new mail and is technically unable to read bodies or
attachments, send, or delete. (IMAP IDLE was rejected: it needs the full-mailbox scope, a TLS
library and a permanent socket.)

- **Sign-in**: OAuth 2.0 authorization code + PKCE (S256) with a 127.0.0.1 loopback redirect in the
  user's own browser, anti-forgery `state`, granted-scope check. Refresh token, Client ID and secret
  are DPAPI-encrypted in `secrets.dat`; access tokens live only in memory. Disconnect revokes.
- **Setup**: the user owns the Google app (personal use; one-time "unverified app" click-through).
  Settings › Email walks through it with Open buttons for each console page and picks the Client
  ID/secret up from the clipboard (or the downloaded JSON). The steps stress publishing the app,
  since "Testing" apps get refresh tokens that expire after 7 days, and copying the secret at once
  (Google shows it only once).
- **Checking**: `history.list` since the last historyId (tiny when nothing changed), then
  `messages.get?format=metadata` only for new unread inbox messages (capped at 40). Every few
  minutes while the user is at the PC; not while away/locked (checks on return); every 15 minutes
  in quiet hours. Expired access → silent refresh + retry; expired history (404) → quiet
  re-baseline; revoked → "signed out" message, checks stop.
- **Rules** (pip-core, unit tested): senders by address, @domain or name; subject words; optional
  Gmail "Important" marker; only unread inbox mail; look-alike domains don't match. RFC 2047
  encoded names/subjects decoded.
- **Delivery**: one grouped bubble ("Email from Mom" / "3 important emails") with Open Gmail,
  Got it, Snooze — deduplicated, persisted, through the usual quiet-hours/DND/fullscreen policy.

Testing: `tools/mock-google.js` imitates Google's OAuth and Gmail endpoints (verifies PKCE,
scope, redirect; supports token expiry, history expiry, revocation, paging). Dev builds can point
at it via `PIP_OAUTH_AUTH`/`PIP_OAUTH_TOKEN`/`PIP_GMAIL_API` (http://127.0.0.1 only); release builds
compile those overrides out. End-to-end run covered sign-in, rules, refresh, paging (5 mails over
3 pages), history re-baseline, revocation and re-sign-in.

Not yet verified against real Google (needs the user's own Google app).

## 11. Connectors — one-click linking, 2026-09-21

Goal (user): connecting Gmail and other services must be as close to one click as possible, with
no API keys, tokens, redirect URLs or OAuth vocabulary shown to the user.

**Research.** Google's installed-app guidance: authorization code + PKCE with a loopback redirect
(custom URI schemes and copy/paste "OOB" are no longer supported for desktop); installed apps
"cannot keep secrets", so a desktop client's ID/secret are app identifiers, not credentials; check
the granted scopes because users can untick permissions (granular consent); `access_denied` means
the user cancelled. Established mail apps (Thunderbird, Mailspring, eM Client, gcloud, rclone)
ship **one app-owned client** and the user only signs in. `gmail.metadata` is a *restricted*
scope: without Google's verification + annual security assessment the consent flow shows
"Google hasn't verified this app" and is capped at 100 users — fine for personal/family use,
the verification is a Phase 5 item if Pip is ever distributed publicly.

**Decision.** Pip ships its own Google client, compiled in by `build.rs` from a git-ignored
`app-windows/google-client.json`. The user's journey is: Connect → browser (choose account,
Continue past the unverified notice, tick the box, Continue) → "Pip is connected" page → back in
Pip showing the account. The previous "make your own Google app" setup stays only as a fallback
for builds without a built-in client (and still works if someone already set it up).

**Architecture.**
- `pip-core::connectors` — the catalogue (name, badge, purpose, plain-language access text,
  method: browser sign-in or private link) and `SignInProblem` with user-facing explanations.
  Adding a connector = one catalogue entry + its sync code.
- `oauth.rs` — generic over a `Service` (endpoints + scope); the service-specific "who is this?"
  step is a function passed in (`mailsync::verify_gmail`). A Microsoft connector would be a new
  `Service` + verify fn + its sync module.
- UI: Settings › Connections (new page) lists every connector with status (Connected / Needs
  attention / Reconnect), the same Gmail card also heads Settings › Email.

**States handled** (all tested end-to-end against `tools/mock-google.js`, which now also simulates
the user cancelling, unticking the permission, closing the tab, the app being rejected, and the
revoke endpoint): not connected → waiting (steps to expect + "Open browser again" + Cancel) →
connected; declined (calm message, "Connect"); missing permission / offline / app rejected /
10-minute timeout (red explanation, "Try again"); forged or stale redirect (ignored, still
waiting); Pip-side cancel (listener closed immediately); Google revokes → "Reconnect" with the
account preselected (`login_hint`) and the same account record reused; second account; disconnect
(two-click confirm, token revoked at Google, local data removed).

**Cost.** No new threads or timers while idle (the sign-in thread exists only during a sign-in,
the waiting panel is static — no spinner redraws). Release exe +19 KB (1.64 MB); startup 24 ms;
private memory ~1.8 MB unchanged. Test-only endpoint overrides are compiled out of release.

## 12. Connecting Google — reduced to Google's own screens, 2026-09-21

User's rule: *"I want to connect my Gmail" → click Google → choose account → done.* No guides,
keys, consoles or configuration in front of the user, ever.

**Why it was complicated.** Not Google's requirements — ours. (1) Pip had no registered Google
identity of its own, so the app fell back to making *each user* register a Google Cloud app
(the 5-minute, 4-step guide with key copying). Google requires every app to be registered once
by its **developer**; making users do it was the whole problem. (2) Pip added its own steps on
top: a forced account picker, a 3-step "what to expect" panel, and a "tick the box" step that
doesn't even exist (Google shows permission checkboxes only when an app asks for more than one
permission; Pip asks for exactly one).

**What Google genuinely requires of the user:** choose the account (skipped automatically when
only one is signed in), the "hasn't verified this app" notice (unavoidable for Gmail data until
Google's paid review + security assessment; ≤100 users meanwhile), and Allow. Everything else was
removed. Alternatives checked and rejected: the device-code flow (Google doesn't allow Gmail
scopes there), a hosted relay/broker service (still needs a registered app, adds a server and a
privacy surface), IMAP/app passwords (passwords — ruled out), borrowing another app's
registration (against Google's terms).

**Now.** One button — *Continue with Google* — opens Google; Pip shows one line ("Continue in your
browser…", "Open it again", Cancel) and a one-sentence heads-up for the unverified notice; the
browser lands on "Pip is connected" and Pip comes to the front showing the account. The account
picker is forced only for *Add account*; *Reconnect* preselects the account. Failures read as one
plain sentence (technical detail goes to `pip.log`). The user-facing "own Google app" setup,
its clipboard key capture and `find_client_keys` were deleted (−115 KB release size).

**Developer step (once, not per user):** register Pip in Google Cloud (Gmail API on, consent
screen published, Desktop client) and drop the downloaded JSON at
`app-windows/google-client.json`; `build.rs` compiles it in and warns in release builds without it.

Tested against `tools/mock-google.js` with real button clicks: fresh connect, waiting, connected,
two-click disconnect (revoked at Google), connect again, add second account (picker shown),
Google signs Pip out → Reconnect (preselected, no picker), mail still detected afterwards,
user cancels on Google's screen, app rejected, tab closed → Cancel, offline → plain message →
try again → connected. 51 unit tests pass; startup 22 ms, ~1.8 MB private memory.

## 13. Google Calendar without a link, 2026-09-22

The same "click, choose account, Allow" flow now covers calendars.

**Why only Google.** Apple has no OAuth for iCloud calendars at all: third-party apps must use
CalDAV with a hand-made app-specific password, which is a password, so Pip keeps the link flow
there. Microsoft does support a proper sign-in, but registering the app needs an Azure directory,
which in turn needs a (free) Azure sign-up with card verification — a decision for the owner of
the app, not something Pip can do. Both stay on the guided link flow until then.

**Shape.** A Google-connected calendar is an ordinary `CalendarFeed` whose url is `gcal:<address>`
instead of an https link, so everything downstream (on/off, "important", alerts, the morning
summary, the two-week window, the Calendars page) works unchanged. `calsync::sync_feed` branches on
that prefix: refresh the access token, `calendarList` (only calendars ticked in Google Calendar),
then `events.list` per calendar with `singleEvents=true` — Google expands repeats, so none of the
RRULE machinery runs. Results are cached as JSON, so events survive restarts and offline spells
exactly like `.ics` feeds do.

- Scope: `calendar.readonly` only — a single permission, so Google shows no tick-boxes.
- Its refresh token lives in `secrets.dat` under `gcal:<feed id>:refresh`; Disconnect revokes it
  and deletes the cache.
- Sign-in state is per connector (`SignIn::Waiting { connector, .. }`), so a cancelled Gmail
  sign-in never shows on the calendar card and vice versa.
- Parsing lives in `pip-core::gcal` (unit tested, ready for the mobile shells).

Tested against the extended mock (calendarList + paged events): connect, 3 events from two
calendars with the hidden one skipped, calendar default reminders applied, Google signs Pip out →
Reconnect with the account preselected, events kept from cache while signed out and while offline,
restart with no network, decline, and two-click Disconnect (token revoked, cache removed). Gmail
and the link flow still pass their own tests. 55 unit tests; release 1.71 MB, startup 37 ms.

## 14. One-click or not at all, 2026-09-22

User's rule: every connector works the same way (click, choose account, allow) or it goes.

- **Removed:** iCloud Calendar and the generic "paste any .ics link" connector. Apple has no OAuth
  for calendars — only CalDAV with a hand-made app-specific password — so iCloud could never meet
  the rule. The `.ics` sync engine stays (Outlook uses it, and existing feeds keep working); only
  the entries in the catalogue and the "Add calendar" picker are gone.
- **Outlook Calendar** stays link-based for now: Microsoft only registers apps inside an Azure
  directory, which needs a free Azure sign-up with card verification on the owner's account.
  Pending that decision it keeps the guided link flow; otherwise it goes the same way as iCloud.
- The picker on Settings › Calendars now offers Google (sign-in) and Outlook (link).

**Google app published (production).** Pip's consent screen now carries a home page and privacy
policy (https://sites.google.com/view/pip-companion, on the owner's Google account; the authorised
domain must be the top private domain `google.com`), and the app's publishing status is
**In production**. That ends the 7-day refresh-token expiry that "Testing" apps get; the
"Google hasn't verified this app" notice stays until (and unless) the app goes through Google's
paid verification, and the 100-user cap applies while unverified.

## 15. Google-only connectors, 2026-09-23

The owner declined the Azure sign-up Microsoft requires, so by the standing rule ("one click or it
goes") the Outlook connector and the whole subscription-link flow were removed:

- `pip-core::links` (link recognition) deleted; `Method` now has a single variant, `SignIn`.
- Gone from the shell: the provider picker, the guided "copy the link" wizard, its hints and
  10-minute window, the **clipboard listener** (`AddClipboardFormatListener`, `WM_CLIPBOARDUPDATE`,
  the T_CLIPWAIT/T_CLIPREAD timers) and the tray's "Add a calendar…" entry. Pip no longer watches
  the clipboard at all, which removes an OS hook and a whole class of privacy questions.
- The `.ics` engine (`core::calendar` + the calsync ICS branch) stays: it costs nothing at rest and
  keeps any pre-existing subscribed feed syncing. Nothing in the UI can create one any more.
- Settings › Calendars still lists calendars (name, on/off, important, delete); connecting and
  disconnecting happens on Connections.

Result: every connector in Pip is now a browser sign-in. 52 unit tests; release 1.69 MB (−16 KB),
startup 31 ms, ~1.9 MB private memory. Both connectors re-verified against the mock afterwards:
connect, reconnect, disconnect, and Gmail's rules still firing.

## 16. Phase 5 — installing without an installer, 2026-09-23

Pip is one executable, so it installs itself rather than shipping NSIS/Inno/WiX and a second
program to maintain:

- `pip.exe` run from a folder offers **Install Pip on this PC?** once (No = run portable). The same
  buttons live in Settings › About, and `--install` / `--uninstall [--silent]` work from a command
  line (that's what Apps & features calls).
- Install: copy to `%LOCALAPPDATA%\Programs\Pip`, Start-menu shortcut, `HKCU\…\Uninstall\Pip`
  entry (name, version, publisher, size, icon, uninstall strings) and autostart pointing at the
  installed copy. Everything is **per-user**: no administrator rights, no UAC prompt, nothing
  outside the user's own profile.
- Uninstall: autostart, shortcut and registry entry removed, `%APPDATA%\Pip` deleted, then a
  detached `cmd` removes the executable and folder a few seconds later (a running program can't
  delete itself). The command line is passed with `raw_arg` because cmd.exe wants it verbatim.
- The shortcut is written by the shell scripting host once, at install time — a .lnk needs COM
  plumbing that isn't worth carrying in the binary, and nothing runs at runtime.
- Settings › About says where this copy lives and offers Install or Remove accordingly.

Measured after installing: startup 41 ms, ~2.0 MB private, 1.78 MB executable. Settings and both
Google connections carried over untouched (they live in `%APPDATA%\Pip`, which install never
touches). Install and uninstall were tested end-to-end against a redirected profile first, so the
real machine only saw the final, working version.

## 17. Phase 4, part one — sync between devices, 2026-09-23

Before any Android tooling, the risky half was built and proven on the desktop: the data model,
the merge rules, the encryption and the transport.

**Where it lives.** One small encrypted file in the user's own Google Drive *application data
folder* — hidden from Drive, unreadable by other apps, reached with `drive.appdata`, which Google
classes as non-sensitive (no extra review, and Pip can touch nothing else in the account).

**What travels.** Reminders, important dates, the preferences block, water history, streak and
acknowledgements. Never the Gmail/Calendar sign-ins: each device connects those itself, so no
access token is ever copied.

**Merge rules** (`pip-core::sync`, unit tested, order-independent): newest edit per record wins;
deletions leave tombstones (recorded by the shell whenever an item disappears from settings) so a
stale device can't resurrect them; progress merges upward — highest water count per day, longest
streak, union of acknowledgements.

**Keys.** A random 256-bit key is made by whichever device starts the sync. It first travelled to
the next device wrapped in a typed pairing code; §18 replaces that with a single Approve tap. A
device that finds an existing sync refuses to start a rival one and asks to be let in instead.

**Tested end to end** against a mock Drive with two independent Pip profiles: first device starts
the sync, second joins with the code, edits flow both ways, a deletion propagates and stays
deleted, a wrong code is refused, an expired access token is refreshed and retried rather than
reported as a sign-out, and an offline round leaves local data untouched. Two bugs were caught this
way: a second device happily creating a rival key, and the file lookup trusting Drive's search
result without checking the name.

## 18. Adding a device with one tap, 2026-09-23

The pairing code in §17 was the wrong shape for this app. Everywhere else a connection is *Continue
with Google → choose → Allow → done*; sync alone asked the user to read sixteen characters off one
screen and type them into another. Same complaint as the Gmail setup guide, one layer down.

**The flow now.** Both devices sign in to the same Google account, nothing else. The new device
makes a key pair, leaves the public half in the shared folder as a join request, and waits. The
device that already holds the key shows *"Andre's phone wants to sync — Approve / Not now"*. One
tap seals the sync key for that one requester (X25519 to a shared point, Blake2b to a key,
XChaCha20-Poly1305 to seal). Only the device that kept the private half can open it.

**Why this is no weaker than a code.** The code only ever protected a file sitting in the user's own
Drive folder, so an attacker needed the Google account first — and with the account they could read
the code file anyway. The approval binds the key to one public key that the user has just said yes
to, and the sealed bytes are useless to anyone else. Google still sees only sealed bytes; the device
name in a join request is the one thing in the clear, so the user can tell which device is asking.

**Refusing properly.** *Not now* leaves a dated refusal marker. Without it the refused device simply
re-published its request seconds later and the card came back — found in testing, not in review. The
refused device now stops asking and offers *Ask again*.

**Housekeeping.** Requests, approvals and refusals expire after fifteen minutes and are swept up by
whichever device passes next, so the folder returns to a single encrypted document. A round lists the
folder once and works from that listing. The Connections page looks for waiting devices as it opens,
so a device that has just signed in appears within a second or two instead of at the next quarter-hour.

**Tested** with two, then three, profiles against the mock Drive: first device starts the sync,
second asks and is approved, a third is refused and stops asking, asks again and is let in, edits
flow both ways and a deletion stays deleted.

## 19. Phase 4, part two — Pip on the phone, 2026-09-23

**One brain, two shells.** The phone runs the same Rust as the PC. Everything that has to agree
byte for byte — the merge rules and the encryption — lives in `pip-core`; Kotlin owns the things
a phone owns: the screen, the browser sign-in, HTTP and (next) alarms.

**The bridge is one function.** The roadmap said UniFFI. The surface turned out to be eight calls
on JSON objects, so a hand-written JNI shim (`app-android/rust`, ~150 lines) does the same job
without a code generator in the build, and `pip-core` stays free of binding dependencies.
`Native.ask("merge", …)` in, JSON out, failures as `{"error": "…"}` so the app can show a sentence
instead of a stack trace. Byte strings travel as hex: a few kilobytes, no dependency either side.

**Signing in.** Android apps get their own OAuth client, with no secret — Google checks the app's
signature instead. The consent screen opens in the phone's own browser (Custom Tabs), PKCE ties the
answer to that one attempt, and Google hands it back to `com.pip.companion:/oauth2redirect`. The
permission is `drive.appdata` and nothing else, exactly as on the PC.

**Joining.** The phone's half of §18: sign in with the same Google account, and it leaves a join
request; the PC shows *"… wants to sync — Approve"*. The phone can approve other devices too — the
same code path, because there is no "main" device. If the account has no sync yet, whichever device
arrives first starts it, so a phone-first user isn't stuck.

**What it does today.** Sign in, join or be joined, pull the shared document, show reminders and
switch them on or off, push changes back. Notifications and the water card come next; this milestone
was about proving the same data, the same rules and the same key on a second kind of device.

**Size.** The Rust library is ~700 KB for arm64; the debug APK is a few megabytes with Compose. Only
arm64 is built — emulator ABIs can be added when something needs them.

## 20. Reminders that ring, and happen more than once a day, 2026-10-05

**Several times a day.** A reminder carries a list of times, not one. "Take pills · 08:00, 14:00,
20:00 · weekdays" is a single thing to name, switch off and delete — the shape every medication app
settles on, because three separate reminders with the same title is a list nobody can maintain. Each
time is its own occurrence: answering the morning one leaves the evening one alone. Records written
by an older Pip have a single `time` and no list; they are migrated on load, and that field is still
written so an un-updated device isn't left blind. The field is crate-private, since setting it alone
would have changed nothing — a mistake one of the tests made before it was closed off.

**Alarms on the phone.** `setAlarmClock` is the only API Android treats as a real alarm: exact,
allowed out of Doze, and shown in the status bar so the phone doesn't promise something it won't do.
Repeating alarms are deliberately inexact, so Pip arms exactly one — the next reminder — and arms
the next when it fires; a boot receiver puts it back, because Android forgets alarms on restart.
Pip qualifies for `USE_EXACT_ALARM`, so there is no permission to beg for, and it still checks
`canScheduleExactAlarms` and degrades to an inexact alarm rather than going quiet.

**Asleep: the alarm screen. Awake: a notification.** Locked or dark, the full-screen intent takes
over the way a morning alarm does — title, time, Snooze, Done — and keeps the screen on while it
rings. In use, the same two buttons arrive as a heads-up, because nothing should seize the screen
out from under someone mid-sentence. Android 14 lets people revoke full-screen alerts, so Pip asks
`canUseFullScreenIntent` and falls back instead of failing.

**What the first real alarm taught us**, none of which the mock could have:
- it stopped ringing when the screen turned off — the phone woke, showed the alarm, dozed, and the
  `onStop` handler silenced it. It now holds the screen and rings for a minute before giving up,
  leaving the notification behind rather than losing the reminder;
- arming from `next_event` woke the phone for quiet-hours boundaries and midnight and lit the alarm
  icon for them. It arms for reminders only, and cancels outright when there are none;
- a reminder from hours earlier was still "due" and rang like a fresh one. Late ones are a quiet
  "Earlier today" note now.

**Editing on the phone.** One screen: a name, the times as chips you tap to change and add to, seven
day circles, and an Important switch. Saving and deleting go through the engine (`save_reminder`,
`delete_reminder`) so times are ordered and deduplicated exactly as on the PC and a deletion leaves
a tombstone — the phone cannot write a document the PC would read differently.

## 21. Seven pages instead of nine, 2026-10-05

Settings had grown a page per feature, which is how a small app starts feeling like a big one.
Two of them were not really their own thing:

- **Water** is a reminder. It has a time window, days of the week and a daily goal, and the engine
  schedules it the same way it schedules the rest. It is now a **Water** section at the top of
  Reminders, above your own.
- **Dates** are what a calendar is for. Birthdays, anniversaries and holidays now sit as an
  **Important dates** section at the top of the renamed **Calendar** page, with the subscribed
  calendars and their alert rules below.

Nothing was hidden and nothing was dropped: every row, card and action moved across intact, the
header's add button follows the page it belongs to ("+ Add reminder", "+ Add date"), and the two
page functions became sections rather than being left behind unused. The early "nothing here yet"
return that used to end the Dates page now ends only its section, so the calendars below it still
draw.

Seven pages: Companion, Reminders, Connections, Calendar, Email, Quiet & alerts, About.

## 22. Marking mail important without typing, 2026-10-05

Two ways in, both using what Pip already has.

**Stars.** Gmail returns the full `labelIds` with every message Pip fetches, so `STARRED` was already
in hand — no new permission, no extra request. A star is something the user did on purpose, so it
outranks a guess from a subject word and sits just below the explicit sender rules.

The honest limit, which the toggle's wording reflects: Pip watches the mailbox *history* and only
examines messages that arrive unread in the inbox. Starring an existing message produces a
`labelsAdded` record that Pip ignores, so the rule catches a star that is **already on the message
when it arrives** — a filter, or a star applied before the next check. The toggle says exactly that
rather than promising more. Watching `labelsAdded` too would close the gap at the cost of more
traffic; not worth it yet.

**"Always from this sender."** When the nudge is about one email, the bubble offers it; one click
adds that address to the important senders. Nothing new is stored to make this work — the message is
already in hand for the bubble, and Pip still keeps no record of who emails you. The button is
hidden on the grouped "3 important emails" card, where there is no single sender to mean, and hidden
when a rule already covers them, which is the same `matching_sender` the importance check uses, so
the two can never disagree. The address is stored lowercase and never duplicated, including when an
`@domain` rule already covers it.

Proven against the mock Gmail: off then on; starred-but-read ignored; the button present, clicked,
and then absent for that sender's next email, with one rule added and no second.

## 23. Background work that can't stop for good, 2026-10-05

A real email, starred on arrival and left unread, produced no nudge. The rule was right and the build
was installed; mail checking itself had quietly stopped three hours earlier and never resumed.

The timers for mail, calendar and phone sync were one-shot by habit: the handler killed its own timer
and the round was re-armed only at the end of a *completed* pass.

```rust
T_MAIL => {
    self.kill_timer(T_MAIL);
    self.check_mail(false);
}
```

Every early return in between — no Google keys yet, no jobs, locked, away, a flag still saying
"running" — therefore ended that subsystem until the app was restarted, silently and permanently. The
first check after launch raced the secrets being opened, lost, and that was the end of email for the
session. The calendar had the same shape plus one of its own: its re-arm sat inside `if result.online`,
so a single offline pass stopped it for good. `calsync::start` even carried a comment promising that
"the next scheduled sync will try again" when there was no next one.

The rule now is that **a background round always leaves its successor armed**, whatever happens:

- The handler arms the next attempt *before* making it, so no return path inside can be the last one.
  A round that completes normally replaces that net with its proper interval.
- Every early return in `check_mail` calls `schedule_mail()`. The schedulers, not the callers, decide
  when there is genuinely nothing to arm — no accounts, no keys, no calendars — so stopping is a
  decision made in one place instead of an accident in five.
- A `running` flag is a hint, not a lock. Each is stamped with its start time and written off after
  `MAIL_STALL_SECS` / `SYNC_STALL_SECS`, so a worker that dies without reporting costs one cycle
  rather than the session.
- `mailsync::start` returns whether the thread actually started, and catches a panic inside it so an
  answer — even an empty one — always comes back.

The general rule: anything that repeats must be impossible to stop by accident. Prefer a wasted cycle
over a subsystem that goes quiet without saying so.

## 24. The round that never started, 2026-10-06

The same family as §23, from the other end. Water logged on the phone never reached the PC — not
slowly, never. Two PC processes ran for forty minutes between them without a single sync round.

Nothing was broken. Startup armed the calendar (20s) and email (25s) but never the phone sync, so
the first round only happened when something else asked for one: opening Connections, or an edit.
A PC left alone therefore never picked up anything the phone had done. It had gone unnoticed
because testing sync means opening Connections and editing reminders constantly — the two things
that happen to arm it.

```rust
self.set_timer(T_CALSYNC, 20_000);
if !self.settings.email.accounts.is_empty() {
    self.set_timer(T_MAIL, 25_000);
}
self.schedule_sync(30_000); // ← the phone had no equivalent
```

With it, the PC pulled the phone's glasses about forty seconds after launch.

§23 said anything that repeats must be impossible to stop by accident. The other half: **it has to
start on its own too.** A recurring job that only runs because something unrelated kicks it off
looks fine for as long as you keep kicking it, which is exactly what testing does.

It also leaves a lesson about diagnosis. The visible symptom was a count on screen that the file
did not have, which read like data being lost. Quitting — which flushes — wrote nothing, proving
the opposite: there was nothing in memory to save, so the PC had never received anything. The test
that looked like it would confirm the theory is what killed it.

## 25. Pip notices when it has gone quiet, 2026-10-06

§23 and §24 were the same bug twice: background work that stopped, or never started, with nothing
to see. That is the heart of it — for a companion that works out of sight, "nothing needed you" and
"this stopped working" look identical. You cannot tell a quiet inbox from a dead mail check, which
is exactly why one went unnoticed for three hours and another for days.

Three point fixes do not prevent a fourth, so Pip now checks on itself. Every job already recorded
when it last finished: `last_check` per mailbox, `sync_status.last_at`, `FeedMeta.synced_at`. A slow
timer asks whether any of them has been silent for far longer than its own interval, writes one line
to the log, and starts it again.

The judgement lives in `core::health` rather than next to the timers, because it is the part worth
testing and it has no business knowing about windows. A job that has *never* finished is judged from
when Pip started, which is precisely the shape of the §24 bug — a sync with no `last_ok` to be late
from. That case is now a test.

Two things keep it from crying wolf, which would be worse than not having it:

- The allowance is three rounds and never under ten minutes, so one slow network round says nothing.
- Email is not judged while the PC is locked or you are away, because pausing then is deliberate,
  and at night it is measured against the quiet-hours interval rather than the daytime one.
  Otherwise every lunch break and every night would have been reported as a stoppage.

A stoppage is logged once and forgotten when the job recovers, so a long outage leaves one line
rather than one every five minutes.

Proven by running it: a mailbox whose last check was three hours old, against a six-minute interval.
Five minutes after launch, to the second:

```
2026-10-06 12:13:53  started in 39 ms (v0.1.0)
2026-10-06 12:18:53  email had gone quiet; starting it again
```

Worth saying plainly: recovering costs one needless round at worst. Going quiet for a day costs
trust, and trust is the whole product.

## 26. Birthdays on the phone, 2026-10-06

The dates were already on the phone — `SyncDoc` has carried them all along — so this was about
showing them and announcing them, not moving data.

The announcement is deliberately not an alarm. A reminder you set for 08:00 is a thing you asked to
be made to do; a birthday is a thing worth knowing. So dates go to their own notification channel at
`IMPORTANCE_DEFAULT`, with no full-screen takeover and no alarm sound, and can be silenced without
silencing the reminders that get you out of bed. The phone still wakes for them, because
`next_alert` now considers date moments alongside reminders — one alarm for the next thing due,
whatever kind it is, and `due` says what it turned out to be.

Acking needed no new plumbing, which is the nicest part. Dates are acked into `state.reminders`
under `date_state_key`, and `progress.acked` already carries that map with a max-merge. "Got it" on
the phone therefore reaches the PC through the ordinary sync, and the same birthday is not waiting
on the other device.

The wording comes from `messages::date_reminder` and `ImportantDate::describe` — the PC's own — so
the two can never disagree about whose birthday it is or how old they are turning.

Calendar events are still left out, and that is a decision rather than a gap. Pip syncs what only
Pip knows. Birthdays are Pip's own records, held nowhere else; calendar events belong to Google
Calendar, which is already on the phone, already notifies, and does it better. Putting them here
would mean a second ICS fetcher and a second OAuth flow in Kotlin, free to drift from the PC's,
polling on battery so Pip can buzz about a meeting that was just buzzed about.

One wart fixed on the way: a date saved without a name described itself as "'s birthday" on both
devices. It now falls back to the kind — "Birthday" — which is what the one real date on these two
devices needed.
