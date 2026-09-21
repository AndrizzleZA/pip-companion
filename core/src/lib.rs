//! pip-core — the platform-independent brain of Pip.
//!
//! Everything here is pure logic with no OS dependencies so the exact same code can run in the
//! Windows shell today and behind UniFFI bindings on Android/iOS later. The shells own windows,
//! timers and notifications; the core answers "what is due, may I show it now, and how does the
//! creature feel about it".
//!
//! Time handling: persisted timestamps are Unix seconds (UTC). Anything calendar-shaped (quiet
//! hours, active windows, weekdays) is evaluated in the local time zone passed in by the caller,
//! which keeps the engine testable with a fixed offset.

pub mod agenda;
pub mod calendar;
pub mod dates;
pub mod links;
pub mod messages;
pub mod mood;
pub mod schedule;
pub mod settings;
pub mod state;
pub mod time;

pub use mood::{mood, Mood};
pub use schedule::{Delivery, Due, ItemKey, Priority};
pub use settings::Settings;
pub use state::State;
