//! The catalogue of services Pip can connect to, described in plain language. Shells render
//! this list (Settings › Connections) and map each entry to how it connects: a browser sign-in
//! (OAuth) or a private subscription link. Adding a service = one entry here + its sync code.

use crate::links::Provider;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectorId {
    Gmail,
    GoogleCalendar,
    OutlookCalendar,
    ICloudCalendar,
    OtherCalendar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// "Sign in with …" in the browser; Pip gets a revocable, limited permission.
    SignIn,
    /// A private read-only calendar link, picked up when the user copies it.
    Link(Provider),
}

pub struct Connector {
    pub id: ConnectorId,
    pub name: &'static str,
    /// Short badge text and colour for the list (no brand artwork shipped).
    pub badge: &'static str,
    pub badge_rgb: u32,
    /// What Pip does with it, one line.
    pub purpose: &'static str,
    /// What Pip can see — and what it can't do. Shown before connecting.
    pub access: &'static str,
    pub method: Method,
}

pub const ALL: [Connector; 5] = [
    Connector {
        id: ConnectorId::Gmail,
        name: "Gmail",
        badge: "M",
        badge_rgb: 0xEA4335,
        purpose: "A gentle nudge when important email arrives",
        access: "Pip only sees who new emails are from and their subjects. It can\u{2019}t open, send or delete email.",
        method: Method::SignIn,
    },
    Connector {
        id: ConnectorId::GoogleCalendar,
        name: "Google Calendar",
        badge: "31",
        badge_rgb: 0x4285F4,
        purpose: "Reminders before your events",
        access: "Read-only: Pip can see your events but never change them.",
        method: Method::SignIn,
    },
    Connector {
        id: ConnectorId::OutlookCalendar,
        name: "Outlook Calendar",
        badge: "O",
        badge_rgb: 0x0F6CBD,
        purpose: "Reminders before your events",
        access: "Read-only: Pip can see your events but never change them.",
        method: Method::Link(Provider::Outlook),
    },
    Connector {
        id: ConnectorId::ICloudCalendar,
        name: "iCloud Calendar",
        badge: "i",
        badge_rgb: 0x8E8E93,
        purpose: "Reminders before your events",
        access: "Read-only: Pip can see your events but never change them.",
        method: Method::Link(Provider::ICloud),
    },
    Connector {
        id: ConnectorId::OtherCalendar,
        name: "Other calendar",
        badge: "+",
        badge_rgb: 0x6B7385,
        purpose: "Any calendar with a subscription (.ics / webcal) link",
        access: "Read-only: Pip can see your events but never change them.",
        method: Method::Link(Provider::Other),
    },
];

pub fn get(id: ConnectorId) -> &'static Connector {
    ALL.iter().find(|c| c.id == id).expect("every ConnectorId is in ALL")
}

/// Why a browser sign-in didn't complete, so the UI can say it plainly and offer the right fix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignInProblem {
    /// The user clicked Cancel/Deny in the browser (not an error).
    Declined,
    /// The sign-in finished without the permission Pip asked for.
    MissingPermission,
    /// Nothing came back from the browser in time.
    TimedOut,
    /// No internet / server unreachable.
    Offline(String),
    /// The service refused this copy of Pip (misconfigured or blocked client).
    AppNotAllowed,
    /// Anything else, with a short explanation.
    Other(String),
}

impl SignInProblem {
    /// Plain-language explanation shown under the "Try again" button.
    pub fn explain(&self, service: &str) -> String {
        match self {
            SignInProblem::Declined => "Nothing was connected. You can try again whenever you like.".into(),
            SignInProblem::MissingPermission => {
                format!("{service} didn\u{2019}t give Pip permission. Try again and choose Allow.")
            }
            SignInProblem::TimedOut => "Pip didn\u{2019}t hear back from your browser. If you closed the tab, just try again.".into(),
            // Technical details stay out of the UI (they're in the variant for logs/tests only).
            SignInProblem::Offline(_) => format!("Couldn\u{2019}t reach {service}. Check your internet connection and try again."),
            SignInProblem::AppNotAllowed => format!("{service} didn\u{2019}t accept this copy of Pip. Please update Pip, then try again."),
            SignInProblem::Other(_) => format!("Something went wrong connecting to {service}. Please try again."),
        }
    }

    /// Declining is the user's choice — show it calmly, not as an error.
    pub fn is_error(&self) -> bool {
        !matches!(self, SignInProblem::Declined)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_is_complete_and_plain() {
        for id in [ConnectorId::Gmail, ConnectorId::GoogleCalendar, ConnectorId::OutlookCalendar, ConnectorId::ICloudCalendar, ConnectorId::OtherCalendar] {
            let c = get(id);
            assert!(!c.name.is_empty() && !c.purpose.is_empty() && !c.access.is_empty());
            // No jargon in anything the user reads.
            for word in ["OAuth", "token", "API", "scope", "client"] {
                assert!(!c.access.contains(word) && !c.purpose.contains(word), "{word} in {}", c.name);
            }
        }
        assert_eq!(get(ConnectorId::Gmail).method, Method::SignIn);
    }

    #[test]
    fn problems_explain_themselves() {
        assert!(!SignInProblem::Declined.is_error());
        assert!(SignInProblem::TimedOut.is_error());
        assert!(SignInProblem::MissingPermission.explain("Google").contains("choose Allow"));
        assert!(SignInProblem::Offline("timed out".into()).explain("Google").contains("internet"));
    }
}
