// Mock of the Google endpoints Pip uses (OAuth + Gmail API, metadata scope), for automated tests
// of the real sign-in and sync code without a Google account. Dev builds only talk to it when the
// PIP_OAUTH_AUTH / PIP_OAUTH_TOKEN / PIP_GMAIL_API environment variables point here.
//
//   node tools/mock-google.js [port]
//
// Control endpoints (for the test script):
//   POST /_deliver  {"from": "...", "subject": "...", "labels": ["INBOX","UNREAD"]}
//   POST /_expire_access      -> current access token becomes invalid (401)
//   POST /_expire_history     -> old history ids return 404
//   POST /_revoke             -> refresh token no longer works (invalid_grant)
//   POST /_mode {"auth": "ok"|"deny"|"partial"|"hang"|"badclient", "email": "..."}
//                             -> how the next consent screens behave (user allows / cancels /
//                                unticks the box / closes the tab / app blocked) and who signs in
//   POST /revoke              -> the real revocation endpoint (logged; kills the refresh token)
//   GET  /_log                -> request log
"use strict";
const http = require("http");
const crypto = require("crypto");
const port = Number(process.argv[2] || 8787);

const CLIENT_ID = "123456789012-pipmocktestclientabcdefghijkl.apps.googleusercontent.com";
const CLIENT_SECRET = "GOCSPX-MockSecretForPipTests1234";
const SCOPE = "https://www.googleapis.com/auth/gmail.metadata";
const CAL_SCOPE = "https://www.googleapis.com/auth/calendar.readonly";
const DRIVE_SCOPE = "https://www.googleapis.com/auth/drive.appdata";
let challenges = {};        // code -> { challenge, redirect }
let access = null;          // valid access token
let refreshValid = "RT-1";
let accessSeq = 0;
let historyId = 1000;
let minValidHistory = 0;
let messages = [];          // { id, historyId, labels, from, subject, date }
const log = [];
let mode = { auth: "ok", email: "andre.test@gmail.com" };
let currentEmail = "andre.test@gmail.com";
let grantedScope = SCOPE;
// Calendars the test account has, and their events.
let calendars = [
  { id: "andre.test@gmail.com", summary: "Andre", primary: true, selected: true, accessRole: "owner", defaultReminders: [{ method: "popup", minutes: 10 }] },
  { id: "fam@group.calendar.google.com", summary: "Family", selected: true, accessRole: "reader", defaultReminders: [] },
  { id: "hidden@group.calendar.google.com", summary: "Hidden", selected: false, accessRole: "reader", defaultReminders: [] },
];
let calEvents = {};   // calendarId -> [event]
// Drive app-data folder: name -> { id, bytes }
let appData = {};
let driveSeq = 0;
const revoked = [];

const b64url = (buf) => buf.toString("base64").replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
const json = (res, code, obj) => { const b = JSON.stringify(obj); res.writeHead(code, { "Content-Type": "application/json", "Content-Length": Buffer.byteLength(b) }); res.end(b); };
const body = (req) => new Promise((r) => { let d = ""; req.on("data", (c) => (d += c)); req.on("end", () => r(d)); });
const issued = new Set();
const newAccess = () => { access = "AT-" + ++accessSeq; issued.add(access); return access; };
const authed = (req) => {
  const t = (req.headers.authorization || "").replace("Bearer ", "");
  return issued.has(t) && t !== "expired";
};

