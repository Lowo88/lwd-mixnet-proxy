//! What a supervisor is told about a half that is still coming up.
//!
//! Startup is not instant and not reliable: the client has to find a gateway and register with it,
//! which was observed to fail outright on 2 of 15 attempts. A binary up/down answer cannot tell
//! "still registering" from "registered and broken", and those want different reactions from
//! whoever is watching.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// How far along a half is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// The process is up and the mixnet client is not registered yet.
    Starting,
    /// Registered with a gateway, so the half has a mixnet address, but not yet carrying traffic.
    Registered,
    /// Carrying traffic: the dialling half is accepting local connections, the listening half is
    /// accepting mixnet streams.
    Serving,
}

impl State {
    /// The name reported over HTTP.
    pub fn as_str(self) -> &'static str {
        match self {
            State::Starting => "starting",
            State::Registered => "registered",
            State::Serving => "serving",
        }
    }

    /// Whether a supervisor should be sending work here yet.
    pub fn is_ready(self) -> bool {
        matches!(self, State::Serving)
    }

    fn from_code(code: u8) -> Self {
        match code {
            1 => State::Registered,
            2 => State::Serving,
            _ => State::Starting,
        }
    }
}

/// The current state, shared between whoever advances it and whoever reports it.
#[derive(Debug, Clone, Default)]
pub struct Health {
    state: Arc<AtomicU8>,
}

impl Health {
    /// A half that has just started.
    pub fn starting() -> Self {
        Self::default()
    }

    pub fn state(&self) -> State {
        State::from_code(self.state.load(Ordering::Relaxed))
    }

    /// Move to `state`. States only ever move forward, so a late report of an earlier one is
    /// ignored rather than allowed to walk the half backwards.
    pub fn advance_to(&self, state: State) {
        let code = state as u8;
        self.state.fetch_max(code, Ordering::Relaxed);
    }

    /// The body served on the health endpoint.
    pub fn as_json(&self) -> String {
        format!("{{\"state\":\"{}\"}}\n", self.state().as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_half_starts_out_starting() {
        assert_eq!(Health::starting().state(), State::Starting);
    }

    #[test]
    fn only_a_serving_half_is_ready() {
        let health = Health::starting();
        health.advance_to(State::Registered);
        assert!(!health.state().is_ready());
    }

    #[test]
    fn advancing_reaches_the_state_asked_for() {
        let health = Health::starting();
        health.advance_to(State::Serving);
        assert_eq!(health.state(), State::Serving);
    }

    #[test]
    fn a_late_earlier_state_does_not_walk_a_serving_half_backwards() {
        let health = Health::starting();
        health.advance_to(State::Serving);
        health.advance_to(State::Registered);
        assert_eq!(health.state(), State::Serving);
    }

    #[test]
    fn the_body_names_the_state() {
        let health = Health::starting();
        health.advance_to(State::Registered);
        assert_eq!(health.as_json(), "{\"state\":\"registered\"}\n");
    }
}
