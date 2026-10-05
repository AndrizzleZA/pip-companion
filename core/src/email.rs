//! Important-email detection. Platform independent: settings, the importance rules, header
//! decoding and parsing of Gmail API (metadata scope) responses. Networking and OAuth live in the
//! shells; nothing here ever sees a message body — the metadata scope doesn't provide one.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmailAccount {
    pub id: String,
    /// Filled in after sign-in (from the Gmail profile).
    pub address: String,
    pub enabled: bool,
    pub updated_at: i64,
}

impl Default for EmailAccount {
    fn default() -> Self {
        EmailAccount { id: String::new(), address: String::new(), enabled: true, updated_at: 0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmailSettings {
    pub accounts: Vec<EmailAccount>,
    /// "mom@example.com", "@work.co.za" (a whole domain) or a name like "Thandi".
    pub senders: Vec<String>,
    /// Words or phrases in the subject.
    pub keywords: Vec<String>,
    /// Also treat mail that Gmail itself marks Important as important.
    pub gmail_important: bool,
    /// Also treat starred mail as important. Note this can only catch a star that is already on
    /// the message when Pip first sees it — Pip looks at mail as it arrives, and never goes back
    /// over what it has already passed over.
    #[serde(default)]
    pub starred: bool,
    /// Minutes between checks while you're at the computer.
    pub check_minutes: u32,
}

impl Default for EmailSettings {
    fn default() -> Self {
        EmailSettings {
            accounts: Vec::new(),
            senders: Vec::new(),
            keywords: ["urgent", "action required", "interview", "final notice", "overdue"].iter().map(|s| s.to_string()).collect(),
            gmail_important: false,
            starred: false,
            check_minutes: 3,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EmailMsg {
    pub id: String,
    pub account_id: String,
    pub from_name: String,
    pub from_addr: String,
    pub subject: String,
    pub labels: Vec<String>,
    /// Unix seconds.
    pub received: i64,
}

impl EmailMsg {
    pub fn sender(&self) -> &str {
        if self.from_name.trim().is_empty() {
            &self.from_addr
        } else {
            &self.from_name
        }
    }
    fn has(&self, label: &str) -> bool {
        self.labels.iter().any(|l| l == label)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    Sender(String),
    Starred,
    Keyword(String),
    GmailImportant,
}

/// Is this (new) message important under the user's rules? Only unread inbox mail qualifies —
/// anything already read elsewhere, archived, sent by you, spam or trash is ignored.
pub fn important(s: &EmailSettings, m: &EmailMsg) -> Option<Reason> {
    if !m.has("INBOX") || !m.has("UNREAD") || m.has("SENT") || m.has("SPAM") || m.has("TRASH") {
        return None;
    }
    if let Some(rule) = matching_sender(s, &m.from_addr, &m.from_name) {
        return Some(Reason::Sender(rule));
    }
    // A star is something the user did on purpose, so it outranks a guess from a subject word.
    if s.starred && m.has("STARRED") {
        return Some(Reason::Starred);
    }
    let subject = m.subject.to_lowercase();
    for k in &s.keywords {
        let k = k.trim().to_lowercase();
        if !k.is_empty() && subject.contains(&k) {
            return Some(Reason::Keyword(k));
        }
    }
    if s.gmail_important && m.has("IMPORTANT") {
        return Some(Reason::GmailImportant);
    }
    None
}

/// Which of the user's sender rules covers this sender, if any. Shared by the importance check
/// and by "do they already always get through?", so the two can never disagree.
pub fn matching_sender(s: &EmailSettings, from_addr: &str, from_name: &str) -> Option<String> {
    let addr = from_addr.to_lowercase();
    let name = from_name.to_lowercase();
    for rule in &s.senders {
        let r = rule.trim().to_lowercase();
        if r.is_empty() {
            continue;
        }
        let hit = if let Some(domain) = r.strip_prefix('@') {
            addr.ends_with(&format!("@{domain}")) || addr.ends_with(&format!(".{domain}"))
        } else if r.contains('@') {
            addr == r
        } else {
            name.contains(&r) || addr.contains(&r)
        };
        if hit {
            return Some(rule.trim().to_string());
        }
    }
    None
}

/// Adds a sender to the always-important list, as a plain lowercase address. Does nothing if a
/// rule already covers them — including a whole-domain rule — so the list can't grow duplicates.
/// Returns true if the list changed.
pub fn always_from(s: &mut EmailSettings, from_addr: &str) -> bool {
    let addr = from_addr.trim().to_lowercase();
    if addr.is_empty() || !addr.contains('@') || matching_sender(s, &addr, "").is_some() {
        return false;
    }
    s.senders.push(addr);
    true
}

// ---------------------------------------------------------------------------------------------
// Header decoding
// ---------------------------------------------------------------------------------------------

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        })
    }
    let bytes: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=').collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut acc = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            acc |= val(*c)? << (18 - 6 * i);
        }
        out.push((acc >> 16) as u8);
        if chunk.len() > 2 {
            out.push((acc >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(acc as u8);
        }
    }
    Some(out)
}

/// Base64url without padding (JWT/PKCE style) — shared helper for shells.
pub fn b64url_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len() * 4 / 3 + 3);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(T[(n >> 6) as usize & 63] as char);
        }
        if chunk.len() > 2 {
            out.push(T[n as usize & 63] as char);
        }
    }
    out
}

