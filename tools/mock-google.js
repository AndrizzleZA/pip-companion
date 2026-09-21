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
//   GET  /_log                -> request log
"use strict";
const http = require("http");
const crypto = require("crypto");
const port = Number(process.argv[2] || 8787);

const CLIENT_ID = "123456789012-pipmocktestclientabcdefghijkl.apps.googleusercontent.com";
const CLIENT_SECRET = "GOCSPX-MockSecretForPipTests1234";
const SCOPE = "https://www.googleapis.com/auth/gmail.metadata";
let challenges = {};        // code -> { challenge, redirect }
let access = null;          // valid access token
let refreshValid = "RT-1";
let accessSeq = 0;
let historyId = 1000;
let minValidHistory = 0;
let messages = [];          // { id, historyId, labels, from, subject, date }
const log = [];

const b64url = (buf) => buf.toString("base64").replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
const json = (res, code, obj) => { const b = JSON.stringify(obj); res.writeHead(code, { "Content-Type": "application/json", "Content-Length": Buffer.byteLength(b) }); res.end(b); };
const body = (req) => new Promise((r) => { let d = ""; req.on("data", (c) => (d += c)); req.on("end", () => r(d)); });
const newAccess = () => (access = "AT-" + ++accessSeq);
const authed = (req) => req.headers.authorization === "Bearer " + access;

http.createServer(async (req, res) => {
  const url = new URL(req.url, "http://127.0.0.1:" + port);
  log.push(req.method + " " + url.pathname + url.search.replace(/(code|code_verifier|refresh_token|client_secret)=[^&]*/g, "$1=***"));
  const p = url.pathname;
  try {
    if (p === "/auth") {
      const q = url.searchParams;
      if (q.get("client_id") !== CLIENT_ID) return json(res, 400, { error: "invalid_client" });
      if (q.get("code_challenge_method") !== "S256" || !q.get("code_challenge")) return json(res, 400, { error: "pkce_required" });
      if (q.get("scope") !== SCOPE) return json(res, 400, { error: "unexpected_scope", scope: q.get("scope") });
      const code = "CODE-" + crypto.randomBytes(6).toString("hex");
      challenges[code] = { challenge: q.get("code_challenge"), redirect: q.get("redirect_uri") };
      res.writeHead(302, { Location: `${q.get("redirect_uri")}/?code=${encodeURIComponent(code)}&state=${encodeURIComponent(q.get("state"))}&scope=${encodeURIComponent(SCOPE)}` });
      return res.end();
    }
    if (p === "/token" && req.method === "POST") {
      const f = new URLSearchParams(await body(req));
      if (f.get("client_id") !== CLIENT_ID || f.get("client_secret") !== CLIENT_SECRET) return json(res, 401, { error: "invalid_client" });
      if (f.get("grant_type") === "authorization_code") {
        const c = challenges[f.get("code")];
        delete challenges[f.get("code")];
        if (!c) return json(res, 400, { error: "invalid_grant", error_description: "unknown code" });
        const expect = b64url(crypto.createHash("sha256").update(f.get("code_verifier") || "").digest());
        if (expect !== c.challenge) return json(res, 400, { error: "invalid_grant", error_description: "PKCE verifier mismatch" });
        if (f.get("redirect_uri") !== c.redirect) return json(res, 400, { error: "invalid_grant", error_description: "redirect mismatch" });
        return json(res, 200, { access_token: newAccess(), expires_in: 3599, refresh_token: refreshValid, scope: SCOPE, token_type: "Bearer" });
      }
      if (f.get("grant_type") === "refresh_token") {
        if (f.get("refresh_token") !== refreshValid) return json(res, 400, { error: "invalid_grant", error_description: "Token has been expired or revoked." });
        return json(res, 200, { access_token: newAccess(), expires_in: 3599, scope: SCOPE, token_type: "Bearer" });
      }
      return json(res, 400, { error: "unsupported_grant_type" });
    }
    if (p.startsWith("/gmail/v1/users/me/")) {
      if (!authed(req)) return json(res, 401, { error: { code: 401, message: "Invalid Credentials" } });
      const rest = p.slice("/gmail/v1/users/me/".length);
      if (rest === "profile") return json(res, 200, { emailAddress: "andre.test@gmail.com", messagesTotal: messages.length, historyId: String(historyId) });
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
    if (p === "/_deliver" && req.method === "POST") {
      const d = JSON.parse(await body(req));
      historyId += 7;
      const m = { id: "msg" + historyId, historyId, labels: d.labels || ["INBOX", "UNREAD"], from: d.from, subject: d.subject, date: Date.now() };
      messages.push(m);
      return json(res, 200, m);
    }
    if (p === "/_expire_access") { access = "expired"; return json(res, 200, {}); }
    if (p === "/_expire_history") { minValidHistory = historyId; return json(res, 200, {}); }
    if (p === "/_revoke") { refreshValid = "revoked-" + Date.now(); return json(res, 200, {}); }
    if (p === "/_log") return json(res, 200, log);
    json(res, 404, { error: "not found" });
  } catch (e) {
    json(res, 500, { error: String(e) });
  }
}).listen(port, "127.0.0.1", () => console.log("mock google on " + port));
