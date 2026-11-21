//! Keeps terminal query replies in the order the queries were asked.
//!
//! The daemon answers cursor-position, status and device-attribute queries
//! itself, the moment it parses them. Colour queries (OSC 10/11/12 with `?`)
//! are answered by the attached client's terminal widget, a round trip later.
//! A program that asks "background colour?" and then "cursor position?" and
//! treats the second reply as the end of the answers would see them in the
//! wrong order, give up on the colour, and later read the late colour reply
//! as if it were keyboard input.
//!
//! [`ReplyHold`] is the per-pane state that prevents this: while a colour
//! query is waiting for the client, daemon replies generated after it are
//! held and released once the client's reply has been written to the PTY, or
//! when a deadline passes without one.

use std::time::{Duration, Instant};

/// How long daemon replies wait for the client to answer a colour query.
///
/// Long enough for a round trip through a busy GTK main loop, short enough
/// that a program is not left hanging when the client never answers.
pub const CLIENT_REPLY_TIMEOUT: Duration = Duration::from_millis(250);

/// Number of colour queries in one OSC sequence that the client's terminal
/// widget answers, given the sequence's `;`-separated parameters.
///
/// `OSC 10 ; ? ST` asks one question; `OSC 10 ; ? ; ? ST` asks for the
/// foreground and the background in one sequence.
#[must_use]
pub fn client_answered_queries(osc_params: &[&[u8]]) -> u32 {
    let Some((code, values)) = osc_params.split_first() else { return 0 };
    if !matches!(*code, b"10" | b"11" | b"12") {
        return 0;
    }
    let asked = values.iter().filter(|value| **value == b"?").count();
    u32::try_from(asked).unwrap_or(u32::MAX)
}

/// Number of colour replies (`OSC 10/11/12 ; <colour>`) in bytes a client
/// sent as raw terminal input.
#[must_use]
pub fn count_client_color_replies(input: &[u8]) -> u32 {
    let mut count = 0u32;
    let mut rest = input;
    while let Some(pos) = rest.windows(2).position(|pair| pair == b"\x1b]") {
        rest = &rest[pos + 2..];
        if rest.len() >= 3 && rest[0] == b'1' && matches!(rest[1], b'0'..=b'2') && rest[2] == b';' {
            count = count.saturating_add(1);
        }
    }
    count
}

/// What the deadline timer should do next.
#[derive(Debug, PartialEq, Eq)]
pub enum HoldExpiry {
    /// Nothing is waiting any more; the timer can stop.
    Idle,
    /// The client still has time to answer; check again at this instant.
    Wait(Instant),
    /// The deadline passed. Write these replies now (possibly none).
    Released(Vec<Vec<u8>>),
}

/// Result of routing one batch of daemon replies.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Routed {
    /// Replies to write to the PTY immediately.
    pub write_now: Vec<Vec<u8>>,
    /// True when this batch started a wait, so a deadline timer is needed.
    pub wait_started: bool,
}

/// Per-pane ordering state for daemon replies. See the module docs.
#[derive(Debug, Default)]
pub struct ReplyHold {
    /// Colour queries forwarded to the client and not answered yet.
    outstanding: u32,
    /// Daemon replies generated after an outstanding colour query.
    held: Vec<Vec<u8>>,
    /// When the wait gives up. `Some` exactly while `outstanding > 0`.
    deadline: Option<Instant>,
}

