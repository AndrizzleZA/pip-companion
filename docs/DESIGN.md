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

Still to do in Phase 2: Google Calendar / Microsoft Graph via OAuth (needs app registrations —
decision for the user), tomorrow's all-day events in the summary, a date/time picker for one-off
deadlines further than a year out.
