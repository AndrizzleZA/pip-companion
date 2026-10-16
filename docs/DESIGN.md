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
   computes *when*; the OS scheduler (Android AlarmManager/WorkManager) delivers. No persistent
   service, no wake locks.
7. **Cross-platform without a heavy framework** → shared logic, thin native shells.
8. **Sync without complexity/battery** → local-first; every record has an id + `updated_at`;
   sync is an occasional merge of small documents, not a live connection.

## 4. Architecture decision

**Rust shared core + thin native shell per platform.**

```
 ┌─────────────────────── pip-core (Rust, no OS dependencies) ───────────────────────┐
 │ settings · reminder engine · quiet hours/DND/snooze policy · mood · messages       │
 │ calendar model · email rules · sync merge                                          │
 └──────────────┬─────────────────────────────────────┬───────────────────────────────┘
                │ plain Rust                          │ hand-written JNI → Kotlin (§19)
     ┌──────────▼───────────┐             ┌───────────▼───────────┐
     │ Windows shell (Rust) │             │ Android: Compose app, │
     │ Win32 layered window,│             │ boot receiver,        │
     │ tray, OS events      │             │ AlarmManager          │
     └──────────────────────┘             └───────────────────────┘

     No iOS column: see §33. Not UniFFI either — the bridge is one call (§19).
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
- **Secrets (Phase 3):** `secrets.dat` in the same folder, encrypted with DPAPI under this Windows
  user — refresh tokens, calendar links, the sync key. Never in the readable settings file, and
  never the Credential Manager, which this once claimed and has never used (§37).

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

**Phase 1 — MVP (desktop foundation)** — complete (§8). Procedural creature, layered desktop
window, tray, reminder engine, notification policy, mood, custom-drawn settings, measured targets.

**Phase 2 — Dates & calendars** — complete (§9, §13). Birthdays, anniversaries and deadlines with
lead time; South African public holidays offline; Google Calendar read-only. iCloud and generic
.ics were removed in §14 — Apple offers no OAuth, and app-specific passwords are not one click.

**Phase 3 — Email** — complete (§10, §22). Gmail metadata only — sender, subject, labels, never
contents; local rules; "Always from this sender" in one click.

**Phase 4 — Phone + sync** — complete (§17, §18, §19, §20, §26). Android (Compose, AlarmManager,
boot receiver) over a small JNI bridge to pip-core; local-first sync through one encrypted file in
the user's own Google Drive app folder; joining a device is one Approve tap.
*Not UniFFI* — the bridge is one JNI function, and a hand-written shim was smaller (§19).
*No iOS* — dropped (§33).

**Phase 5 — The companion itself** — complete (§16, §28–§32). Per-user install without an
installer; choosing a character; one drawing shared by every creature; growing up earned through
good days, with an upgrade that never makes an existing companion younger.

**Phase 6 — Ready for someone else** ← current
The gate between "it works on my machine" and "I can hand this to a person who is not me."
- Local crash log — done (§34, §36). Message, source location, version, build stamp, Windows build
  and thread, with the symbols for each build archived so a report can still be read later.
- Security review — done (§37), and its findings fixed in §38 and §39: the tokens in `secrets.dat`,
  the sealed sync key, the encrypted Drive document, and what a stolen state file reveals. Two of
  the smaller findings are still open and named at the end of §37.
- Signed binary — **deferred on cost, not forgotten.** A certificate is a few hundred a year and
  there is no money in a free pet. Unsigned is survivable: SmartScreen warns once, the person clicks
  *More info → Run anyway*, and the README now says so rather than letting them meet it cold. The
  monthly-subscription certificates are worth pricing the day this stops being a hobby — this is the
  one item here that money alone fixes, which is why it is the only thing left open.
- Opt-in update check — **declined; the reasoning is §46 and is not repeated here.** It has already
  been rewritten once when its premise expired, and a summary kept in two places is the defect §51
  is about. The short version is that it waits on the signed binary above.
- Accessibility — keyboard half **done (§45)**: Tab, Shift+Tab, Enter, Space, arrows within a
  control, a visible focus ring and scroll-to-follow, all without a mouse. The screen reader is
  **not** done and is its own milestone, argued in §45: a custom-drawn window is one `HWND` with
  nothing inside it, and a provider that reports an empty window is worse than silence.
- Final polish: more animations — **done (§55, §56, §57)**. Three ways to dig, three ways to bed
  and five things it does while asleep, a dozen more idle behaviours including three that walk,
  and a chooser with weights, cooldowns and a real chance of sitting still. The count is left out
  on purpose: §51 is about numbers restated in four places, and the sections themselves are the
  list. *Personality presets dropped (§46).*
- **Saving stopped at every logon — found and fixed (§54).** A file held for a moment while Windows
  logged in tripped the §39 "cannot read my own files" valve, which then refused to save for that
  entire session, for a week, in silence. The read is now retried, a log line that cannot be written
  is kept instead of dropped, and the warning outlives the moment it appears in.

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

**The bridge is one function.** The roadmap said UniFFI. The surface turned out to be a handful of
calls on JSON objects, so a hand-written JNI shim (`app-android/rust`) does the same job without a
code generator in the build, and `pip-core` stays free of binding dependencies.
`Native.ask("merge", …)` in, JSON out, failures as `{"error": "…"}` so the app can show a sentence
instead of a stack trace. Byte strings travel as hex: a few kilobytes, no dependency either side.

*This said "eight calls" in four places (§50). It was eight at Phase 4 and is twenty-eight now, and
the number was never the point — the point is that it is **one** JNI function with a command name,
so adding a call costs a match arm rather than a binding. The count is left out deliberately: a
number that has to be maintained in four places will be wrong in four places.*

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

## 27. Where an ack belongs, 2026-10-06

Proving the "Got it" button on a real phone turned up a bug the tests had not: the ack reached the
PC and was filed in the wrong place.

`apply` decided where an ack went by whether this device already had an entry for that key:

```rust
if let Some(r) = state.reminders.get_mut(key) { r.acked = ... } else { state.events_acked... }
```

A birthday dismissed on the phone has no entry on the PC yet — that is the whole point of it
arriving — so it landed in `events_acked`, while the date rules read acks from `state.reminders`.
The PC would therefore have asked about the same birthday again. The claim that dismissing on one
device settles it on the other was written down before it was true.

An ack is now filed by what the key *is*: `date:` prefixed or a known reminder id goes where the
rules look, and anything else is a calendar occurrence. Devices that ran the old code carry misfiled
keys, so applying a round sweeps them out of `events_acked` as well — both devices came back with an
empty map and five correctly filed entries.

Two smaller things fixed alongside, both found by looking at real data rather than by reasoning:

- `progress.acked` was never pruned. Deleting a reminder or a date left its mark in the document for
  ever, carried in every round. A tombstone means the record is gone everywhere, so the mark goes
  with it — and nothing can be resurrected by dropping it, because the lists already refuse a
  tombstoned id.
- An ack that outlives its record is exactly how the test date's leftover was noticed, which is a
  fair argument for cleaning up test data properly rather than leaving it and explaining it away.

The lesson is about testing, though. Every part of this was covered: `ack_date` had a test proving
the ack lands in the document, and the document round-trips. What no test covered was the *other*
device reading it back, because both halves were checked against the same assumption. Cross-device
behaviour needs a test that applies one device's document to a second device's state — which is what
the new ones do.

## 28. Stage 1 — choosing your companion, 2026-10-06

Characters and voices, done without adding anything the app has to carry.

A character changes only the crown: a sprout, bunny ears, cat ears or a tuft. Body, animations,
expressions, blinking and every pose are shared, so a new character is a few paths in
`draw_crown`/`draw_ears_behind` and nothing at runtime. Ears are drawn before the body, like the
arms, so the head overlaps their base. The default is Sprout, which is the original drawing
unchanged — nobody's Pip changes under them.

Because the creature is procedural, the choice reaches everywhere for free: the pet, the settings
preview and the tray icon all call the same `draw`, so all three change together. No sprites, no
asset loading, no memory held per character.

A voice is a recipe — a few notes, a decay and how much of the octave above to mix in — fed to the
synthesiser that already made the chime. It is built into memory the first time it plays and kept
until the voice changes, so the cost is one small buffer for the voice in use and nothing on disk.
Picking a voice plays it immediately, whether or not reminder sounds are on: the tap is the request,
and a silent answer would look broken.

Deliberately not done:

- **No custom sound files.** Asked for and declined for this stage: files mean loading, validating
  and failing, for a feature four built-in voices already cover.
- **Characters don't sync to the phone.** The phone doesn't draw Pip, so putting them in the sync
  document would add a field nothing reads. *Reversed twice. §44 put appearance in the document for
  a reason this did not consider — it is the only copy of anything that exists off this machine —
  while noting the phone still did not read it. §50 made that second part false too: the phone
  draws the companion now, from this very field. Both halves of the original reasoning are gone, and
  what replaced them is that the premise "the phone doesn't draw Pip" was never permanent.*
- **No guilt-tripping.** It belongs with hunger in a later stage, and the guiding rules already say
  the pet never guilt-trips — worth deciding deliberately rather than by accident here.

Measured before and after, same method, single runs so small deltas are noise:

| | before | after |
|---|---|---|
| private memory | 1.85 MB | 1.83 MB |
| threads / handles | 1 / 171 | 1 / 170 |
| CPU | 1.80 s/h | 1.56 s/h |
| startup | 20 ms | 23 ms |
| executable | 2046 KB | 2061 KB |

The whole stage costs about fifteen kilobytes of binary and nothing that can be measured at runtime,
which is the point: the architecture already had a procedural creature and an in-memory
synthesiser, so customisation was a matter of parameterising what was there rather than adding to it.

## 29. Stage 1b — different creatures, one drawing, 2026-10-06

Stage 1 gave four crowns on one body. This gives four bodies, without a second copy of `draw`.

A body is eight numbers — top, bottom, half-width, where it is widest, and how far the four
controls reach — and those numbers reproduce the original outline exactly. That is the whole trick:
the hand-drawn shape was already a symmetric blob, so it could become data without being redrawn.
Everything that hangs off it (eyes, mouth, belly, shine, blush, shoulders, feet, shadow, crown,
accessory anchor) is a field in the same plan, and `draw` reads the plan.

The crown is a field too, so bodies and crowns multiply rather than add: a future creature is one
more entry in `shape_of`, any body with any crown, and no new drawing code at all.

Sprout is the original to the point, and the default — nobody's companion changes under them.

**The customisation stays one list, not two.** Bodies and crowns are independent inside, but the
settings page offers four named creatures rather than a Shape row and a Crown row. Two rows would
double the decisions, allow combinations nobody curated, and turn "choose your companion" into a
character creator. Simplicity is the higher priority, and the internal split means a curated fifth
creature costs nothing.

Honest notes on the fiddly parts:

- The mouth was the worst of it, as expected: five variants, all absolute, around (45–55, 69–78).
  Rather than restate five sets of curves per creature, the art stays authored once against
  `MOUTH_BASE` and a body moves it by shifting the transform. Sprout's offset is exactly zero.
- `draw_accessory` did have its own assumptions. `Acc::Sip` interpolates into the mouth, so it now
  reads the face anchor; badges hover beside the head, so they move with it; sparkles ring the body,
  so they spread with its width.
- Eye *size* is still shared. On Kit's wide face slightly larger eyes would read better, but
  keeping one size keeps the family resemblance, and it is one field away if that changes.

A method note worth keeping: comparing screenshots by hash was useless here. Three captures of one
unchanged build gave two different hashes, because the pet blinks. The real check is a test that
compares the generated path to the hand-written one segment by segment, which is deterministic, runs
in milliseconds, and fails loudly if anyone nudges a control point.

Measured, same method as Stage 1: private memory 1.81 MB unchanged, threads 1, handles 170, CPU
2.10 to 1.99 s/h, executable 2061 to 2065 KB. Four kilobytes for three new creatures.

## 30. Stage 2 — growing up, 2026-10-06

A companion that arrives small and fills out over a fortnight, for one kilobyte.

Age needs no new state: `created_at` has been in `State` since the first version, so age is
arithmetic on a number already saved. No timer, no counter, nothing to keep in step — the drawing
simply asks how old it is each time it runs.

**Growth happens inside a fixed window, about the feet.** The tempting implementation is to scale
the window, since the pet is already drawn at a size factor — but the saved anchor is the box's
*top-left*, so a growing box would drag the companion downward a little every day and re-clamp it
against the taskbar. Instead the growth multiplies the pose's own squash/stretch, which is already
pinned to the feet. The window never changes size, a saved desktop position never drifts, and the
companion grows out of the same patch of ground it has always stood on.

Two cues, because size alone reads as *small* rather than *young*:

- Overall scale from 0.84 to 1.0.
- Eyes proportionally larger when young, so a small face is a young face.

**Fully grown after a fortnight**, and the curve is quick at first — three days shows more change
than the week that follows. That is deliberate: growth too slow to notice is growth nobody sees. It
also means every companion older than two weeks is already an adult, so nothing changes under anyone
who has had one a while. The one on this machine was 15 days old when this was written, and is
unaffected; that case is a test.

Deliberately not done:

- **Behaviour doesn't change with age.** Liveliness is the owner's setting, and quietly overriding
  it because the companion is young would be the app arguing with them.
- **The tray icon stays an adult.** At sixteen pixels the difference is two, and it would mean
  threading age through four more functions for nothing.
- **Age doesn't sync.** `created_at` isn't in the sync document, so age is per-device. The phone
  doesn't draw the companion, so there is nothing for it to be wrong about yet; worth revisiting if
  that changes.

The age is shown as a read-only row on the Companion page — "3 days old, still growing" — and the
portrait in the sidebar is as grown as the companion actually is, rather than an idealised one.

Measured, same method: private memory 1.82 MB and working set 1.68 MB both unchanged, threads 1,
handles 170, executable 2065 to 2066 KB.

## 31. Growing up is earned, not waited for, 2026-10-06

§30 grew a companion by age, which was wrong in a way that only becomes obvious once it is running:
a computer left switched off for a fortnight produced a fully grown companion that had never been
looked at. Age measures the calendar, not the relationship. It is gone — no hybrid.

What counts now is **good days**: at most one per calendar day, earned by meeting the water goal or
by any moment together — a pat, a reminder finished, a glass logged. Either is enough, so someone
with water switched off still grows. The count never decreases, and a day that earned nothing is
never mentioned. There is no streak to break here.

Counting happens in `roll_day`, where the streak is already decided, so the day boundary stays the
only place that knows about calendars. Interaction is recorded as a flag set by `bump_affection`
and read once when the day turns over, rather than by comparing the last interaction's date —
which would have meant threading a timezone through `roll_day`, `log_drink` and the phone bridge to
answer a question the flag answers exactly.

It merges like `best_streak`: by taking the higher. Growing up on one device is growing up on both,
and nothing can take it away.

**Three stages, at 0, 15 and 45 good days, and the change at each has to be legible.** The old
0.84–1.00 size range was too subtle — a creature a hundred pixels tall losing a sixth of its height
reads as "drawn slightly smaller", not as "younger". So four things move together:

| | scale | eyes | crown | squat |
|---|---|---|---|---|
| just arrived | 0.78 | ×1.22 | 0.40 | 1.08 |
| growing up | 0.90 | ×1.10 | 0.72 | 1.03 |
| fully grown | 1.00 | ×1.00 | 1.00 | 1.00 |

The crown carries most of it, because it is each character's silhouette: a sprout reduced to a stub,
bunny ears shrunk to nubs. The eyes carry the rest — big eyes in a small face is what everyone reads
as young without being told. The squat makes a youngster wider and shorter, which is the difference
between a small adult and a baby. Reaching a stage is said once and celebrated; falling short of one
is never mentioned.

An honest reservation about the thresholds rather than the count: three stages is right — two gives
one moment and then nothing, four would make each step too small to see — but 45 good days is at
minimum 45 calendar days, and realistically two or three months. Many companions will never be seen
fully grown. 0/10/30 would put the last stage inside a month for someone using it daily, and is a
one-line change if that reads better in practice.

**Settled on 2026-10-08: they stay at 15 and 45**, and the reservation above was wrong about the
arithmetic. It assumed a good day would be rare. The first real companion earned sixteen of them in
sixteen calendar days — one every day, because meeting the water goal *or* any moment together
counts, and a day with neither is rare for someone who has the thing on their desktop. Fully grown
is about a month away at that rate, not two or three.

There is also a convergence worth keeping now that characters arrive (§43): Moth arrives at 45 and
the last stage lands at 45, so the final character turns up on the same day the companion finishes
growing. Under 0/10/30 that splits — fully grown at 30, with something still to come at 45 — which
makes the arrival read as an afterthought rather than as the end of the arc.

## 32. The upgrade must not make anyone's companion younger, 2026-10-06

§31 was correct and still shipped a regression: a state file written before it has no experience, so
on first launch a companion that had been around for weeks reappeared as a newborn and visibly
shrank. Measured on a real file — fifteen days of history, affection 75 — it dropped from roughly
0.93 scale to 0.78, with a stub where its tuft had been. Correct by the new rule, and obviously
wrong to the person looking at it.

State files now carry `VERSION`, and `migrate` runs once on load, crediting
`water.history.len()` good days. The history is the evidence: an entry exists only for a day the app
actually ran and rolled the day over, so it measures presence rather than calendar time, which is
the distinction §31 was built on. Three properties make it safe to run on every load: it takes the
`max`, so a file that already has experience never loses any; `HISTORY_DAYS` is 30, below `GROWN`,
so no one is handed a fully grown companion they did not earn; and the version check makes a second
run a no-op.

Migration happens in `load_state`, which then writes the file back — once, guarded by the version
check. This is not where saving normally belongs: `flush_save` only writes when `state_dirty` is
set, and loading deliberately does not set it. But nothing about an upgrade marks the state dirty,
so the migrated file would have sat unwritten until some unrelated change happened to save it, and
the migration would have silently re-run on every launch in the meantime. It is harmless when it
does — it is idempotent — but a migration that never lands is not a migration.

Backfilling from the days that *met the water goal* was the obvious alternative and is wrong: two of
those fifteen days met it, which would still have shown a newborn. Grandfathering from `created_at`
is worse — it is the age rule §31 deleted, let back in through the migration.

## 34. What Pip leaves behind when it dies, 2026-10-07

*(§33, written later, records why iOS was dropped.)*

Phase 6 begins here because every other item in it assumes you can find out what went wrong on a
machine you cannot see. There was already a panic hook — it logged `panic: {info}` — and it had two
problems. It called `Storage::open()` from inside the hook, so a panic raised anywhere in storage
would have panicked a second time on the way out and aborted with nothing written at all. And what
it wrote was a single line with no version, no Windows build, no thread and no stack: enough to
know that Pip died, never enough to know why.

Everything the hook needs is now resolved at startup, while the program still works — the log path
and the Windows build string — and the hook only formats and appends. The Windows build comes from
the registry, not `GetVersionEx`, which reports 6.2 for everything after 8.1 unless the executable
carries a compatibility manifest.

**The backtrace is the part that needed measuring rather than assuming.** MSVC keeps debug symbols
in a separate `pip.pdb`, so `debug = 1` costs the shipped executable nothing — 2 118 144 bytes with,
2 118 656 without, on identical code. But the `.pdb` is 11 MB against Pip's 2 MB, so it is archived
per release, never shipped; and on a machine without it every frame of ours resolves to `<unknown>`
— not even an address. A stranger's crash report would have been thirty lines of `<unknown>` burying
the one line that helps. So the trace is printed only when it names Pip's own functions, with the
six frames of panic machinery dropped, and otherwise replaced by a sentence saying why there is
none. What survives everywhere is the message, the exact source location, the version, the Windows
build and the thread — six lines, each of which earns its place.

**An honest limit:** a crash on someone else's machine still arrives without a stack. If the
location ever proves not to be enough, the next step is recording raw frame addresses and the module
base, which the archived `.pdb` can resolve offline — more work, and not worth it until a real
report proves the need.

The mail thread's `catch_unwind` went at the same time. Its comment promised that a panic there
would be contained so the next check could go ahead, but release builds abort on panic, so it could
only ever have caught something in a debug build — the one place a crash does least harm. It read
as a safety net and shipped to nobody as one.

A crash is also the one path that cannot be reached by using the app, so `--cmd crash` panics on
purpose. It is behind the devtools feature and absent from the shipped binary, which was checked
rather than assumed.

## 35. A promise a stranger can check, 2026-10-07

The About page said: *"Everything stays on this computer: no account, no network access, no tracking."*
That was true in Phase 1 and has not been since Phase 2. It sat one click away from a Connections
page offering Gmail, Google Calendar and sync through Drive — so the first curious person to open
both would have caught Pip in a plain untruth about the one subject where being caught costs most.
Nothing was leaking; the words were simply left behind by the features that came after them.

What is actually true does not fit in a fixed sentence, because it depends on what the person has
switched on. So the claim is assembled from the live settings. Always: no account of its own, no
server behind it, no tracking — all three still exactly true, and worth keeping, because they are
what people are really asking. Then, only for what is connected: mail read as sender, subject and
labels and never the message; the calendar read and never written; one encrypted file passing
through the person's own Google Drive. Connect nothing and the page says nothing about Google,
which is also honest, rather than warning about traffic that will never happen.

Checked against the binary rather than memory: every host Pip can reach belongs to Google or is a
test fixture, and there is no telemetry of any kind. Both states were rendered — nothing connected,
and a Gmail account connected — and read back from the screenshots.

## 36. Symbols kept for a build that has gone out, 2026-10-07

§34 said the `.pdb` was archived per release. Nothing archived it. `target\release\pip.pdb` is
overwritten by the next build, and a `.pdb` only matches the exact binary it was built with, so
every build handed to anyone was already unreadable by the time the next `cargo build` finished.

Archiving alone would not have fixed it, because nothing identified a build. The version was 0.1.0
from the first commit until 0.2.0 on 2026-10-16, so every crash report named the same thing and a
shelf of symbol files would have had nothing to key them by — and a version number that moves twice
a year would not have helped either, which is the point: the stamp is the commit, not the version. So builds are now stamped:
`build.rs` resolves the commit, bakes it in as `PIP_BUILD`, and it appears on the first line of
every crash report and in the startup line of the log. `tools/archive-symbols.ps1` builds release
with that stamp and copies the executable, its symbols and a manifest — commit, subject, date, size
and SHA-256 of the exe — into a folder named after the commit.

The archive sits outside the repository. It is binary, it is eleven megabytes a build, and nothing
in git should be able to remove it. It goes to the repo drive's root rather than
alongside the repo, because the folder holding this project refuses to have anything new created
in it - which is also why the first attempt to archive quietly produced nothing - and because that
drive is the one with room to spare. A dirty tree is refused rather than archived, because a commit
that does not describe what was built is worse than no record at all: it looks authoritative and
sends you to the wrong source. `-Force` archives it as `<commit>-dirty` for when that is what you
actually want.

**What this does not yet do.** §34 is still true: on a machine without the `.pdb`, every frame of
ours comes back `<unknown>` rather than an address, so an archived `.pdb` cannot decode a stranger's
report. What the archive buys today is the ability to take a report's build stamp, recover the exact
binary and symbols, and reproduce or debug against them — rather than guessing which of a month of
builds the person was running. Making a stranger's stack readable needs raw frame addresses and the
module base in the report, which `std::backtrace` does not expose on stable; that means taking on
the `backtrace` crate, and it should wait until a real report proves the location line insufficient.

## 37. Security review, 2026-10-07

The Phase 6 item. Covered: where the Google tokens live, the sync key and how it reaches a second
device, the encrypted Drive document, and what a stolen state file would give away. Read against the
code and checked against the running install, not from memory.

### What holds up

**The scopes are minimal, and they are what actually enforces the privacy claim.** Gmail is
`gmail.metadata`, which cannot return a message body at all — the promise that Pip never reads your
mail is kept by Google's API, not by Pip's good manners. Calendar is `calendar.readonly`. Drive is
`drive.appdata`, which reaches Pip's own folder and no other file the person owns.

**The sign-in flow is correct.** PKCE with S256 over a 32-byte verifier, a 16-byte `state` checked
before either a code or an error is accepted, a loopback redirect on an ephemeral port, https
enforced (plain http only for a local mock, and only in devtools builds), and an authority
containing `@` rejected so a crafted link cannot disguise its host. WinHTTP is called with
`WINHTTP_FLAG_SECURE` and nothing anywhere relaxes certificate validation.

**The sync document is properly sealed.** XChaCha20-Poly1305, a fresh 24-byte random nonce stored
alongside, `pip-sync-v1` as associated data, and tampering or a wrong key is an error rather than
garbage. Adding a device is X25519 with the shared secret hashed under a domain separator — not a
short typed code, which is the usual weak point and is absent here.

**Nothing secret reaches the log.** Every line written is a fixed string, a file name, or Google's
own OAuth error code and description. No token, address or subject appears, including on the crash
path added in §34.

### What should change

**1. The random number generator's failure is silent.** `random_bytes` ignores the status from
`BCryptGenRandom` and returns its zero-filled buffer if the call fails. That buffer becomes the sync
key, the nonces, the X25519 secrets and the PKCE verifier. `sha256` ignores `BCryptHash` the same
way. The system-preferred RNG failing is close to unthinkable, which is exactly why nobody would
notice it: the result is not an error but a key of all zeros. These must fail loudly.

**2. A low-order public key is not rejected.** `join::shared` calls `diffie_hellman` and hashes the
result without asking `was_contributory()`. A join request carrying a low-order point makes the
approving device derive a wrap key the attacker can predict, so the sealed grant — which contains
the sync key — can be opened by them. It is a chained attack: it needs write access to the person's
Drive app folder *and* the person tapping Approve on a device they do not recognise. The check is
one line and should be there regardless.

**3. What a stolen state file gives away is more than it looks.** `state.json` holds `mail_pending`:
sender names, sender addresses and subject lines of important mail not yet acknowledged — three
entries on the live install while writing this. `settings.json` holds reminder titles and the
connected account's address. Both are plaintext. The copy that travels to Drive is encrypted; the
copy sitting on the disk is not, which is the wrong way round, since the disk is where it stays.

**4. On this machine that file is not even hard to reach.** `%APPDATA%\Pip` inherits an ACL from
`AppData\Roaming` that grants read access to a group of local accounts belonging to a sandboxing
tool installed on the machine — two ordinary user accounts, not administrators. `secrets.dat`
resists them, being DPAPI-bound to the signed-in user, but the plaintext above does not. Nothing
exotic put them there and the point is not about that one tool: developer tooling, backup agents and
management software all add accounts to a user profile, and a file is only as private as the
permissions it inherited. Pip did not cause this and cannot assume it is unusual; writing its own
folder with inheritance disabled and only this user on it closes it wherever it happens.

**5. DPAPI is as strong as the account, and the docs oversell it.** Anything running as this Windows
user can call `CryptUnprotectData` with the entropy constant compiled into the binary and read every
token. That is inherent to DPAPI and the right trade for an app with no password of its own — but
§4 claims "Windows Credential Manager (DPAPI), never in our files", and the Credential Manager is
not used anywhere. Everything is in `secrets.dat`. The two are about equally strong, so this is a
documentation defect rather than a weakness, but it is the kind that makes a reader doubt the rest.
The UI's own wording — "stored encrypted for your Windows account on this PC" — is accurate.

**6. Smaller things.** Decrypted plaintext is copied into a `Vec` and dropped without being wiped,
so secrets linger in freed heap. The `WM_COPYDATA` command channel is unauthenticated, though a
shipping build only answers `show` and `quit`, which any process in the session could achieve
anyway. `join::accept` does not check that a grant came from a device that really held the key, so a
forged grant costs a wrong key rather than a disclosure. And `join::shared` hashes only the shared
point, not both public keys, which is the usual hardening against unknown key-share.

### Not covered

The Android side, token revocation on disconnect, and the update mechanism, which does not exist
yet. Each wants its own pass.

## 38. Three fixes from the review, and the one that went wrong, 2026-10-07

Findings 1, 2 and 3–4 of §37.

**The random number generator fails loudly now.** `random_bytes` and `sha256` check the status they
were discarding. A key of all zeros is not a degraded outcome, it is a catastrophic one, and the
only thing that made it survivable before was that `BCryptGenRandom` never fails — which is an
argument for checking, not against.

**Low-order public keys are refused.** `join::shared` asks `was_contributory()` and returns
`None` if the shared secret collapsed to zero. Seven points are pinned in tests: zero, one, the two
of order eight, and p−1, p, p+1. Writing those tests found that a first attempt used `[1; 32]` —
every byte one — which is an ordinary full-order point, correctly accepted. The test was wrong, not
the code, and a vaguer test would have passed while proving nothing.

**The data folder is taken off every other account.** Inheritance off; full control to this user,
SYSTEM and the administrators by well-known SID. Once per installation, marked by a file.

### What that third fix did the first time

It ran `icacls` with `/T`, intending "and everything inside". `/T` applied `/inheritance:r` to each
file as well, and a file whose permissions came *only* from inheritance was left with an empty list
— reachable by nobody. Pip, running as the user who had just lost access, then read nothing, found
nothing, and did exactly what it is built to do when there is nothing to find: it started fresh and
saved a new empty `state.json` and `settings.json` over the real ones.

Recovered: the state from the previous night's backup, and the reminders, dates and preferences from
the encrypted Drive document, which is what that document is for. Not recovered: the Gmail account
and the calendar connection, because `SyncDoc` carries prefs, reminders and dates and does not carry
connections; and the companion's character, colour, size and voice, because `Prefs` carries
`pet_name` and none of the rest. A second mistake during recovery is worth recording too: a
PowerShell `Set-Content -Encoding utf8` wrote a BOM, `serde_json` refused the file, and Pip
quarantined it as `.corrupt` and reset again — the quarantine is why nothing was lost that time.

Three things this says, in order of how much they matter.

**Pip resets rather than refuses.** A file it cannot read is treated exactly like a file that is not
there, and the difference between "this user has no data" and "this user's data is unreadable" is
the difference between a correct fresh start and silent destruction. `load` already tells them
apart — it renames an unparseable file to `.corrupt` and says so in the log — but a permission
error takes the `Err(_)` arm and is indistinguishable from absence. That is the real defect here and
it is not yet fixed.

**A change that cannot be undone should not be made on a hunch.** The ACL was altered on a live
install with no snapshot of what it had been, so the repair was `/reset`, which also reset the
folder and undid the hardening. It is still undone.

**And the blast radius was not thought through.** The fix was tested on an empty folder, where it
works, and shipped to a folder with eight files in it, where it does not. Pre-existing files were
the entire point of the change — the inherited permissions being removed were on files that already
existed.

## 39. A file that will not open is not a file that is not there, 2026-10-07

The defect behind §38. `load` read a file, and anything that went wrong on the way — any `Err` at
all — became "no data, use defaults", which the caller then saved. For a first run that is exactly
right. For a file that exists and will not open, it is the destructive answer to a recoverable
problem, and it is what turned a bad permission change into lost settings.

The two cases are now separated at the only place that can tell them apart. `ErrorKind::NotFound`
is an ordinary fresh start. Every other error — no permission, a lock, a failing disk — sets a flag
that turns off saving entirely, for every file, until Pip is restarted and can read them again.
Nothing in memory is trustworthy in that state: it is defaults standing in for data that is still on
the disk, and writing it is the only way to actually lose anything.

Unparseable is a third case and keeps its old behaviour: the file is readable, so it can be moved
aside as `.corrupt` and reported, and starting fresh after that loses nothing that was not already
broken.

The person is told, in the one place they will see it — a bubble, before anything else, because a
blocked load looks exactly like a first run from the inside and greeting someone as new while their
data sits unreadable on the disk is the most misleading thing Pip could do. The wording says the
data is still there, that nothing is being saved over it, and that they have not broken anything,
because the harmful response would be to set everything up again on top of it.

Three tests hold the distinction: a missing file still starts fresh and still saves; a file that
will not open stops every save and leaves the file exactly as it was; nonsense in a readable file is
still quarantined and does not lock saving. They force the unreadable case with a directory standing
where the file belongs, which is the portable way to get an error that is not `NotFound`. It was
also checked for real, with a file denied by ACL — the §38 scenario exactly — where the old build
would have overwritten both files and the new one leaves them untouched and says why.

One thing that caught: the message first used an em dash, which the bubble font has no glyph for, so
it rendered as `(2014)`. It was the first em dash in `messages.rs` — found by looking at the bubble
rather than at the string.

## 40. Tuft stops being a recoloured Sprout, 2026-10-07

Four creatures, and two of them were the same drawing. As height against width, Sprout was 66×70 =
0.94 and Tuft 61×62 = 0.98 — both round, within a few percent of each other — while Bunny sat at
1.32 and Kit at 0.64. Their faces agreed too: Sprout's eyes at 61.0 and 11.0 apart, Tuft's at 60.0
and 10.5. Only the crown really differed, so Tuft read as Pip with different hair.

Slimming it was the obvious move and the wrong one on its own, because Bunny is already the slim
one; it would have traded a Sprout collision for a Bunny collision. What none of the four had was a
silhouette that is not symmetrical top to bottom: every one of them had shoulders and hips within a
point of each other — 22/21, 17/17, 27/26, 20/19 — so nothing was bottom-heavy. Tuft is now
13/19, with the widest point dropped to 70. Narrow at the shoulder, full at the hip, widest low: a
pear, which reads as sitting into the ground rather than standing up.

The face follows from the body by the ratios the other four already keep — `mouth_y = eye_y + 10`,
`eye_dx = half × 0.33`, `arm.x = half × 0.83`, `shadow_rx = half × 0.72` and so on — so the
proportions stay in the family even though the outline no longer does.

**Honestly, the proportion alone does not separate it from Bunny:** 1.25 against Bunny's 1.32 is
nearly the same build. What separates them is the taper, which is why the shoulder-to-hip number is
the one doing the work here, not the slimming. Rendered side by side at all three stages, Sprout is
now unmistakably a different creature and Bunny reads as a tall capsule against Tuft's teardrop.

A stronger taper was tried — shoulders in to 11, widest down to 72 — and reverted: at the size a
companion is actually drawn, it was indistinguishable from this, and a change nobody can see is not
worth the risk of a pinched head.

**This changes a companion under the person using it.** The rule until now has been that nobody's
companion changes under them — §31 went to some trouble so that growing up could never take
something away. This breaks that rule knowingly, because the alternative is leaving two of the four
characters as the same drawing, and it was asked for by the person it happens to. It should not
become a habit: once anyone else is using Pip, reshaping a character they chose needs a better
answer than this one.

## 41. Two more creatures, and the row that had to wrap, 2026-10-07

A character is a `Body` — eight numbers — and a `Crown`, and the bodies are symmetric blobs, so the
body only sets proportions and the crown is what makes it a different animal. New characters
therefore need new crowns or they arrive as siblings of the ones already here, which is exactly how
Tuft spent its first weeks being a recoloured Sprout (§40).

**Bloom** is a small round body under a ring of six petals with a dot of pollen, and it is the only
crown in the cast that is not green or ink: colour alone separates it across a desktop. The first
version sat the ring at the crown point and it read as a flower *badge* stuck on a forehead — the
petals have to be wider than the head they sit on, and sit above it, or the body is just round. The
shadow is wider than `half × 0.72` for the same reason: the petals overhang.

**Moth** is the smallest of the six, with two jointed stalks and a knob on each tip. The knobs are
load-bearing. Crowns scale to 0.40 at the first stage, which takes a 2.8-wide stalk under a pixel —
a stroke that thin greys out to nothing, while a filled knob only gets smaller. Both crowns are
drawn as fills rather than outlines for that reason, and both were checked as babies rather than
assumed: Bloom's flower becomes a bud and Moth's antennae become two dark specks, which is the same
language as Sprout's stub and Tuft's three nubs.

**An honest limit.** Bloom and Moth have nearly the same body — 53×54 against 49×48, both round —
and are told apart entirely by their crowns. That is within how this is built, and at a glance a
pink flower and a pair of dark antennae are not confusable, but it does mean the cast now has one
body shape doing double duty. The next character to arrive should take a proportion none of them
have rather than a sixth round one.

**The settings row.** One segmented control is sized by its own labels, so the characters were going
to run back over their description: six on a line is 309 wide and would start at x=415, while the
hint text runs to x=432. They wrap four to a line now, each line its own right-aligned control, and
a line not holding the current character simply shows nothing selected — which is what an
out-of-range index already did. A test pins this at 100%, 125% and 150%: every width here is a
constant times `s` and the window itself is created at `LOGICAL_W * s`, so fitting at one scale is
fitting at all of them, but that argument only holds while it holds.

**An unknown character no longer costs every setting.** `Character` derived `Deserialize`, so a name
this build does not know failed the whole file — and `load` moves an unparseable file aside as
`.corrupt` and starts from defaults, so one unrecognised word took someone's reminders and quiet
hours with it. It now reads as Sprout. The cost is that the unknown name is then saved as "sprout",
so going back to a newer build will not restore the choice; keeping the raw string is the better
answer the day more than one version is in the wild.

## 42. Kit becomes a mushroom, 2026-10-07

Kit was the wide, low one with pointed ears. It is now a stem under a cap: the body narrowed from
half 43 to half 20 and its widest point dropped almost to the ground, so the silhouette flares at
the foot and the width now comes from the crown instead. The shadow is 32 against the 14 the ratio
would give, because the cap overhangs the stem by more than two to one and a shadow the width of
the stem would look like the cap was floating.

The cap is flat underneath rather than a full ellipse. A dome on a stem is a lollipop; the single
line that makes it a mushroom is the straight edge where cap meets stem, lifted very slightly in
the middle so the stem does not appear to spear it. Three lighter spots, kept because at the size a
companion is drawn they read as a toadstool rather than as noise.

**Caps get a shrink floor and nothing else does.** Crowns scale to 0.40 at the first stage, and a
cap at 0.40 is narrower than the stem it sits on: rendered, a baby read as a blob in a red beanie,
not as a young mushroom. Every other crown is narrower than its own head, so shrinking only makes
it look younger — which is the whole point of §31. The cap is the one crown wider than its body, so
the floor belongs to the cap rather than to the growth table the whole cast shares.

`Crown::CatEars` stays in the enum with nothing wearing it. Crowns are a library that bodies
multiply with, and the next creature that wants pointed ears should find them drawn already.

**The variant is still `Character::Kit`.** Only `label()` changed, to "Toadstool". The name
serialises to "kit", and until today an unrecognised value failed the whole settings file — §41
softened that, but renaming the variant would still have silently turned every existing Kit into a
Sprout on first load. The identity people chose is the serialised name, not the label.

## 43. The seventh character, and a cast that arrives, 2026-10-07

**Pebble** completes the set, and it only became possible once Kit stopped being wide (§42). It is
the broad, low, bald one: `Crown::None` draws nothing at all, so the outline *is* the character and
it has to be the widest thing here or there is nothing to recognise. It took the exact body the
mushroom gave up, which is why it could not be written until that landed.

One honest consequence: Pebble has the least legible growing-up of the seven. §31 built four cues —
scale, eyes, crown, squat — and said plainly that size alone was too easy to miss. A creature with
no crown is down to three, and the one carrying most of the legibility is the one it does not have.
It still reads younger as a baby; it just says it more quietly than the others, and that is the
price of a character defined by absence.

### Arrivals, not unlocks

The three new characters are not there on the first day. They turn up at 7, 21 and 45 good days.

This is the kind of feature that usually arrives as a grid of greyed-out skins with "22 more days!"
under each one, and this project has refused that four separate times: §7 says the pet never
guilt-trips, §28 declined it deliberately rather than by accident, and §31 made growing up
something that can only ever be good news. So it is built as a gift and the refusals are the design:

- **Nothing that has not arrived is drawn.** `Character::available` filters the list and the
  settings row simply has four entries on the first day. There is no locked state, no greyed chip,
  no tooltip. It is also the cheapest possible implementation, which is usually what honesty costs.
- **Nothing counts.** No "next one in N days", no progress bar, nowhere to go and feel behind. The
  only number anywhere is the one that was already there, and it only ever says what has happened.
- **It speaks once, on the day.** `welcome_arrivals` is the shape of `celebrate_growth`: compare
  before and after, say it once, and have no counterpart for "not yet". It writes the state as it
  speaks, because "once" is only true if the day that caused it is saved — found by rendering it,
  where the second launch announced Pebble a second time.
- **Nothing that was ever free becomes conditional.** The four originals and all seven colours are
  unconditional on day one and every day after. Gating something already chosen would take a
  companion away from the person who chose it, which is what §31 exists to prevent. A test pins it.
- **A character that has not arrived falls back to Sprout** — a hand-edited file, a restore, a
  device further along — and the choice stays in the file rather than being rewritten, so it comes
  back by itself on the day it arrives. `experience` already merges across devices by taking the
  higher, so two devices cannot disagree about what has turned up.

## 44. A restore should give back a face, 2026-10-07

When the data folder was destroyed (§38), the encrypted Drive document gave back reminders, dates
and preferences, and nothing about the companion itself. The person got their reminders and a
stranger's face: `Prefs` carries `pet_name` and not one thing that is actually looked at.

§28 decided deliberately that characters do not sync, because the phone does not draw Pip and the
field would be one nothing reads. That reasoning was sound and it was about sync. It did not
consider backup — and the Drive document is the only copy of anything that exists off the machine.
§39 stopped the overwrite that caused the loss, so this is not about preventing it again; it is
about what is left to restore from when something else goes wrong. §28 now says it was reversed
here rather than being quietly contradicted.

So appearance travels: character, colour, size, voice, liveliness, theme.

**It is its own record, and that is the whole design.** A phone deserialises the document into
these structs and writes the merged result back, so a field its build does not know is dropped on
the way through. Inside `Prefs` — one record, one timestamp — an older phone would have written
prefs back with the appearance fields missing, they would have read as defaults, and the next sync
would have reset the companion to a mint Sprout. The harm this exists to prevent, arriving through
the door it came to lock. As a separate `Option`, an older phone drops it to `None`, and in the
merge **absence never wins**: `None` can fail to carry appearance but can never erase it. The cost
is that while such a phone is in the loop the Drive copy may be written without it, which narrows
the backup rather than corrupting it — and ends the moment the phone is rebuilt against this core.

**A restored character still waits for its arrival.** The choice is taken as given and written to
settings, because holding it back would quietly rewrite what somebody picked. What §43 decides is
what gets *drawn*, and it declines to draw a character that has not arrived and hands it back on the
day it does. So a restore cannot bypass an arrival, and cannot lose a choice either.

### Deliberately not done

- **The `WM_COPYDATA` command channel stays unauthenticated.** Finding 6 of §37 named it; it is
  declined, and the argument is the one the review already made. A shipping build answers exactly
  two verbs, `show` and `quit`, and any process in the session that wanted Pip gone could post
  `WM_CLOSE` or call `TerminateProcess` without asking Pip's permission. Windows already refuses
  the message from a lower integrity level, which is the boundary that matters. Authenticating it
  would mean a shared secret on disk to protect a door that opens onto a corridor anyone can walk.

## 33. No iOS, because of the rent, 2026-10-07

Written late: §6 and §34 have both been pointing at this section for a while.

The original plan had three shells — Windows, Android, iOS — and the architecture diagram carried
an iOS column from the first day. It is gone, and the reason is not technical.

**Shipping an iOS app means paying Apple $99 a year, for as long as the app exists.** Not once, to
publish: every year, to keep it installed on anybody's phone, including the author's. A build
stops working when the certificate lapses. That is a subscription for the privilege of running
software on hardware you already own, and this is a free pet with no revenue and no intention of
having any — so the rent would be paid out of pocket, indefinitely, to keep a second phone app
alive that duplicates what the Android one already does.

Nothing else about iOS was a problem. The core is portable Rust with no OS dependencies; the shell
would have been SwiftUI over the same one-function bridge; local notifications map onto the same
"the core computes *when*, the OS delivers" split as AlarmManager. It would have worked. It simply
costs a recurring fee that nothing here will ever earn back.

What follows from it:

- The architecture diagram in §4 now shows two shells, not three, and no longer names
  `UNUserNotificationCenter` among the schedulers.
- Phase 4's sync is PC and Android. The design never assumed more than two devices, and the join
  handshake does not care how many there are, so nothing was built around the absent third.
- Nothing in the core is iOS-shaped and nothing needs removing. If the fee ever stops mattering,
  this is a new shell against an unchanged core, not a reversal.

The honest summary is that iOS was dropped for a recurring bill, not for a technical reason, and it
is worth the design saying so plainly rather than implying the platform was found wanting.

- **`join::shared` keeps hashing only the shared point, and `join::accept` keeps trusting the
  grant.** The last two of §37's finding 6, declined on 2026-10-08.

  Both defend the step where a new device is let in, and both attacks need the same thing first:
  the ability to write into the person's own Drive app folder. That means already being inside
  their Google account — at which point the mail, the calendar and the sync document's hiding place
  are all available directly, and the sync key is the least of what has been lost. Binding both
  public keys into the hash is the textbook hardening against unknown key-share, but the attack it
  would have to stop here is an attacker substituting their *own* public key into a join request,
  and binding does not prevent that: their key is bound just as honestly as anyone's. Authenticating
  the granter turns a forged grant from "this device adopts a key that does not work" into "this
  device refuses it", which is a denial of service someone could only mount against themselves.

  Against no reachable benefit sits a real cost. Both change what the two sides hash, so a PC and a
  phone that are not rebuilt together can no longer pair at all — and the Android app in the field
  was built against the current handshake. It is surgery on working cryptography to close a door
  that opens onto a corridor the attacker already owns.

  This is not "never". The comfortable part of the argument is that the Google account in question
  belongs to the person running Pip, and that stops being comfortable the moment it is a stranger's.
  Both should be done before anyone else uses this, and the natural moment is the signed binary,
  when a release that updates both ends exists anyway. Recorded here so that the next person to
  read §37 finds the decision rather than an open finding nobody acted on.

## 45. The settings window without a mouse, 2026-10-08

Phase 6, accessibility. Keyboard reach is done; a screen reader is not, and the second half of this
section is the argument for why that is a milestone rather than a task.

### Keyboard reach

Every interactive thing is reachable with Tab and Shift+Tab, in reading order, including the
sidebar. Enter and Space do exactly what clicking does, Left and Right move within a segmented
control, and the view scrolls to follow the focus.

**Focus is held as the action, not as an index.** `hits` is rebuilt from nothing every frame, and
the reminder and date lists grow and shrink while the window is open, so an index would have meant
that adding a reminder quietly moved the focus to a different control. Keyed by `UiAct`, the focus
is on the thing the person is pointing at and survives the rebuild — verified by focusing "+ Add
reminder" and pressing Enter twice: the list below it grew by two and the focus did not move.

**The two character rows behave as one control.** Left and Right walk the whole group rather than
the row, because the group is "every act of the same variant" — `Who(_)` is seven chips that happen
to be drawn on two lines, and `std::mem::discriminant` already knows that without being told. Moving
along a segmented control also chooses as it goes, which is how every other one in Windows behaves.

**The draw loop was not restructured**, which was the thing worth checking before starting. The ring
is painted at the end of the frame from the hit list that already exists, so every control gets one
without knowing it is focusable, including the sidebar and the header button that are drawn over
the scrolled content. It is two strokes, a pale one under an accent one, because a single accent
line vanishes against the accent-coloured controls it has to mark — and it is a ring where hover is
a fill, so the two cannot be mistaken for each other.

Reading order needed one small inversion: `hits` is in draw order, which puts the scrolled content
first and the sidebar and header last because those are painted over the top. The tab order lifts
them back to the front rather than rearranging the drawing to suit the keyboard.

Esc is untouched and still in the window procedure: leave the field first, close the window second.
While a field is being edited the arrows and Space belong to the text; Tab commits and moves on.

### A screen reader is its own milestone

**Not built, deliberately.** A custom-drawn window is a single `HWND` with nothing inside it, so
there is nothing for a screen reader to find and no partial version of this that helps. A provider
that reports a window with no contents is worse than silence, because it looks supported.

What it would actually take:

- Answering `WM_GETOBJECT` is the easy part. Behind it sits `IRawElementProviderSimple`,
  `IRawElementProviderFragment` and `IRawElementProviderFragmentRoot`, each with `IUnknown` — hand-
  written vtables and reference counting, because this project depends on `windows-sys`, which is
  raw bindings with no COM support. The crate that makes implementing COM interfaces bearable is a
  different one, and `Win32_UI_Accessibility` is not enabled here either.
- One provider per control, and there are around a hundred and seventy across seven pages, each
  needing a name, a control type, a bounding rectangle, navigation to its neighbours, a stable
  runtime id, and the right pattern — Invoke, Toggle, SelectionItem, Value or RangeValue.
- Events: focus changes, property changes, and structure changes every time a reminder is added.

The real cost is none of that. It is that UI Automation is a *retained* tree — elements that exist
between frames and can be navigated — and this renderer is deliberately immediate: `hits` is thrown
away and rebuilt sixty times a second. Bridging them means building and maintaining the retained
element model the renderer exists to avoid, and keeping it correct as the drawing changes. That is
a structural addition to the window, not a feature on top of it.

It is also the half that cannot be verified by looking. Keyboard reach can be driven and
photographed; a provider has to be tested against Narrator and Accessibility Insights, with every
name read aloud and judged. "Character, Toadstool, 5 of 7" is the standard, and there are a hundred
and seventy of those sentences to get right.

So it is named here as its own piece of work rather than started. Keyboard reach is the half that
stands alone and helps people who can see the window but cannot use a mouse; the screen reader is
the half that needs a week and a different kind of testing.

## 46. Two things that are not being built, 2026-10-08

Recorded the way §28 and §44 record a refusal: with the argument, so that the next person finds a
decision rather than an idea nobody got round to.

### The opt-in update check

**Declined, and the original reason has since expired — see the update below before reading on.**

The roadmap's reasoning was "once anyone else has it, there must be a way to ship a fix", which is
true and still does not produce a feature, because the premise underneath it is missing:
`.git/config` has no remote. This project exists on one PC. There is no release anywhere for a check
to find, no URL to ask, and no second copy of Pip in the world that could need telling.

An update check pointing at nothing is worse than not having one. It is a network call on every
launch that can only fail, an error state to design and explain, a privacy claim to qualify — the
README would have to stop saying *nothing leaves your machine until you connect something* — and a
promise to the person running it that fixes arrive, when nothing is shipping them.

What has to exist first is somewhere for releases to live and a way to prove a download is the
build it claims to be. That is the same conversation as the signing certificate, and both start
costing money on the same day. When there is a remote and a signed release, this becomes a small
feature: a version file, an opt-in toggle defaulting to off, a check no more than once a day, and
never an automatic download. Until then it is scaffolding for a building nobody has started.

**Updated 2026-10-12: the premise above is gone, and the answer is still no — for a different and
smaller reason.** There is a remote now, and a `v0.1.0` release with `pip.exe` attached, so "nowhere
for a release to live" has stopped being true. Half of what that paragraph asked for exists.

The half that does not is the half that matters. A check tells someone a new version is ready and
sends them to download it; that download is unsigned, so every update walks them through
*"Windows protected your PC" → More info → Run anyway* again. Today they meet that wall once, when
they first install. An update check without a certificate converts a one-time friction into a
recurring one, and teaches the person that clicking past a security warning is a normal part of
using Pip — which is a worse habit to install in someone than an out-of-date pet.

So the condition is now exactly one thing: **a signed binary**. Not a cadence, not a version file,
not a toggle — those are an afternoon. When a release can be downloaded without Windows objecting,
this is worth building, in the shape the paragraph above already describes. Until then the honest
instruction is the one the README already gives: the repository is public, and anyone who wants the
newest Pip can look.

### Personality presets

**Dropped.** It has sat on the roadmap since Phase 1 as two words, and nobody — including the
person who wrote them — has ever said what a preset would contain. Liveliness already exists and is
a real setting with three values. Voice exists. Colour, size and character exist. Whatever
"personality" was meant to add on top of those was never pinned down, and a feature that cannot be
described cannot be built, tested or refused on its merits.

Two words on a list are not a plan, and carrying them from phase to phase makes the roadmap look
like it has work left in it when it does not. If it comes back it should come back as a sentence
about what a companion would do differently — not as a word that sounds like it means something.

## 47. Where this stands, 2026-10-08

Phase 6 is complete except for signing, which money fixes and nothing else does. This section is
the handover: what exists, what was refused and why, and what to do first.

### What exists

A companion that lives on the desktop in one process, under 2 MB of memory and a fifth of a second
of CPU an hour, drawn per-pixel into a layered window with no framework under it. It reminds you
about water and whatever else you ask, keeps birthdays and deadlines, reads the metadata of mail
that matters and your calendar read-only, and syncs to an Android phone through one encrypted file
in your own Google Drive. Seven characters, three of which arrive as the companion grows. Settings
is seven pages, usable without a mouse. A crash leaves a readable report behind, and the symbols for
every build that went out are archived so it can still be read a year later.

Phases 1 to 5 are in §8 through §32. Phase 6 is §34 to §46.

### What was deliberately refused

The refusals are the design as much as the features, and each one is argued where it was made:

- **Guilt-tripping**, four times over: §7 made it a rule, §28 declined it rather than letting it
  arrive by accident, §31 made growing up something that can only be good news, and §43 built
  arrivals as gifts with nothing counting down to them.
- **iOS** (§33) — $99 a year, forever, for a free pet. Not a technical judgement.
- **The two `join::` hardenings** (§44) — both need an attacker already inside the person's Google
  account, and both would break pairing until the PC and phone were rebuilt together. Revisit
  before anyone else uses this.
- **The `WM_COPYDATA` channel** (§44) — a lock on an inside door; anything in the session can
  already close the window.
- **A screen reader** (§45) — its own milestone, because bridging an immediate-mode renderer to a
  retained accessibility tree is a structural addition, and a half-built provider looks supported
  while helping nobody.
- **The update check and personality presets** (§46) — nowhere to publish, and two words nobody ever
  defined.
- **Custom sound files, and ICS/iCloud calendars** (§28, §14) — a feature four voices already cover,
  and a connector Apple makes impossible to do in one click.

### What to do first

1. **Rebuild and install the phone.** It is the only live gap. The phone runs a build from before
   the appearance record, so every sync it performs writes the Drive document back without the
   companion's face in it (§44). Nothing is lost — absence never wins the merge — but the backup is
   narrowed until this is done. `ANDROID_NDK_HOME`, then `cd app-android && ./gradlew assembleDebug`.
   Then make the phone sync and confirm the document still carries appearance.
2. **Price a certificate** if this is ever handed to someone who is not you. Everything else in
   Phase 6 is done; this is the wall every first-time user hits.
3. **Read §37 before changing anything about secrets or sync**, and §38 and §39 before changing
   anything that writes a file. The first is what the security review found; the second two are what
   happened when a fix went wrong, and why `load` now refuses to treat a file it cannot read as a
   file that is not there.

### The rule this project kept

Every section here answers *why*, not *what* — the code says what. Where something was tried and
reverted, the revert is written down with the reason (§40's stronger taper, §41's flower badge,
§42's beanie, §38's whole incident). Where a decision was reversed, the original says so (§28, and
§44 which overturned it). Someone picking this up should keep that habit: a design document that
only records what survived is a list of outcomes, and a list of outcomes cannot be argued with.

## 48. The phone catches up, 2026-10-08

§44 said the cost of an old phone "ends the moment the phone is rebuilt against this core". It was
still running a build from 2026-10-06, so the cost was live: characters, arrivals and the appearance
record all postdate it, and every sync it performed wrote the Drive document back without the
companion's face in it.

Rebuilt and installed over the existing app, so nothing on the phone was lost and the pairing
survived — confirmed rather than assumed: after the update the PC reports `connected=true`,
`waiting=false` and no join requests, so no device had to be approved again. The join handshake is
untouched, which is exactly why that held (§44 declined the two changes that would have broken it).

**Proving it needed an instrument, because the design hides the answer.** Absence never wins the
merge, so a device that drops appearance looks identical to one that carries it: the PC merges its
own copy back in either way and the document ends up correct. The only moment the difference is
visible is between fetching the remote document and merging it, so `SyncResult` now records whether
the fetched document carried an appearance record, and the devtools `sync` command reports it.

Measured across the update, with the PC writing the document first each time:

| | document after the phone wrote it |
|---|---|
| phone build of 2026-10-06 | `remote_appearance=Some(false)` |
| phone build of 2026-10-08 | `remote_appearance=Some(true)` |

The phone really did sync in both rounds rather than failing quietly, which matters because a phone
that never ran would have produced the same "true" as a phone that carried the field: its own
settings now hold the PC's six reminders, including one created on the PC that morning.

One thing the check revealed that is worth writing down: the phone does **not** adopt the
appearance into its own settings, and should not. It holds `sprout`/`mint` while the PC holds
something else, because the phone does not draw Pip — §28's original reasoning, still true. What
changed in §44 was that the field travels *through* it, not that the phone has any use for it.

## 49. A record that merges on a timestamp has to own one, 2026-10-08

§44 gave appearance its own record and then merged it on `prefs_updated_at`. That was wrong, and it
only showed once the phone was rebuilt — the first sync after §48 turned a lavender Toadstool back
into a mint Sprout.

The asymmetry did it. The phone keeps prefs and never draws Pip, so it holds *default* appearance
with a prefs stamp from months of ordinary use: 1 791 292 047. The PC had never edited the prefs
block, so its stamp was 0. Sharing one timestamp meant the phone's defaults were "newer" than a
choice the person had actually made, and the merge did exactly what it was told.

`Settings` now carries `appearance_updated_at`, stamped by the settings window whenever the
character, colour, size, voice, liveliness or theme changes, and the merge takes **strictly** newer
so a tie keeps what the device already has. Two devices that have never touched appearance both sit
at 0 and neither imposes anything on the other; the first real change wins everywhere, which is what
was wanted all along.

Three tests hold it: a device with only a clock cannot overwrite a real choice, a real change still
travels, and an older change never wins.

**What this says about the §44 design.** The separate record was right — it is what stopped an older
phone erasing appearance. What was wrong was borrowing someone else's clock for it, which looked
economical and quietly coupled two records that change for different reasons and on different
devices. A record that merges on a timestamp has to own that timestamp.

**And about how it was found.** Not by reasoning about the merge, which had looked fine twice. By
installing the build and reading the companion's name back off the running app, where it said
`sprout / mint` and should have said `kit / lavender`. The phone rebuild was the first time the two
stamps were ever compared on real devices, and it took about ten seconds to see and far longer to
work out.

## 50. Pip on the phone, 2026-10-12

§28 said characters do not sync because the phone does not draw Pip. The phone draws Pip now.

**This was relocation, not a rewrite, and that was the whole finding.** `render/mod.rs` and
`render/creature.rs` imported `pip_core::settings` and `tiny_skia` and nothing else — no Windows
anywhere — so the creature had been portable for weeks without anyone noticing. It moved into a new
`pip-draw` crate (`pip-core` + `tiny-skia`), which both shells now use. `pip-core` stays free of a
rendering dependency.

What stayed behind is what is genuinely tied to Windows: `text.rs` loads Segoe UI by mapping
`C:\Windows\Fonts` through `CreateFileMappingW`, `ui.rs` is built on it, and `Palette` and
`rgba_to_bgra` are a layered window's concerns. `app-windows/src/render/mod.rs` is now a re-export
of `pip_draw::*` plus those three, so every call site in the desktop still says `crate::render::…`
and not one of them changed.

**One JNI command, in the entry point that was already there.** `draw` takes a character, a colour,
an `experience` and an expression, and returns an RGBA square. It takes `experience` rather than a
stage on purpose: the phone derives the growth stage with the same thresholds and passes the
character through `Character::shown`, so a companion that has not arrived yet (§43) is not drawn on
the phone either. There is no second `Pose`, no second drawing path, and no Kotlin that knows what
a creature looks like.

Pixels travel as base64 rather than the hex the rest of the bridge uses. Hex is right for a 32-byte
key and wrong for a frame: a 288-pixel square is 331 KB, which is 663 KB as hex against 442 KB as
base64, and Android decodes base64 with `android.util.Base64` — so it still costs no dependency on
either side, which was the reason for the hex rule in the first place.

**It renders once per change, not on a timer.** Animation is deliberately not here. §7 says every
feature justifies its cost, and a pet repainting itself sixty times a second inside a list view is
a battery decision wearing a drawing decision's clothes. It wants arguing on its own.

### What the phone actually showed

Verified on the real device, not an emulator. The companion appeared as a lavender Toadstool —
which is itself the §44 appearance record arriving: three days earlier the phone's own settings
still said `sprout`/`mint`, because the phone stores what it is told and had nothing to draw with it.

Showing it at two growth stages took two attempts, and the first failure is the more interesting
one. Setting the phone's `experience` to 0 and relaunching produced an identical picture, because
the app syncs on launch and the merge pulled the value straight back from the PC — `experience`
merges by taking the higher and never decreases (§31). It could only be demonstrated with the phone
offline for one launch. The design defending itself against a deliberate attempt to lower it is a
better result than the screenshot was.

### A number that was wrong in four places

The document claimed the bridge was "eight calls" in four separate sections, including one written
days ago that repeated it. It was eight at Phase 4 and is twenty-eight now. Fixed, and the count is
deliberately not replaced with twenty-eight: the point was always that it is *one* JNI function with
a command name, so a new call costs a match arm. A number maintained in four places will be wrong in
four places — the same defect as §37's finding 5, which is how it was spotted.

## 51. Why the document keeps going stale, 2026-10-12

Three stale-documentation defects in two weeks, each found and fixed on its own:

- **§37, finding 5** — §4 said secrets lived in the Windows Credential Manager. Nothing here has
  ever used it; they are DPAPI-encrypted in `secrets.dat`.
- **§50** — the bridge was "eight calls" in four places. It was eight at Phase 4 and twenty-eight
  when counted.
- **§28, here** — "the phone still does not read it", about the appearance record the phone now
  draws the companion from.

Nobody had asked why it keeps happening, so: they are not the same defect, and only two of the
three were ever preventable.

**They are three different failures.** The first was a plan written down as a fact — §4 described
what Phase 3 was going to do, the implementation chose differently, and the sentence was never
revisited because nothing pointed at it. The second was *duplication*: one fact restated in four
places, which guarantees that fixing it anywhere leaves it wrong somewhere, and which is how it got
written into §33 three days before §50 found it — new prose copied an old sentence that was already
false. The third was simply true when written and made false later by work in a different part of
the system, which is not a mistake at all.

**What is worth doing, and it is small.**

*Prefer a claim that cannot drift.* "The bridge is eight calls" ages; "the bridge is one function"
does not, and it was the actual point — the count was never why the shim beat UniFFI. §50 removed
the number rather than correcting it to twenty-eight, because a number maintained in four places
will be wrong in four places. Where a fact must appear more than once, the second place should
point at the first rather than restate it.

*Where a claim rests on something checkable, pin it with a test.* Two now exist, and they are the
only two that could: `there_is_exactly_one_way_in` asserts the bridge really is one JNI entry
point, and `secrets_are_dpapi_and_not_the_credential_manager` asserts the storage really is DPAPI
and really is not the Credential Manager. Both read their own module's source, which is unusual and
deliberate: what they assert is the *shape* of a file, and a second entry point or a change of
store is exactly the kind of edit made without opening this document. There is precedent in §41,
where a test asserts a layout fact rather than a behaviour. Writing the first of them produced a
small joke at my expense — it failed immediately, because `include_str!` included the assertion
that named the forbidden API. It now reads only the module above the tests.

**What is not worth doing, which is most of it.**

A test cannot catch the third defect or anything like it. "The phone does not read this field" is a
statement about another component's behaviour at a moment in time; there is no assertion that
distinguishes *false* from *no longer true*, and the only way to find out is for a person to change
the phone and remember the sentence. That is not a gap a mechanism closes.

Nor is a general process worth inventing. The repair habit this project already has — amend at the
original, point forward, never quietly rewrite — has worked every time it has been used: §28 now
carries two forward-pointers, to §44 and to §50, and a reader arriving at the original decision
learns its full history in the place they are already looking. A document that recorded only what
survived would be a list of outcomes, and §47 already says why that is worse. The habit is the
mechanism, and it does not need a scaffold.

**The honest summary** is that one of these was a real process failure (restating a fact four
times), one was a documentation debt that a test now covers, and one was the document doing its job
— recording something true, and being amended when it stopped being true. Two small rules and two
tests, rather than a procedure: that is the whole of it.

**Amended 2026-10-12, later the same day.** A fourth one turned up within hours (§53): the README's
resource table quoted fresh-start numbers under a sentence promising they were measured with every
account connected. It is a fourth *kind*, and it bears on the conclusion above, so: it was neither
a plan written as a fact, nor duplication, nor a claim made false by later work. It was a measured
number that travelled away from the conditions it was measured under. No test can catch it — the
measurement needs a twelve-hour process with real accounts attached — and the rule "prefer a claim
that cannot drift" does not help, because the number *was* the point and deleting it would have
been a worse README.

The rule it does suggest is narrow and costs nothing: **a measured number carries its conditions
and its date, or it is not a measured number.** §8 does this and is still correct eighteen days
later precisely because it says "Phase 1" and "pet forced awake"; the README said "with mail,
calendar and phone sync all connected" over numbers from a Pip with nothing connected, and was
wrong within a day of being written. That is a third small rule rather than a mechanism, which
leaves the conclusion above standing: the count is now four, and the number of procedures invented
is still zero.

## 52. Two things a stranger found, 2026-10-12

The repository is public and there is a release, so Pip was installed from its own GitHub release
onto an empty profile and used as somebody who had never seen it. The download was the file the
release notes promise, the welcome read well, and a new profile correctly offered four characters
rather than seven (§43). Two things did not survive the look.

**Clicking "+ Add reminder" left a live alarm behind.** A new reminder was created enabled, with no
name, set for the next hour, on weekdays — so pressing the button to find out what it does earned
an interruption an hour later from something called "Reminder". It is also where the two reminders
called "Reminder" in the author's own list came from, which is the kind of evidence worth more than
an opinion: the person who wrote it had already fallen into it twice without noticing.

It is now created switched **off**, and switches itself on the moment it is given a name. Naming
something is when it stops being an experiment, and the alternative — leaving the person to find a
toggle afterwards — trades one annoyance for another. The id of the just-added reminder is
remembered so only that one is affected: switching on a reminder somebody had deliberately switched
off, because they happened to edit its title, would be its own small betrayal.

**Pip never said how to drink.** Double-clicking logs a glass and a single click shows the day, and
neither appeared anywhere in the app — only in the README, which nobody reads before running a pet.
Water is the thing Pip is mostly for. The welcome now says it.

Verifying the first half needed a devtools `type` command, because the half that mattered could not
be checked by clicking: an abandoned reminder staying off is only half the fix, and if naming one
had failed to switch it on, the result would have been **worse** than the bug — a reminder someone
named, expected, and never heard from. It was checked both ways: abandoned stays off, named comes
on.

**The title field now asks a question**, and showing it needed a second fix. It said "Title", which
names the box rather than telling anyone what to put in it, and the dates page two clicks away had
already solved this - "Name, e.g. Mom's birthday". It now reads "What to remind you about, e.g.
take vitamins".

Changing the words was not enough, because the hint was hidden whenever the field had focus, and a
new reminder is created already focused: the person was looking at an empty box and a cursor, which
is precisely when the hint is worth having. It now stays until they type, with the cursor in front
of it, which is what every other text box in the world does.

Left alone deliberately: the welcome cannot be shown again once dismissed. Real, and harmless.

## 53. The numbers the README promised were the wrong numbers, 2026-10-12

A four-hour memory sample (49 readings, five minutes apart, against the installed build at 7–11
hours of uptime with mail, calendar and phone sync connected) was taken to answer a worry about a
leak. It found no leak. It did find that the README's resource table was wrong, which is the more
interesting result, and the fourth instance of the pattern §51 is about.

Measured, live process, 12.7 hours uptime, everything connected:

| Metric | Fresh, nothing connected | In use, all sync on |
|---|---|---|
| Private working set ("Memory" in Task Manager) | 1.40 MB | 3.07 MB |
| Private bytes (committed) | 1.96 MB | 5.48 MB |
| Working set incl. shared pages | 14.3 MB | 15.4 MB |
| Threads | 3 | 3 idle, 9 peak mid-sync |
| Handles | 154 | 338–418 |
| CPU | — | 10.5 s over 12.7 h = 0.82 s/h |

The README had been quoting **1.8 MB private, 1.7 MB working set, 1 thread, 172 handles** under the
sentence "with mail, calendar and phone sync all connected and checking". Those are fresh-start
numbers. Nobody running Pip with their accounts connected would ever see them. The CPU claim of
~1 s/hour survived contact with reality (0.82 s/h measured over half a day); the memory claim did
not, and the thread count was out by a factor of three at idle and nine at peak.

Three distinct things were all being called "memory", which is how the error survived: private
working set (what Task Manager shows, and the honest "what does this cost" figure), private bytes
(committed, including pages swapped out) and total working set (which includes shared DLL pages
Pip does not pay for and is ~14 MB even on a cold start). §8's table is a Phase-1 snapshot with
nothing connected and no accounts to sync; it was correct when written and is still correct as a
dated record. The README is not dated, and that is the difference.

On the leak question specifically: private bytes rose from 4.74 MB to 5.93 MB across the four
hours, which looked like 0.19 MB/hour of steady growth — and an hour later, unprompted, it was back
down to 5.48 MB and handles had fallen from 415 to 358. The first read of this was reported to
Andre as handles "climbing steadily with no sign of levelling off", which came from comparing two
single readings instead of hourly averages. Hourly averages show handles flat after the first hour
(373 → 386). Both figures oscillate inside a band and return to it. WinHTTP handles are RAII-wrapped
(`Handle::drop` calls `WinHttpCloseHandle`) and there is no raw `CreateThread` anywhere, so there
was no mechanism for a leak either. **No change made.** A band that is re-entered is not a leak,
and an endpoint-to-endpoint comparison of a noisy series is not a trend.

What the measurement cost: a resource claim in a public README, corrected four days after
publishing it, found only because somebody went looking for a different bug.

## 54. An open bug: Pip saved only when it exited, 2026-10-13

**Open. Not diagnosed. Recorded here because it is invisible from the outside and the next person
to look at it — including me in a week — will not find it by reading the code.**

The symptom, found while checking on a long-running install rather than by looking for it:
`state.json`, `settings.json` and `pip.log` had not been written since 8 October, across at least
two restarts, while the same process was cheerfully writing `calendars\*.json` and `cmd-reply.txt`
into the same folder minutes earlier.

Pip debounces saves: anything that changes state sets a flag and asks for a save 1.5 seconds later
(`schedule_save` → `T_SAVE` → `flush_save`). `shutdown()` calls `flush_save()` directly instead.
In the affected process the first path did nothing and the second one worked:

| | Old process (up 6 h) | Fresh process |
|---|---|---|
| `--cmd show`, then watch the folder 8 s | nothing | `state.json.tmp` → rename → `state.json`, 1.5 s later |
| startup line in `pip.log` | missing | written |
| data written at exit | yes — everything survived | — |

So the save *code* is fine, the folder is writable, and the file permissions are correct. What had
stopped was the scheduled save, and only in that one process. Everything accumulated over four days
reached disk at shutdown, which is the only reason this cost nothing.

**Why it matters anyway.** For four days a crash or a power cut would have lost everything since
launch. Pip's whole persistence story is "write a temp file and rename it, so an interrupted save
cannot corrupt anything" (§4) — which protects against a save going wrong, and not at all
against a save never being attempted.

**Ruled out, with evidence rather than reasoning:** permissions (`icacls` shows full control, and
the process wrote other files in that folder); the wrong data folder (the reply file it wrote
landed in `%APPDATA%\Pip`, so that *is* its folder); disk space (64 GB free); and the §39 read-safety
valve, which would have blocked the shutdown save too, and would have left Pip running on a blank
state — a newborn companion at the default screen position, which is not what was on screen.

**The lead, which is not a diagnosis.** Handle count had climbed to 497 over six hours; a fresh
process sits at 172. Something that accumulates with uptime is a suspect, and `SetTimer` failing is
the kind of thing that would stop a debounced save while leaving direct calls alone. That is a
hypothesis. It has not been tested, and §53 was written the same morning about moving faster than
the evidence.

**Two things I got wrong before getting here**, both reported to Andre as fact and both corrected
within the hour: that the four days of water and experience were unrecoverable (they were in memory
and were written at exit), and that the read-safety valve had tripped (it cannot have). The first
mattered — it is the difference between "your data is gone" and "your data is fine" — and the lesson
is the same one as §53: an endpoint, a single reading, or one plausible mechanism is not a finding.

**How to make it reproducible, which is the only next step worth taking.** Let the install run and
compare one timestamp: if `state.json` stops advancing again after a day or two of uptime while Pip
is plainly still working, this becomes a bug with a handle on it. If it never recurs, it stays here
as a recorded one-off rather than being quietly forgotten.

**The fallback, if the cause is never found.** The worse half of this bug is that it is undetectable
by the person using it: Pip looks completely normal while silently not saving. A save that fails, or
a scheduled save that never arrives, should be something Pip can notice about itself and say out
loud — the §39 valve already proves Pip can refuse to write and explain why. Making the *absence* of
a write visible is a smaller job than finding the cause, and it is the one that survives not finding
it.

**Was parked, pending this; built once this was fixed — §55.** The burrow animation — Pip digging a
hole and climbing into it instead of vanishing when hidden — was proposed in full and not built
that day: tray hide only, the hole
closing over rather than persisting, `PopIn` kept, and the body disappearing by shrinking as it
descends under an overdrawn near lip rather than by adding a clip to the one drawing function the
phone also calls. It is polish, it fixes nothing broken, and a save path that cannot be trusted
outranks an animation. The proposal is not written down here on purpose: an unbuilt design
recorded as prose is exactly the "plan written down as a fact" that §51 names as the first way this
document goes stale.

**Diagnosed and fixed, 2026-10-15.** Two days of being wrong in public, then an ordinary answer.

The correlation is exact. The machine had not rebooted since 8 October — 180 hours of uptime — which
I first read as "there have been no logons", and it is not what it means: Windows records eight
*interactive logon sessions* in that window, because Andre signs out rather than restarting. Lined
up against which Pip processes saved:

| Logon | Pip started | Logged its start | Saved |
|---|---|---|---|
| 12 Oct 10:28:51 | 10:28 | no | no |
| 13 Oct 06:47:47 | 06:48:32 | no | no |
| 14 Oct 06:36:12 | 06:36 | no | no |
| 14 Oct 18:36:21 | 18:37:24 | no | no |
| *(none — started by hand)* | 13 Oct 12:56 | **yes** | **yes, for 17 hours** |

Every Pip that starts in the rush of a logon cannot read its own files, trips the §39 valve, and
spends that whole session not saving. Every Pip started by hand is fine. `explorer.exe` restarting
at 18:36:27 is the independent witness that 18:36 really was a fresh logon.

**The second thing I had wrong**, and it is the one that mattered: I told Andre the four days of
water and experience had been rescued by the save in `shutdown()`. They had not. The data appeared
42 seconds *after* the replacement process started, which is when it first syncs — **the phone had
been holding it all along** (§48, doing exactly what it was built for). Had the phone not existed,
each logon would have silently cost everything since the last manual launch.

**The fix is three things, and none of them needs the exact Win32 error.**

*The read is retried.* Six attempts, 60 ms apart, for anything except "not there" — which returns
at once, being the ordinary first run on the startup path of every launch. Being cleverer about
which error codes are transient would mean guessing which one a logon produces, and the price of
guessing wrong is a day of someone's data, against 300 ms once on a path that ends in a warning
anyway. A test holds a real `share_mode(0)` lock on `state.json` for 150 ms and asserts that the
session still saves.

*A log line that cannot be written is kept, not dropped.* The log is the only record of what went
wrong, and the moment it matters most — a locked file, a logon, a full disk — is exactly the moment
the write fails. `log()` now buffers in order and flushes the backlog on the next call that opens
the file, bounded at 200 lines so a permanently broken write cannot grow into a leak. This is why
the bug was invisible: the one sentence explaining it was being thrown away for a week, which is
also why `pip.log` showed a four-day gap with no complaint in it.

*The warning outlives the moment.* It used to be a bubble shown once at startup with a 120-second
dismiss timer, at 06:36, while nobody was at the machine. The bubble now has no timer and waits for
its button; the tray tooltip carries "Not saving: Pip cannot read its own files" for as long as it
is true; and if the pet is hidden, the notification area gets it, on the same reasoning §4 gives for
reminders — a bubble on a hidden pet is nobody telling anybody.

**Declined, having said in the morning that it was part of the fix: re-enabling saving mid-session.**
It sounds obviously right and it is the §39 disaster with extra steps. When a read fails, what is in
memory is *defaults*; clearing the flag later and saving would write those defaults over the real
file, which is precisely the thing the valve exists to prevent. Doing it safely would mean re-reading
and adopting the file mid-session, merging it against whatever the person has done since — a real
feature, for a case the retry above should now make rare. Rare and loud beats clever and destructive.

**And the measuring tool had been broken the whole time.** `tools/measure.ps1` launches the release
binary, which is by definition a loose exe, so Pip put up "Install Pip on this PC?" and waited; the
script then measured a process sitting on a modal dialog with no window and no log, and reported
only that it could not find a log file. It has broken silently since the install offer was added
(§16), through two runs today, and it left a dialog box on Andre's desktop. It now passes
`--portable`. A tool that fails by measuring the wrong thing is worse than one that fails loudly,
which is the same complaint as the rest of this section.

**What the repaired tool said the moment it ran, 2026-10-15** (4-minute window, pet forced awake,
throw-away data folder, no accounts connected — the §8 protocol):

| Metric | Measured | Target | |
|---|---|---|---|
| Processes | 1 | 1 | ✅ |
| Idle private memory | 1.82 MB avg / 1.93 MB max | ≤ 8 MB | ✅ |
| Idle working set | 2.14 MB avg / 3.69 MB max | ≤ 15 MB | ✅ |
| Private working set | 0.49 MB avg / 0.83 MB max | — | |
| **Idle CPU** | **1.76 s per hour** (cycle-exact, 3.701 GHz) | **≤ 1 s/h** | ❌ |
| Startup | 38 ms avg, 19–105 | ≤ 150 ms | ✅ |
| Threads / handles | 3 / 171 max | — | |
| Executable | 2125 KB | ≤ 3 MB | ✅ |

**Idle CPU has drifted past its target and nobody knew**, because the only instrument that would
have said so had been quietly measuring a modal dialog. §8 recorded 0.96 s/h under this exact
protocol on 21 September; it is 1.76 s/h now. Not caused by this section's fix, which adds nothing
to the idle loop — the retry runs at startup and the log buffer only when something is logged — and
startup is unchanged at 38 ms because no file was locked. The real-world figure is still under the
target (0.82 s/h measured over 12.7 hours of ordinary use in §53, where the pet sleeps and is not
forced awake), so this is a worst-case target missed rather than a promise broken to anyone.

It is left open and unchased, deliberately. Finding 0.8 s/h of drift across five phases of features
is a bisecting job, and doing it badly — a plausible culprit named after one look — is the mistake
this section already records twice. What it is not allowed to be is forgotten again: the number is
written down here, with the date and the protocol, which is the rule §51 arrived at.

**Corrected a few hours later, in §55's measurement pass: 1.02 s/h.** Same protocol, same build but
for the burrow, which adds nothing to the idle loop. Two runs, 1.76 and 1.02, against a target of
1.00 and a September reading of 0.96; the tool's own tick-sampled column moved 2.04 → 0.23 across
the same pair and is labelled noisy for that reason. So **"idle CPU has drifted past its target" was
over-read from a single sample**, which is the error this section was written about, committed
inside the section itself, within a day. What is actually true is narrower and duller: idle CPU sits
around its 1 s/h target, and a four-minute window cannot tell 0.96 from 1.76. Establishing whether
there is drift at all needs a longer window first — and that, not a bisect, is the next step if
anyone wants one. The paragraph above is left standing rather than rewritten, because a document
that quietly edits its wrong readings out is the one §47 argues against.

## 55. The burrow, 2026-10-15

Pip used to vanish when you hid it. It now digs a hole, climbs in, and the hole closes over. Coming
back is the same in reverse. It is pure polish and fixes nothing, which is why it waited behind §54.

**Where it applies, and where it deliberately does not.** The tray's Show/Hide and `--cmd hide`,
which are the only two callers of `set_visible(false)`. Not quiet hours: Pip sleeps *visibly* at
night, eyes shut with a Zzz, and that sleeping pet is the only thing on screen saying why it has
gone quiet — swapping it for an empty hole would delete information to gain an animation, and would
mean a hole sitting on the desktop for eight hours. Not a fullscreen app either: that path calls
`ShowWindow` directly and never comes through here, which is correct, because someone playing a
game wants the pet gone now and not in two thirds of a second. **Calm** skips it too and hides
instantly, since Calm is what you pick when you want fewer moving parts.

**The hole does not persist.** It closes over Pip and then the window hides, leaving a clean
desktop. A dark oval left lying there would still be a thing on your screen — on top of everything
else, if always-on-top is set — it would keep the window composited and hit-testable all day for
nothing, and it would turn "where did Pip go?" into a different confusion rather than less of one.

**There was no room, and that nearly killed it.** The window is 112×104 and the ground line Pip
stands on sits four design units from the bottom edge. A hole deep enough to read as a hole needs
more than four units. Making the window taller was the trap: its top-left corner is the position
you dragged Pip to, it is saved to disk, and growing the box downward would shift every existing
companion and change how it clamps to screen edges. So the hole lives *above* the ground line, in
the space the creature vacates on its way down — centred at y 90 with a 30×8.5 radius, which fits
inside the existing window with nothing to spare and nothing to change.

**How the body disappears, after getting it wrong once.** The first version drew the creature and
then painted the front half of the hole's rim over it. It looked exactly like what it was: a
straight dark line sawing the body in half, four units above the feet, before Pip had gone anywhere.
The mistake was thinking of the rim as a lid. A hole seen at this angle is a *mouth* — its near edge
is the ellipse's lower arc, and anything inside it is visible through the whole opening. There is
nothing to paint over the body at all.

What hides the body instead is that **it shrinks faster than the hole narrows**, so at no point is
it wider than the thing it is going into. That is also what a small animal going down a burrow
actually looks like: it foreshortens. The pose system could already sink and scale about the feet —
growing up (§30) uses the same mechanism — so the whole animation is two curves and no new drawing
machinery.

*Clipping was the other option and was refused.* Telling the renderer "paint nothing below this
line" is exact and tempting, and it means adding a clip parameter to `creature::draw` — the one
function the phone also calls (§50) — to support something the phone will never do. That is the
first crack in the one-drawing rule, bought to solve a problem that shrinking already solves.

**Interruptions.** Hidden is recorded the moment you click, not when the digging stops, so a crash
or a shutdown mid-dig still brings Pip back hidden — what you asked for. A reminder that fires
mid-dig goes to the notification area rather than into a bubble that is about to be thrown away
with the window, which is simply what a hidden pet already does. The pet stops accepting the mouse
the instant it starts digging: halfway down a hole is not a thing you can pick up.

**What did not change.** `PopIn` stays and is still what first launch uses; `Unburrow` only
replaces it on the show side of a hide. The tray icon is a portrait and its job is identity, not
state, so it does not become a hole. The phone gets nothing: it asks for one still frame through
`Pose::base` and `creature::draw` and never touches `animate`, so adding two `AnimKind`s does not
reach it and `there_is_exactly_one_way_in` is untouched. No shape moved, `body.bottom` is still
93.0, and `every_creature_fits_the_window` still passes.

**Two tests, both earned.** `no_creature_is_wider_than_the_hole_once_it_is_inside` walks all seven
characters through the dig and asserts the body is never wider than the hole at that moment — the
next wide body gets told here rather than on somebody's desktop, since nothing clips it. And
`the_burrow_ends_with_an_empty_window` renders the last frame and counts the painted pixels, because
the first version left a three-pixel speck sitting in the middle of a closed hole: the creature's
scale bottomed out at five per cent while the hole went all the way to nothing. Two curves that have
to finish together is exactly the kind of thing a number in a comment does not keep true.

**Measured** (§8 protocol, 4-minute window, pet forced awake): 1.81 MB private memory, 1.93 MB
working set, 30 ms startup (16–85), 4 threads, 171 handles, 2128 KB binary — all unchanged by this
section, which is what it should be: the animation only runs while you are hiding or showing the
pet, and the window draws nothing at all the rest of the time. Idle CPU 1.02 s/h, which is the
reading that corrected §54's claim of drift.

**The first build of this dug for ever.** Finishing the dig calls `set_visible(false)` a second time,
to do the hide it deferred, and that call came back through the same branch that starts a burrow —
because the "I am burrowing" flag had been cleared one line too early. The guard it needed to fail
was the one thing switched off before the call. Pip dug, finished, dug again, thirty times a second,
and never hid.

Nothing in the test suite could have caught it: the loop lives in the window's own message pump,
which needs a real `HWND`, a real timer and a real message loop. What caught it was a trace of three
lines — `set_visible` on entry, `on_frame` while burrowing — against a throw-away instance in a
temporary data folder, which printed the cycle immediately. Afterwards, with the flag left set
across the call, the same instance measured **665 ms from asking to hidden** (650 ms of animation
plus a frame) and 23 ms from asking to visible.

Two lessons, both already in this document and both ignored here. The flag is read for two different
questions — "should I start?" and "am I finishing?" — and a single `bool` doing that is exactly the
sort of thing §51 says to make impossible rather than to comment; it is left as a `bool` with the
reason written at both ends, because the alternative is a state enum for one animation. And the
verification had to be done on a running window, not reasoned about: the arithmetic for this looked
fine, and the arithmetic for the near lip looked fine too (§55 above), and both were wrong in a way
only looking could show.

## 56. Making it visible, and a burrow to sleep in, 2026-10-15

Andre watched §55 run on his own desktop and asked for three things: make it more visible, use it
for the night, and add more of these small animations. All three are the same observation from
different angles — the companion is 112 pixels tall on a screen someone is working on, and a
gesture that is *technically* on screen is not the same as one anybody sees.

**Slower, and in beats.** 650 ms was long enough to see and too short to notice: by the time your
eye arrived it was a closed hole. The burrow is now 1500 ms and reads as four separate things —
dig, look down at what you dug, drop in, hole closes over. The looking beat does most of the work:
it is the one that makes the drop deliberate rather than a slide.

**Earth, because four pixels of arm is nothing.** The paws move about four pixels at normal size.
Specks of soil thrown out of the hole are what actually says "digging" at this scale. They taught
two lessons immediately. Drawn last, with the other badges, they landed on the creature's face —
Pip dug its way to a faceful of its own soil — so they moved to the ground layer, before the body.
Drawn behind the body and thrown straight up from the feet, they were then invisible for their
whole arc, because "behind the body" means behind *Pebble's* body, which is 43 units wide. They now
leave the ground already clear of the widest creature in the set.

**The night burrow, which contradicts §55 and should.** §55 refused to use the burrow for quiet
hours, and the argument was that a sleeping pet is the only thing on screen saying *why* nothing is
happening, so swapping it for an empty hole trades information for an animation. That argument was
about *hiding*. It does not apply to what was actually wanted: the hole stays open and the
companion stays visibly asleep in it, with its Zzz. Nothing is traded. At the start of quiet hours
Pip digs in, yawns on the way down and curls up; in the morning it stretches and climbs out.

The resting pose and the two transitions both *set* the body transform rather than multiplying it,
and the transitions are skipped while the resting pose applies, because two things that both set a
scale will cheerfully compound and put the companion down a well. The Zzz learned to follow the
head: three Z's left at standing height floated in the sky above an apparently empty hole.

Quiet hours only — not "you walked away", not a locked screen. Those are states Pip leaves the
moment you touch the machine, and digging a hole for a coffee break is a lot of ceremony for ninety
seconds. Clicking a sleeping companion climbs it out of the burrow rather than snapping it upright,
which is the same rule the rest of this section is about.

**Five more idle animations, and why they were nearly free.** Yawn, Sniff, Scratch, Sneeze and
Settle, bringing the idle pools to three, seven and nine by liveliness. The thing worth writing
down is the cost: **how often an idle animation plays is what costs CPU, and that did not change.**
The timer and its intervals are untouched. What grew is how many different ones it can pick from,
which is a match arm that runs only while one is already playing. Variety is the cheap half of
liveliness and frequency is the expensive half, and it is easy to reach for the wrong one.

Sneeze briefly borrowed the digging's soil for its puff and looked exactly like a companion
sneezing dirt. The whip of the head carries it without help.

**Measured** (§8 protocol, 4-minute window, pet forced awake): 1.80 MB private memory, 1.89 MB
working set, 24 ms startup (11–70), 4 threads, 171 handles, 2132 KB binary. **Idle CPU 0.89 s/h,
inside the 1 s/h target** — with five more animations in the pools than the run that reported
1.76 s/h, which is the claim about variety being free, measured rather than asserted.

That is now three readings of the same number under the same protocol: **1.76, then 1.02, then
0.89**, a spread of nearly a second against a target of one. §54 called the first of those drift
and had to take it back; what is actually true is that a four-minute window cannot measure this to
better than about ±0.5 s/h, and anyone wanting to know whether idle CPU has really moved since the
0.96 s/h of 21 September needs a longer window, not another four minutes.

**The two of them met, at 22:48 the same night.** Quiet hours had started and the pet was hidden,
so showing it played `Unburrow` — climbing out of a hole it was supposed to be asleep in, after
which the resting pose dropped it straight back down. The mirror was worse: hiding while already
nestled started a fresh `Burrow`, which snapped the hole it was lying in shut and dug another one.

Both are the same mistake. Two features each own the body transform and the hole, and each was
written as though it were the only one. The fix is one line in each direction — disappearing is
not dug while it is already in a burrow, and appearing plays nothing at all, because a window that
appears is already the whole transition. Found by watching the real window rather than by thinking
about it, which is now three for three in this pair of sections.

## 57. A creature rather than a slideshow, 2026-10-16

A long brief: redesign the digging, give the companion real sleeping routines, many more idle
behaviours, and a selection system so none of it repeats mechanically — while staying extremely
light. Being done in stages; this section grows with them.

**What the system already had, and why the digging looked thin.** Animations are pure functions of
`(kind, t)` that tweak a resting `Pose`, the window only renders frames while one is actually
playing, and an idle timer picks one every few minutes. That is a good shape and nothing here
rewrites it. The digging looked thin for two unrelated reasons: it was one continuous slide with no
beats (fixed in §56), and there was only ever *one* of it, so the second time you saw it you had
seen it.

**Three things asked for that Pip cannot honestly do, said now rather than quietly skipped.**

*Hiding behind desktop icons and files.* Pip is a layered window that knows nothing about the
desktop; finding where icons sit means reaching into another process's list view and reading its
memory. That is fragile, it breaks on every shell change, and a pet that enumerates your desktop
contents is a privacy story this project does not want to have to explain (§4). **Declined.**

*Playing with his tail.* No character in the cast has one — they are mochi bodies with feet and a
crown (§41). A tail is a new body part for seven creatures, not an animation. **Not now**, and it
belongs in a character design rather than here.

*Popping out somewhere else.* Possible — the window moves, and dragging already proves moving it
per frame is cheap. But where Pip sits is a position *you* chose and it is saved; moving it and
staying there would be the app rearranging your desktop. Worth doing only as "pops up a short way
off, realises, comes back", which is a walking animation, and walking is its own stage.

**The selection system** (`behaviour.rs`) replaces `options[rand % len]`. Each behaviour carries a
weight, a cooldown and the quietest liveliness it is allowed at, and **doing nothing has its own
weight** — about a third of the time, because a companion that acts every time its timer fires is
one you watch instead of work (§7). Nothing repeats back to back. Cooldowns start at launch, so a
rare animation cannot land in the first minute of the first run.

It owns no timers and starts nothing. It answers one question when the existing idle timer asks,
and that timer's cadence is untouched — which is the whole performance story, and the thing it
would be easy to get wrong by reaching for frequency when the ask was variety. Six tests cover
what is otherwise untestable about randomness: Calm never gets a lively behaviour, sitting still
happens 20–45 % of the time, the rare one stays at least eight times rarer than a common one,
nothing plays twice running, cooldowns hold, and a yawn is an evening thing.

**Three ways down the hole**, weighted 7/2/1: the plain dig, *peek* — in, then straight back up to
check whether anybody saw, then gone — and *stuck*, which wedges halfway, heaves and slides back
twice, and pops through. All three end identically, gone with the hole shut, which is what lets the
window keep hiding on "the animation finished" without knowing which one ran; a test asserts that
for every character and every variant.

The struggle needed rebuilding once. Rocking side to side was the obvious way to show being gripped
and at this size thirteen degrees of tilt is about two pixels of lean. Losing and regaining a tenth
of the descent moves the whole creature, and that is what reads.

**Sleep, and the one decision that mattered.** The ask was breathing, dreaming, twitching feet,
rolling over, a nightmare, waking slowly — a night with a shape to it rather than a still frame
held for nine hours.

*There is no continuous breathing, on purpose.* A gentle breath that never stops means the 30 fps
clock never stops: roughly 65 seconds of CPU over a nine-hour night, against a target of one second
per hour (§8). It would have been the most expensive thing in the program by an order of magnitude,
spent animating something nobody is looking at — Pip stops idling entirely once you leave the
machine, so the only audience for all-night breathing is an empty room. Sleep instead moves the way
the waking pet does: short pieces, long gaps, chosen from a table. A breath that arrives every few
minutes is also *more* noticeable than one that never stops.

Five things it does while under: a long breath, a twitch with a kick of the feet, turning over, half
an eye open to check the room, and — twice a night at most — a bad dream, which bolts it upright
wide awake, looks around, finds nothing, and sinks back. They run on top of the resting pose rather
than replacing it, each starting and ending at nothing, which is what lets them drop into a night's
sleep without a seam at either end. The transitions do the opposite and *set* the body, because
they begin or end at the resting pose, and `AnimKind::owns_the_body` is the one place that
distinction is written down rather than restated at each call.

Three ways to bed — straight in; in and then unable to get comfortable, shuffling about; or sitting
on the edge nodding off, head dipping lower each time until it slumps in — and two ways up: a
stretch and a climb, or rubbing its eyes first and yawning on the way out. Liveliness does not
apply to any of it. A Calm companion still breathes, and a Lively one does not have livelier dreams.

The nodding needed doubling before it read at all. These are round bodies with no shoulders to
measure a lean against, so twenty degrees of dignified nod is indistinguishable from sitting up
straight; thirty is a creature falling asleep.

**Measured** (§8 protocol): 1.77 MB private memory, 1.89 MB working set, 38 ms startup, 4 threads,
171 handles, 2142 KB binary, **idle CPU 0.69 s/h** — the lowest of the four readings this number
has had (1.76, 1.02, 0.89, 0.69), taken on the build with by far the most animations in it.

Not a coincidence and not only the noise of §54: giving "do nothing" a real weight means **fewer**
animations actually play per hour than when the old pool picked one every single time its timer
fired. More behaviours, less movement, less CPU. That is worth stating plainly because the intuition
runs the other way, and the intuition is what would have led to building the breathing.

**The everyday library, and the first time Pip leaves the spot it was put in.** Six more behaviours:
leaning in at the screen and holding the look a beat too long; a whole-body shake; crouching and
pouncing on something only it can see and landing on nothing; wandering off a little way, stopping,
thinking better of it and coming back; tripping over nothing, sprawling, picking itself up as
though nothing happened; and — rarest of the set, once every forty-five minutes at the very most —
finding something tiny by its feet, picking it up, holding it out to admire, and discovering it was
never there.

*Travelling.* Three of those move the window. The rule they are built around is that **where the
companion sits is a place the person dragged it to and it is saved to disk**, so an animation may
borrow that position and must hand back exactly what it took. The curve lives in `walk_offset`,
next to the animation it belongs to rather than in a table across the window code, and the window
offsets where it draws without ever touching the saved anchor. A test asserts every journey starts
and ends at zero and never goes further than the window can carry — a walk that ended a pixel out
would carry the pet across the desk over a morning, one trip at a time. The direction is chosen
per journey from whichever side has room, so it never shuffles up against a screen edge.

*Mood.* Thirsty is the one mood that changes what the companion feels like doing: the bouncy
behaviours — hop, pounce, stumble — simply stop being candidates. It never says anything about it,
per §28. It just has less bounce, which is how a mood reads on anything alive.

**A test that paid for itself five times before it was finished.** Animations can put the body
anywhere, and `every_creature_fits_the_window` only ever checked the shapes standing still. The
stumble landed with a drop of seven design units into four units of floor and sprawled with its
chin sawn off by the bottom of its own window — obvious the moment it was rendered, invisible to
every test that existed. `no_animation_pushes_the_creature_through_the_floor` walks every
animation, every character and twenty-five points in time, transforms the body the way `draw` does,
and checks the bounds against the window.

Written to catch the stumble, it immediately caught four more: the sniff, the sneeze, the shake and
the find, all of them tilting a wide body about a point near the floor and swinging a hip through
the bottom edge. **Two of those shipped the night before and are on Andre's desktop now.** The
general fix is the same each time and is a real thing about these bodies rather than a fudge: a
body pivoting on its feet rises, so a lean needs a lift under it, and the widest character in the
cast is the one that decides how much.

**Measured** (§8 protocol): 1.78 MB private memory, 1.92 MB working set, 30 ms startup, 4 threads,
171 handles, 2148 KB binary, **idle CPU 0.32 s/h** — a third of the target, and the lowest of the
five readings this number has had (1.76, 1.02, 0.89, 0.69, 0.32), on the build with twenty-two
more animations than the one that started the sequence.

The trend is now too consistent to be the noise of §54 alone. Giving "do nothing" a real weight and
putting cooldowns on everything means the companion moves *less often* than it did when a shorter
list picked something every time its timer fired. Twenty-two more things it can do, a third of the
CPU, and it is the cooldowns rather than the drawing that did it — which is worth remembering the
next time the obvious way to make something feel more alive is to make it move more.

## 58. Pictures that cannot drift, 2026-10-16

The repository has been public for four days and contained no picture of the creature. A desktop pet
described only in prose, behind an unsigned download: there is no version of that which a stranger
installs.

They are **generated, not captured** — `cargo run -p pip-draw --example shots --features png-format`
writes every image in the README out of the same `creature::draw` the desktop and the phone call.
That is the whole reason to trust them. A screenshot is a claim about what the program looks like,
and this document has had to correct four claims-kept-by-hand in a fortnight (§51, §53, §54, §57).
A generated one cannot be wrong without the program being wrong, which is the only kind of wrong
worth having. It also means the next character, or a change to how anything is drawn, updates the
pictures by being run rather than by being remembered.

Four of them: the whole cast in seven colours, one companion at its three stages of growing up, the
dig from first scratch to closed hole, and one asleep in its burrow. 120 KB in total, which is the
budget a repository this size can afford without thinking about it.

The crop is found rather than fixed. Each creature stands on the bottom of its window and the space
above belongs to whatever crown it has — a sprout, long ears, a mushroom cap — so a fixed margin is
wrong for every character but one. The tool finds the drawn pixels and trims to them, which also
means it will be right for a creature nobody has drawn yet.

The example declares `required-features = ["png-format"]`, because a plain `cargo test` builds every
example and must not need a PNG encoder to compile a tool whose entire purpose is writing PNGs.