impl ReplyHold {
    /// Route the daemon replies produced by one parsed batch of output.
    ///
    /// `replies` are in query order. `replies_before_query` is how many of
    /// them were generated before the first colour query of the batch, and
    /// `client_queries` how many colour queries the batch contained.
    /// `client_can_answer` is false when no client is driving the pane, in
    /// which case nobody will answer and nothing is held.
    pub fn route(
        &mut self,
        mut replies: Vec<Vec<u8>>,
        replies_before_query: usize,
        client_queries: u32,
        client_can_answer: bool,
        now: Instant,
    ) -> Routed {
        if !client_can_answer {
            // Nobody will answer; anything still held goes out first.
            let mut write_now = self.release();
            write_now.append(&mut replies);
            return Routed { write_now, wait_started: false };
        }
        let was_waiting = self.outstanding > 0;
        if client_queries == 0 && !was_waiting {
            return Routed { write_now: replies, wait_started: false };
        }
        // With a query already outstanding every reply of this batch comes
        // after it; otherwise only those after the batch's first query do.
        let keep = if was_waiting { 0 } else { replies_before_query.min(replies.len()) };
        self.held.extend(replies.drain(keep..));
        if client_queries > 0 {
            self.outstanding = self.outstanding.saturating_add(client_queries);
            self.deadline = Some(now + CLIENT_REPLY_TIMEOUT);
        }
        Routed { write_now: replies, wait_started: !was_waiting }
    }

    /// The client's answers to `count` colour queries were written to the PTY.
    ///
    /// Returns the held daemon replies once every outstanding query has been
    /// answered; they must be written right after the client's bytes.
    pub fn client_replied(&mut self, count: u32) -> Vec<Vec<u8>> {
        if self.outstanding == 0 {
            return Vec::new();
        }
        self.outstanding = self.outstanding.saturating_sub(count);
        if self.outstanding == 0 { self.release() } else { Vec::new() }
    }

    /// Advance the deadline timer.
    pub fn expire(&mut self, now: Instant) -> HoldExpiry {
        match self.deadline {
            None => HoldExpiry::Idle,
            Some(deadline) if now < deadline => HoldExpiry::Wait(deadline),
            Some(_) => HoldExpiry::Released(self.release()),
        }
    }

    /// Whether a colour query is still waiting for the client.
    #[must_use]
    pub const fn is_waiting(&self) -> bool {
        self.outstanding > 0
    }

