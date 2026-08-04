//! Short-lived, single-use tickets authorising one QUIC attach.
//!
//! The QUIC data plane had no authentication at all: `handle_stream` granted
//! full bidirectional PTY access on a session id alone. A client written
//! against the wire format, holding no credential, was able to exfiltrate the
//! scrollback and execute a command in the victim's shell.
//!
//! Why an opaque ticket rather than the account JWT:
//!
//! - The JWT lives 30 days. Putting it on the data plane turns any leak of a
//!   session id into a leak of the whole account.
//! - Revocation is instant here (drop the entry) and free.
//! - Single use is trivial to enforce server-side and impossible with a
//!   stateless signed token.
//! - It keeps `JWT_SECRET` off the data plane entirely.
//!
//! Tickets are in-memory: the hub is a single process, and a ticket that does
//! not survive a restart is exactly right — the client just asks for another.

use rand::RngCore;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Long enough that guessing is hopeless, short enough to fit a stream header.
const TICKET_BYTES: usize = 32;
/// The client redeems immediately after minting; a minute covers a slow network
/// without leaving a useful window for a stolen ticket.
const TICKET_TTL: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct TicketStore {
    inner: Arc<Mutex<HashMap<String, Ticket>>>,
}

struct Ticket {
    session_id: String,
    user_id: String,
    expires_at: Instant,
}

impl TicketStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Mint a ticket for a session the caller has already been authorised for.
    pub fn issue(&self, session_id: &str, user_id: &str) -> String {
        let mut raw = [0u8; TICKET_BYTES];
        rand::rng().fill_bytes(&mut raw);
        let token = hex_encode(&raw);

        let mut store = self.inner.lock().unwrap();
        Self::prune(&mut store);
        store.insert(
            token.clone(),
            Ticket {
                session_id: session_id.to_string(),
                user_id: user_id.to_string(),
                expires_at: Instant::now() + TICKET_TTL,
            },
        );
        token
    }

    /// Consume a ticket. Returns the user it was issued to, or None if it is
    /// unknown, expired, already used, or bound to a different session.
    pub fn redeem(&self, token: &str, session_id: &str) -> Option<String> {
        let mut store = self.inner.lock().unwrap();
        Self::prune(&mut store);
        let ticket = store.remove(token)?;
        if ticket.session_id != session_id || ticket.expires_at <= Instant::now() {
            return None;
        }
        Some(ticket.user_id)
    }

    fn prune(store: &mut HashMap<String, Ticket>) {
        let now = Instant::now();
        store.retain(|_, ticket| ticket.expires_at > now);
    }
}

impl Default for TicketStore {
    fn default() -> Self {
        Self::new()
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ticket_works_once() {
        let store = TicketStore::new();
        let token = store.issue("sess-1", "user-1");

        assert_eq!(store.redeem(&token, "sess-1").as_deref(), Some("user-1"));
        assert_eq!(
            store.redeem(&token, "sess-1"),
            None,
            "a redeemed ticket must not work a second time"
        );
    }

    #[test]
    fn a_ticket_is_bound_to_its_session() {
        let store = TicketStore::new();
        let token = store.issue("sess-1", "user-1");

        assert_eq!(
            store.redeem(&token, "sess-2"),
            None,
            "a ticket for one session must not open another"
        );
    }

    #[test]
    fn an_unknown_ticket_is_rejected() {
        let store = TicketStore::new();
        assert_eq!(store.redeem("deadbeef", "sess-1"), None);
    }

    #[test]
    fn tickets_are_unpredictable_and_full_length() {
        let store = TicketStore::new();
        let a = store.issue("s", "u");
        let b = store.issue("s", "u");
        assert_ne!(a, b);
        assert_eq!(a.len(), TICKET_BYTES * 2);
    }
}