fn decode_word(charset: &str, enc: &str, text: &str) -> Option<String> {
    let bytes = match enc {
        "B" | "b" => b64_decode(text)?,
        "Q" | "q" => {
            let mut v = Vec::new();
            let b = text.as_bytes();
            let mut i = 0;
            while i < b.len() {
                match b[i] {
                    b'_' => v.push(b' '),
                    b'=' if i + 2 < b.len() => {
                        v.push(u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).ok()?, 16).ok()?);
                        i += 2;
                    }
                    c => v.push(c),
                }
                i += 1;
            }
            v
        }
        _ => return None,
    };
    let cs = charset.to_ascii_lowercase();
    Some(if cs == "iso-8859-1" || cs == "latin1" || cs == "windows-1252" {
        bytes.iter().map(|&c| c as char).collect()
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

/// Decodes RFC 2047 encoded words (`=?UTF-8?B?...?=`), common in subjects and sender names.
pub fn decode_header(v: &str) -> String {
    let mut out = String::new();
    let mut rest = v;
    let mut last_was_word = false;
    while let Some(start) = rest.find("=?") {
        let (before, after) = rest.split_at(start);
        // Whitespace between two encoded words is dropped.
        if !(last_was_word && before.trim().is_empty()) {
            out.push_str(before);
        }
        let parts: Vec<&str> = after[2..].splitn(3, '?').collect();
        if parts.len() == 3 {
            if let Some(end) = parts[2].find("?=") {
                if let Some(text) = decode_word(parts[0], parts[1], &parts[2][..end]) {
                    out.push_str(&text);
                    let consumed = 2 + parts[0].len() + 1 + parts[1].len() + 1 + end + 2;
                    rest = &after[consumed..];
                    last_was_word = true;
                    continue;
                }
            }
        }
        out.push_str("=?");
        rest = &after[2..];
        last_was_word = false;
    }
    out.push_str(rest);
    out
}

/// Splits a From header into (display name, address).
pub fn parse_from(v: &str) -> (String, String) {
    let v = decode_header(v);
    if let (Some(lt), Some(gt)) = (v.rfind('<'), v.rfind('>')) {
        if lt < gt {
            let name = v[..lt].trim().trim_matches('"').trim().to_string();
            return (name, v[lt + 1..gt].trim().to_lowercase());
        }
    }
    (String::new(), v.trim().to_lowercase())
}

// ---------------------------------------------------------------------------------------------
// Gmail API (v1) — metadata scope
// ---------------------------------------------------------------------------------------------

pub mod gmail {
    use super::*;

    pub const SCOPE: &str = "https://www.googleapis.com/auth/gmail.metadata";
    pub const API: &str = "https://gmail.googleapis.com/gmail/v1/users/me";

    #[derive(Deserialize)]
    struct Profile {
        #[serde(rename = "emailAddress")]
        email: String,
        #[serde(rename = "historyId")]
        history_id: String,
    }

    /// (address, historyId) from users.getProfile.
    pub fn parse_profile(json: &str) -> Option<(String, String)> {
        let p: Profile = serde_json::from_str(json).ok()?;
        Some((p.email, p.history_id))
    }

    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct History {
        history: Vec<HistoryRec>,
        #[serde(rename = "nextPageToken")]
        next_page: Option<String>,
        #[serde(rename = "historyId")]
        history_id: Option<String>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct HistoryRec {
        #[serde(rename = "messagesAdded")]
        added: Vec<Added>,
    }
    #[derive(Deserialize)]
    struct Added {
        message: MsgRef,
    }
    #[derive(Deserialize)]
    struct MsgRef {
        id: String,
        #[serde(rename = "labelIds", default)]
        labels: Vec<String>,
    }

    pub struct HistoryPage {
        /// New message ids that landed in the inbox unread (the only ones worth fetching).
        pub added: Vec<String>,
        pub next_page: Option<String>,
        pub history_id: Option<String>,
    }

    pub fn parse_history(json: &str) -> Option<HistoryPage> {
        let h: History = serde_json::from_str(json).ok()?;
        let mut added: Vec<String> = Vec::new();
        for rec in h.history {
            for a in rec.added {
                let l = &a.message.labels;
                let inbox_unread = l.iter().any(|x| x == "INBOX") && l.iter().any(|x| x == "UNREAD");
                if inbox_unread && !added.contains(&a.message.id) {
                    added.push(a.message.id);
                }
            }
        }
        Some(HistoryPage { added, next_page: h.next_page, history_id: h.history_id })
    }

    #[derive(Deserialize)]
    struct Message {
        id: String,
        #[serde(rename = "labelIds", default)]
        labels: Vec<String>,
        #[serde(rename = "internalDate", default)]
        internal_date: String,
        #[serde(default)]
        payload: Payload,
    }
    #[derive(Deserialize, Default)]
    struct Payload {
        #[serde(default)]
        headers: Vec<Header>,
    }
    #[derive(Deserialize)]
    struct Header {
        name: String,
        value: String,
    }

    pub fn parse_message(json: &str, account_id: &str) -> Option<EmailMsg> {
        let m: Message = serde_json::from_str(json).ok()?;
        let header = |n: &str| m.payload.headers.iter().find(|h| h.name.eq_ignore_ascii_case(n)).map(|h| h.value.as_str());
        let (from_name, from_addr) = parse_from(header("From").unwrap_or(""));
        let subject = decode_header(header("Subject").unwrap_or("")).trim().to_string();
        Some(EmailMsg {
            id: m.id,
            account_id: account_id.to_string(),
            from_name,
            from_addr,
            subject: if subject.is_empty() { "(no subject)".into() } else { subject },
            labels: m.labels,
            received: m.internal_date.parse::<i64>().map(|ms| ms / 1000).unwrap_or(0),
        })
    }

    pub fn profile_path(api: &str) -> String {
        format!("{api}/profile")
    }

    pub fn history_path(api: &str, start: &str, page: Option<&str>) -> String {
        let mut p = format!("{api}/history?startHistoryId={start}&historyTypes=messageAdded&labelId=INBOX&maxResults=100");
        if let Some(t) = page {
            p.push_str("&pageToken=");
            p.push_str(t);
        }
        p
    }

    pub fn message_path(api: &str, id: &str) -> String {
        format!("{api}/messages/{id}?format=metadata&metadataHeaders=From&metadataHeaders=Subject")
    }

    /// Web link that opens the message (or the inbox) in the right account.
    pub fn web_link(address: &str, id: Option<&str>) -> String {
        match id {
            Some(id) => format!("https://mail.google.com/mail/?authuser={address}#inbox/{id}"),
            None => format!("https://mail.google.com/mail/?authuser={address}#inbox"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_star_counts_as_important_only_when_asked() {
        let mut s = EmailSettings { senders: Vec::new(), keywords: Vec::new(), ..Default::default() };
        let starred = msg("Thandi <thandi@example.com>", "Lunch?", &["INBOX", "UNREAD", "STARRED"]);

        assert_eq!(important(&s, &starred), None, "off by default");
        s.starred = true;
        assert_eq!(important(&s, &starred), Some(Reason::Starred));

        // The rest of the rules still apply: starring something already read changes nothing.
        let read = msg("Thandi <thandi@example.com>", "Lunch?", &["INBOX", "STARRED"]);
        assert_eq!(important(&s, &read), None, "already read");
        let archived = msg("Thandi <thandi@example.com>", "Lunch?", &["UNREAD", "STARRED"]);
        assert_eq!(important(&s, &archived), None, "not in the inbox");
        let unstarred = msg("Thandi <thandi@example.com>", "Lunch?", &["INBOX", "UNREAD"]);
        assert_eq!(important(&s, &unstarred), None, "nothing to go on");
    }

    #[test]
    fn always_from_this_sender_adds_the_address_once() {
        let mut s = EmailSettings { senders: Vec::new(), keywords: Vec::new(), ..Default::default() };

        assert!(always_from(&mut s, "Accounts@Example.com"));
        assert_eq!(s.senders, vec!["accounts@example.com"], "stored lowercase");

        // Asking again, in any casing, changes nothing.
        assert!(!always_from(&mut s, "accounts@example.com"));
        assert!(!always_from(&mut s, "ACCOUNTS@EXAMPLE.COM"));
        assert_eq!(s.senders.len(), 1);

        // Nor does it pile on top of a rule that already covers them.
        s.senders = vec!["@work.co.za".into()];
        assert!(!always_from(&mut s, "boss@work.co.za"));
        assert_eq!(s.senders.len(), 1);

        // And it refuses anything that isn't an address.
        assert!(!always_from(&mut s, "Thandi"));
        assert!(!always_from(&mut s, "  "));
        assert_eq!(s.senders.len(), 1);

        // Once added, that sender is important whatever else is off.
        s.senders = Vec::new();
        always_from(&mut s, "mom@example.com");
        let m = msg("Mom <mom@example.com>", "nothing in particular", &["INBOX", "UNREAD"]);
        assert_eq!(important(&s, &m), Some(Reason::Sender("mom@example.com".into())));
    }

    #[test]
    fn the_button_knows_when_a_sender_is_already_covered() {
        let s = EmailSettings { senders: vec!["@work.co.za".into(), "mom@example.com".into()], ..Default::default() };
        assert!(matching_sender(&s, "boss@work.co.za", "The Boss").is_some());
        assert!(matching_sender(&s, "MOM@example.com", "Mom").is_some(), "casing doesn't matter");
        assert!(matching_sender(&s, "stranger@elsewhere.com", "Someone").is_none());
    }

    fn msg(from: &str, subject: &str, labels: &[&str]) -> EmailMsg {
        let (from_name, from_addr) = parse_from(from);
        EmailMsg { id: "1".into(), account_id: "a".into(), from_name, from_addr, subject: subject.into(), labels: labels.iter().map(|s| s.to_string()).collect(), received: 0 }
    }

    #[test]
    fn rules() {
        let mut s = EmailSettings::default();
        s.senders = vec!["mom@example.com".into(), "@ombud.co.za".into(), "Thandi".into()];
        let inbox = &["INBOX", "UNREAD"];
        assert_eq!(important(&s, &msg("Mom <Mom@Example.com>", "Sunday", inbox)), Some(Reason::Sender("mom@example.com".into())));
        assert!(important(&s, &msg("Case Officer <frans@ombud.co.za>", "Your complaint", inbox)).is_some());
        assert!(important(&s, &msg("Thandi Mokoena <t@m.com>", "hi", inbox)).is_some());
        assert_eq!(important(&s, &msg("Shop <deals@shop.com>", "URGENT: final notice on your account", inbox)), Some(Reason::Keyword("urgent".into())));
        assert_eq!(important(&s, &msg("Shop <deals@shop.com>", "50% off", inbox)), None);
        // Already read / archived / Gmail-important-only.
        assert_eq!(important(&s, &msg("Mom <mom@example.com>", "x", &["INBOX"])), None);
        assert_eq!(important(&s, &msg("x <x@y.com>", "x", &["INBOX", "UNREAD", "IMPORTANT"])), None);
        s.gmail_important = true;
        assert_eq!(important(&s, &msg("x <x@y.com>", "x", &["INBOX", "UNREAD", "IMPORTANT"])), Some(Reason::GmailImportant));
        // Look-alike domains don't match.
        assert_eq!(important(&s, &msg("x <x@notombud.co.za>", "x", inbox)), None);
    }

    #[test]
    fn header_decoding() {
        assert_eq!(decode_header("=?UTF-8?B?SGnDqSB0aGVyZQ==?="), "Hié there");
        assert_eq!(decode_header("=?utf-8?Q?Caf=C3=A9_menu?= today"), "Café menu today");
        assert_eq!(decode_header("=?UTF-8?B?QQ==?= =?UTF-8?B?Qg==?="), "AB");
        assert_eq!(decode_header("plain =? not encoded"), "plain =? not encoded");
        assert_eq!(parse_from("\"Andre Kruse\" <Andre@Example.com>"), ("Andre Kruse".into(), "andre@example.com".into()));
        assert_eq!(parse_from("no-reply@x.com"), (String::new(), "no-reply@x.com".into()));
        assert_eq!(b64url_encode(b"hello?"), "aGVsbG8_");
    }

    #[test]
    fn gmail_parsing() {
        assert_eq!(gmail::parse_profile(r#"{"emailAddress":"a@gmail.com","messagesTotal":5,"historyId":"123"}"#), Some(("a@gmail.com".into(), "123".into())));
        let h = gmail::parse_history(
            r#"{"history":[{"id":"1","messagesAdded":[{"message":{"id":"m1","threadId":"t","labelIds":["UNREAD","INBOX"]}},{"message":{"id":"m2","labelIds":["SENT"]}}]}],"historyId":"130"}"#,
        )
        .unwrap();
        assert_eq!(h.added, vec!["m1".to_string()]);
        assert_eq!(h.history_id.as_deref(), Some("130"));
        let empty = gmail::parse_history(r#"{"historyId":"130"}"#).unwrap();
        assert!(empty.added.is_empty());
        let m = gmail::parse_message(
            r#"{"id":"m1","threadId":"t","labelIds":["INBOX","UNREAD"],"internalDate":"1789990000000","payload":{"headers":[{"name":"From","value":"=?UTF-8?Q?Th=C3=A9o?= <theo@x.com>"},{"name":"Subject","value":"Dinner?"}]}}"#,
            "acc",
        )
        .unwrap();
        assert_eq!((m.from_name.as_str(), m.subject.as_str(), m.received), ("Théo", "Dinner?", 1_789_990_000));
    }
}