http.createServer(async (req, res) => {
  const url = new URL(req.url, "http://127.0.0.1:" + port);
  log.push(req.method + " " + url.pathname + url.search.replace(/(code|code_verifier|refresh_token|client_secret)=[^&]*/g, "$1=***"));
  const p = url.pathname;
  try {
    if (p === "/auth") {
      const q = url.searchParams;
      if (q.get("client_id") !== CLIENT_ID) return json(res, 400, { error: "invalid_client" });
      if (q.get("code_challenge_method") !== "S256" || !q.get("code_challenge")) return json(res, 400, { error: "pkce_required" });
      if (![SCOPE, CAL_SCOPE, DRIVE_SCOPE].includes(q.get("scope"))) return json(res, 400, { error: "unexpected_scope", scope: q.get("scope") });
      if (!(q.get("prompt") || "").includes("consent")) return json(res, 400, { error: "consent_prompt_expected" });
      const back = `${q.get("redirect_uri")}/?state=${encodeURIComponent(q.get("state"))}`;
      if (mode.auth === "hang") return json(res, 200, { note: "user closed the tab" });
      if (mode.auth === "deny") {
        res.writeHead(302, { Location: `${back}&error=access_denied` });
        return res.end();
      }
      const code = "CODE-" + crypto.randomBytes(6).toString("hex");
      challenges[code] = { challenge: q.get("code_challenge"), redirect: q.get("redirect_uri"), partial: mode.auth === "partial", email: mode.email, scope: q.get("scope") };
      res.writeHead(302, { Location: `${back}&code=${encodeURIComponent(code)}&scope=${encodeURIComponent(q.get("scope"))}` });
      return res.end();
    }
    if (p === "/token" && req.method === "POST") {
      const f = new URLSearchParams(await body(req));
      if (mode.auth === "badclient" || f.get("client_id") !== CLIENT_ID || f.get("client_secret") !== CLIENT_SECRET) return json(res, 401, { error: "invalid_client" });
      if (f.get("grant_type") === "authorization_code") {
        const c = challenges[f.get("code")];
        delete challenges[f.get("code")];
        if (!c) return json(res, 400, { error: "invalid_grant", error_description: "unknown code" });
        const expect = b64url(crypto.createHash("sha256").update(f.get("code_verifier") || "").digest());
        if (expect !== c.challenge) return json(res, 400, { error: "invalid_grant", error_description: "PKCE verifier mismatch" });
        if (f.get("redirect_uri") !== c.redirect) return json(res, 400, { error: "invalid_grant", error_description: "redirect mismatch" });
        currentEmail = c.email;
        if (c.partial) return json(res, 200, { access_token: newAccess(), expires_in: 3599, refresh_token: refreshValid, scope: "openid", token_type: "Bearer" });
        grantedScope = c.scope;
        return json(res, 200, { access_token: newAccess(), expires_in: 3599, refresh_token: refreshValid, scope: c.scope, token_type: "Bearer" });
      }
      if (f.get("grant_type") === "refresh_token") {
        if (f.get("refresh_token") !== refreshValid) return json(res, 400, { error: "invalid_grant", error_description: "Token has been expired or revoked." });
        return json(res, 200, { access_token: newAccess(), expires_in: 3599, scope: grantedScope, token_type: "Bearer" });
      }
      return json(res, 400, { error: "unsupported_grant_type" });
    }
    if (p.startsWith("/gmail/v1/users/me/")) {
      if (!authed(req)) return json(res, 401, { error: { code: 401, message: "Invalid Credentials" } });
      const rest = p.slice("/gmail/v1/users/me/".length);
      if (rest === "profile") return json(res, 200, { emailAddress: currentEmail, messagesTotal: messages.length, historyId: String(historyId) });
      if (rest === "history") {
        const start = Number(url.searchParams.get("startHistoryId"));
        if (start < minValidHistory) return json(res, 404, { error: { code: 404, message: "Requested entity was not found." } });
        const added = messages.filter((m) => m.historyId > start);
        // Page size 2 to exercise pagination.
        const offset = Number(url.searchParams.get("pageToken") || 0);
        const page = added.slice(offset, offset + 2);
        const out = { historyId: String(historyId) };
        if (page.length) out.history = page.map((m) => ({ id: String(m.historyId), messagesAdded: [{ message: { id: m.id, threadId: m.id, labelIds: m.labels } }] }));
        if (offset + 2 < added.length) out.nextPageToken = String(offset + 2);
        return json(res, 200, out);
      }
      if (rest.startsWith("messages/")) {
        if (url.searchParams.get("format") !== "metadata") return json(res, 403, { error: { code: 403, message: "Metadata scope does not support 'format' other than metadata" } });
        const m = messages.find((x) => x.id === rest.slice(9));
        if (!m) return json(res, 404, { error: { code: 404 } });
        return json(res, 200, { id: m.id, threadId: m.id, labelIds: m.labels, internalDate: String(m.date), payload: { headers: [{ name: "From", value: m.from }, { name: "Subject", value: m.subject }] } });
      }
      return json(res, 404, {});
    }
    // --- Google Drive (appDataFolder only, which is all Pip may touch) ---
    if (p.startsWith("/drive/v3/files") || p.startsWith("/upload/drive/v3/files")) {
      if (!authed(req)) return json(res, 401, { error: { code: 401, message: "Invalid Credentials" } });
      const upload = p.startsWith("/upload/");
      const idInPath = p.replace(/^\/(upload\/)?drive\/v3\/files\/?/, "");
      if (!upload && req.method === "GET" && !idInPath) {
        const q = url.searchParams.get("q") || "";
        const m = q.match(/name='([^']+)'/);
        const name = m ? m[1] : null;
        const files = Object.entries(appData)
          .filter(([n]) => !name || n === name)
          .map(([n, f]) => ({ id: f.id, name: n }));
        return json(res, 200, { files });
      }
      if (!upload && req.method === "GET" && url.searchParams.get("alt") === "media") {
        const f = Object.values(appData).find((x) => x.id === idInPath);
        if (!f) return json(res, 404, { error: { code: 404 } });
        res.writeHead(200, { "Content-Type": "application/octet-stream", "Content-Length": f.bytes.length });
        return res.end(f.bytes);
      }
      if (upload && (req.method === "POST" || req.method === "PATCH")) {
        const chunks = [];
        await new Promise((r) => { req.on("data", (c) => chunks.push(c)); req.on("end", r); });
        const raw = Buffer.concat(chunks);
        const text = raw.toString("latin1");
        const nameMatch = text.match(/"name":"([^"]+)"/);
        const name = nameMatch ? nameMatch[1] : "unnamed";
        // The payload is the part after the second header block, minus the trailing boundary.
        const marker = "application/octet-stream\r\n\r\n";
        const start = text.indexOf(marker) + marker.length;
        const end = text.lastIndexOf("\r\n--pipsyncboundary--");
        const bytes = raw.subarray(start, end);
        const id = appData[name] ? appData[name].id : "file" + ++driveSeq;
        appData[name] = { id, bytes };
        return json(res, 200, { id, name });
      }
      return json(res, 404, {});
    }
    if (p === "/_drive") return json(res, 200, Object.fromEntries(Object.entries(appData).map(([n, f]) => [n, f.bytes.length])));
    if (p === "/_drive_clear") { appData = {}; return json(res, 200, {}); }

    // --- Google Calendar ---
    if (p.startsWith("/calendar/v3/")) {
      if (!authed(req)) return json(res, 401, { error: { code: 401, message: "Invalid Credentials" } });
      const rest = p.slice("/calendar/v3/".length);
      if (rest === "users/me/calendarList") return json(res, 200, { items: calendars });
      if (rest === "users/me/calendarList/primary") return json(res, 200, { id: calendars.find((c) => c.primary).id });
      const m = rest.match(/^calendars\/(.+)\/events$/);
      if (m) {
        const id = decodeURIComponent(m[1]);
        const all = calEvents[id] || [];
        const offset = Number(url.searchParams.get("pageToken") || 0);
        const page = all.slice(offset, offset + 2);   // page size 2 to exercise paging
        const out = { items: page };
        if (offset + 2 < all.length) out.nextPageToken = String(offset + 2);
        return json(res, 200, out);
      }
      return json(res, 404, {});
    }
    if (p === "/_event" && req.method === "POST") {
      const d = JSON.parse(await body(req));
      const cal = d.calendarId || calendars[0].id;
      (calEvents[cal] = calEvents[cal] || []).push(d.event);
      return json(res, 200, { ok: true, count: calEvents[cal].length });
    }
    if (p === "/_deliver" && req.method === "POST") {
      const d = JSON.parse(await body(req));
      historyId += 7;
      const m = { id: "msg" + historyId, historyId, labels: d.labels || ["INBOX", "UNREAD"], from: d.from, subject: d.subject, date: Date.now() };
      messages.push(m);
      return json(res, 200, m);
    }
    if (p === "/_expire_access") { issued.clear(); access = "expired"; return json(res, 200, {}); }
    if (p === "/_expire_history") { minValidHistory = historyId; return json(res, 200, {}); }
    if (p === "/_revoke") { refreshValid = "revoked-" + Date.now(); return json(res, 200, {}); }
    if (p === "/_mode" && req.method === "POST") { Object.assign(mode, JSON.parse(await body(req))); return json(res, 200, mode); }
    if (p === "/revoke" && req.method === "POST") {
      const f = new URLSearchParams(await body(req));
      if (f.get("token") === refreshValid) refreshValid = "revoked-" + Date.now();
      revoked.push(f.get("token") ? "token" : "none");
      return json(res, 200, {});
    }
    if (p === "/_revoked") return json(res, 200, revoked);
    if (p === "/_log") return json(res, 200, log);
    json(res, 404, { error: "not found" });
  } catch (e) {
    json(res, 500, { error: String(e) });
  }
}).listen(port, "127.0.0.1", () => console.log("mock google on " + port));