    fn release(&mut self) -> Vec<Vec<u8>> {
        self.outstanding = 0;
        self.deadline = None;
        std::mem::take(&mut self.held)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CPR: &[u8] = b"\x1b[1;1R";
    const DA1: &[u8] = b"\x1b[?64;1;2;6;22c";

    fn replies(items: &[&[u8]]) -> Vec<Vec<u8>> {
        items.iter().map(|item| item.to_vec()).collect()
    }

    #[test]
    fn colour_queries_are_recognised_per_question() {
        assert_eq!(client_answered_queries(&[b"11", b"?"]), 1);
        assert_eq!(client_answered_queries(&[b"10", b"?", b"?"]), 2);
        assert_eq!(client_answered_queries(&[b"12", b"?"]), 1);
        // Setting a colour, a title, or a palette entry asks nothing.
        assert_eq!(client_answered_queries(&[b"11", b"#000000"]), 0);
        assert_eq!(client_answered_queries(&[b"0", b"?"]), 0);
        assert_eq!(client_answered_queries(&[b"4", b"1", b"?"]), 0);
        assert_eq!(client_answered_queries(&[]), 0);
    }

    #[test]
    fn colour_replies_are_counted_in_client_input() {
        assert_eq!(count_client_color_replies(b"\x1b]11;rgb:0000/0000/0000\x1b\\"), 1);
        assert_eq!(
            count_client_color_replies(b"\x1b]10;rgb:ffff/ffff/ffff\x07\x1b]11;rgb:0/0/0\x07"),
            2
        );
        // Keystrokes, cursor keys and other OSC traffic are not replies.
        assert_eq!(count_client_color_replies(b"ls -la\r"), 0);
        assert_eq!(count_client_color_replies(b"\x1b[A\x1b[1;5C"), 0);
        assert_eq!(count_client_color_replies(b"\x1b]52;c;aGk=\x07"), 0);
        assert_eq!(count_client_color_replies(b"\x1b]1"), 0);
    }

    /// The gh / termenv probe: "background colour?" then "cursor position?".
    /// The cursor reply must wait for the client's colour reply.
    #[test]
    fn reply_after_colour_query_waits_for_the_client() {
        let mut hold = ReplyHold::default();
        let now = Instant::now();

        let routed = hold.route(replies(&[CPR]), 0, 1, true, now);
        assert_eq!(routed, Routed { write_now: Vec::new(), wait_started: true });
        assert!(hold.is_waiting());

        assert_eq!(hold.client_replied(1), replies(&[CPR]));
        assert!(!hold.is_waiting());
        assert_eq!(hold.expire(now + CLIENT_REPLY_TIMEOUT), HoldExpiry::Idle);
    }

    #[test]
    fn reply_asked_before_the_colour_query_is_not_delayed() {
        let mut hold = ReplyHold::default();
        let routed = hold.route(replies(&[CPR, DA1]), 1, 1, true, Instant::now());
        assert_eq!(routed, Routed { write_now: replies(&[CPR]), wait_started: true });
        assert_eq!(hold.client_replied(1), replies(&[DA1]));
    }

    #[test]
    fn replies_without_a_colour_query_pass_straight_through() {
        let mut hold = ReplyHold::default();
        let routed = hold.route(replies(&[CPR, DA1]), 2, 0, true, Instant::now());
        assert_eq!(routed, Routed { write_now: replies(&[CPR, DA1]), wait_started: false });
        assert!(!hold.is_waiting());
    }

    /// The sentinel query may arrive in a later read than the colour query.
    #[test]
    fn reply_in_a_later_batch_is_still_held() {
        let mut hold = ReplyHold::default();
        let now = Instant::now();
        assert!(hold.route(Vec::new(), 0, 1, true, now).wait_started);

        let routed = hold.route(replies(&[CPR]), 1, 0, true, now);
        assert_eq!(routed, Routed { write_now: Vec::new(), wait_started: false });
        assert_eq!(hold.client_replied(1), replies(&[CPR]));
    }

    /// Foreground and background asked together: the sentinel reply follows
    /// the second colour reply, not the first.
    #[test]
    fn every_colour_query_must_be_answered_before_release() {
        let mut hold = ReplyHold::default();
        hold.route(replies(&[DA1]), 0, 2, true, Instant::now());
        assert!(hold.client_replied(1).is_empty());
        assert_eq!(hold.client_replied(1), replies(&[DA1]));
    }

    #[test]
    fn deadline_releases_replies_when_the_client_never_answers() {
        let mut hold = ReplyHold::default();
        let now = Instant::now();
        hold.route(replies(&[CPR]), 0, 1, true, now);

        let deadline = now + CLIENT_REPLY_TIMEOUT;
        assert_eq!(hold.expire(now), HoldExpiry::Wait(deadline));
        assert_eq!(hold.expire(deadline), HoldExpiry::Released(replies(&[CPR])));
        assert!(!hold.is_waiting());
        // A reply that turns up after the deadline releases nothing twice.
        assert!(hold.client_replied(1).is_empty());
        assert_eq!(hold.expire(deadline), HoldExpiry::Idle);
    }

    #[test]
    fn nothing_is_held_when_no_client_can_answer() {
        let mut hold = ReplyHold::default();
        let routed = hold.route(replies(&[CPR]), 0, 1, false, Instant::now());
        assert_eq!(routed, Routed { write_now: replies(&[CPR]), wait_started: false });
        assert!(!hold.is_waiting());
    }

    /// The driving client went away mid-wait: held replies go out, in order,
    /// with the next batch instead of waiting for the deadline.
    #[test]
    fn held_replies_are_flushed_once_no_client_can_answer() {
        let mut hold = ReplyHold::default();
        let now = Instant::now();
        hold.route(replies(&[CPR]), 0, 1, true, now);
        let routed = hold.route(replies(&[DA1]), 1, 0, false, now);
        assert_eq!(routed, Routed { write_now: replies(&[CPR, DA1]), wait_started: false });
        assert_eq!(hold.expire(now), HoldExpiry::Idle);
    }

    #[test]
    fn stray_client_reply_without_a_pending_query_is_ignored() {
        let mut hold = ReplyHold::default();
        assert!(hold.client_replied(3).is_empty());
        assert!(!hold.is_waiting());
    }
}
