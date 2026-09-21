//! Recognises calendar subscription links in pasted/copied text, and explains the common
//! mistakes (the public address of a private Google calendar, Outlook's HTML link, ...).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Google,
    Outlook,
    ICloud,
    Other,
}

impl Provider {
    pub fn label(self) -> &'static str {
        match self {
            Provider::Google => "Google Calendar",
            Provider::Outlook => "Outlook",
            Provider::ICloud => "iCloud",
            Provider::Other => "another calendar",
        }
    }

    /// "your Google calendar", "your iCloud calendar", "the calendar".
    pub fn your_calendar(self) -> String {
        match self {
            Provider::Google => "your Google calendar".into(),
            Provider::Outlook => "your Outlook calendar".into(),
            Provider::ICloud => "your iCloud calendar".into(),
            Provider::Other => "that calendar".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkCheck {
    /// A usable subscription link (normalised).
    Calendar { url: String, provider: Provider },
    /// Looks like a calendar link, but the wrong one — with a plain-language fix.
    Mistake(String),
    /// Nothing calendar-like here.
    NotALink,
}

/// Pulls the first http(s)/webcal URL out of arbitrary copied text.
fn first_url(text: &str) -> Option<&str> {
    text.split_whitespace()
        .map(|t| t.trim_matches(|c: char| matches!(c, '"' | '\'' | '<' | '>' | '(' | ')' | ',' | ';')))
        .find(|t| {
            let l = t.to_ascii_lowercase();
            l.starts_with("https://") || l.starts_with("http://") || l.starts_with("webcal://") || l.starts_with("webcals://")
        })
}

fn host_and_path(url: &str) -> (String, String) {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let (host, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((rest, "/".into()));
    (host.to_ascii_lowercase(), path)
}

/// Checks copied text for a calendar link. `lenient` (a link pasted on purpose, "Other" provider)
/// also accepts unrecognised https links; the clipboard watcher only accepts confident matches.
pub fn check(text: &str, lenient: bool) -> LinkCheck {
    let Some(raw) = first_url(text) else { return LinkCheck::NotALink };
    let (host, path) = host_and_path(raw);
    let lower_path = path.to_ascii_lowercase();
    let known = host.ends_with("google.com") || host.contains("outlook.") || host.ends_with("icloud.com");
    // Big providers serve everything over https; quietly upgrade instead of refusing.
    let url = match raw.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") && known => format!("https://{rest}"),
        Some((scheme, _)) if scheme.eq_ignore_ascii_case("http") => {
            return LinkCheck::Mistake("That link isn't secure (http). Pip only follows https calendar links.".into());
        }
        _ => raw.to_string(),
    };

    if host == "calendar.google.com" || host == "www.google.com" {
        if lower_path.contains("/ical/") {
            if lower_path.contains("/private-") {
                return LinkCheck::Calendar { url, provider: Provider::Google };
            }
            if lower_path.contains("/public/") {
                // Holiday/"other" calendars are public by design; a personal calendar's public
                // address only works if the whole calendar was made public.
                if lower_path.contains("group.v.calendar.google.com") || lower_path.contains("holiday") {
                    return LinkCheck::Calendar { url, provider: Provider::Google };
                }
                return LinkCheck::Mistake(
                    "That's the public address. For your own calendar, copy \"Secret address in iCal format\" instead (a bit further down).".into(),
                );
            }
        }
        return LinkCheck::Mistake("That's a link to the Google Calendar website. Copy \"Secret address in iCal format\" under Integrate calendar.".into());
    }
    if host.contains("outlook.") || host.contains("office365.com") {
        if lower_path.ends_with(".ics") {
            return LinkCheck::Calendar { url, provider: Provider::Outlook };
        }
        if lower_path.ends_with(".html") {
            return LinkCheck::Mistake("That's the HTML link. Copy the ICS link just below it.".into());
        }
        return if lenient { LinkCheck::Calendar { url, provider: Provider::Outlook } } else { LinkCheck::NotALink };
    }
    if host.ends_with("icloud.com") {
        if lower_path.contains("/published/") {
            return LinkCheck::Calendar { url, provider: Provider::ICloud };
        }
        return LinkCheck::Mistake("That's not the calendar's share link. Turn on \"Public Calendar\" and use Copy Link.".into());
    }
    let is_webcal = url.to_ascii_lowercase().starts_with("webcal");
    if is_webcal || lower_path.ends_with(".ics") || lower_path.contains(".ics?") || lenient {
        return LinkCheck::Calendar { url, provider: Provider::Other };
    }
    LinkCheck::NotALink
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(t: &str) -> Option<Provider> {
        match check(t, false) {
            LinkCheck::Calendar { provider, .. } => Some(provider),
            _ => None,
        }
    }

    #[test]
    fn recognises_providers() {
        assert_eq!(provider("https://calendar.google.com/calendar/ical/me%40gmail.com/private-0123abc/basic.ics"), Some(Provider::Google));
        assert_eq!(provider("https://calendar.google.com/calendar/ical/en.sa%23holiday%40group.v.calendar.google.com/public/basic.ics"), Some(Provider::Google));
        assert_eq!(provider("https://outlook.live.com/owa/calendar/00000000-0000/abcd/cid-1234/calendar.ics"), Some(Provider::Outlook));
        assert_eq!(provider("webcal://p42-caldav.icloud.com/published/2/MTIzNDU2"), Some(Provider::ICloud));
        assert_eq!(provider("  Here: webcal://example.org/team.ics  "), Some(Provider::Other));
    }

    #[test]
    fn explains_mistakes() {
        assert!(matches!(check("https://calendar.google.com/calendar/ical/me%40gmail.com/public/basic.ics", false), LinkCheck::Mistake(m) if m.contains("Secret address")));
        assert!(matches!(check("https://calendar.google.com/calendar/embed?src=me%40gmail.com", false), LinkCheck::Mistake(_)));
        assert!(matches!(check("https://outlook.live.com/owa/calendar/x/y/calendar.html", false), LinkCheck::Mistake(m) if m.contains("ICS")));
        assert!(matches!(check("http://example.org/cal.ics", false), LinkCheck::Mistake(_)));
    }

    #[test]
    fn upgrades_known_http_and_ignores_noise() {
        match check("http://calendar.google.com/calendar/ical/a/private-b/basic.ics", false) {
            LinkCheck::Calendar { url, .. } => assert!(url.starts_with("https://")),
            other => panic!("{other:?}"),
        }
        assert_eq!(check("just some text I copied", false), LinkCheck::NotALink);
        assert_eq!(check("https://news.example.com/story", false), LinkCheck::NotALink);
        assert!(matches!(check("https://my.server/calendar-feed", true), LinkCheck::Calendar { .. }));
    }
}
