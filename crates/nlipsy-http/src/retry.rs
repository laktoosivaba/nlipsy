//! `Retry-After`, in both directions, with the clock left to the caller.
//!
//! RFC 9110 §10.2.3 gives the header two forms — `delay-seconds`, a count, and an
//! `HTTP-date`, an instant — and an end-point may send either. Writing it is therefore
//! a choice (this binding writes seconds) and reading it is not (a caller must
//! understand both).
//!
//! Neither function here asks what time it is. [`parse_retry_after`] takes `now` as an
//! argument, which is what keeps this crate free of a clock: `SystemTime::now()` is a
//! host call that `wasm32-unknown-unknown` does not have, and a rule that reads the
//! clock cannot be tested against a fixed date. The caller supplies the instant it
//! already knows.

use std::time::{Duration, SystemTime};

use http::HeaderValue;

/// `Retry-After` for a delay this end-point is asking for: whole seconds, floored at 1.
///
/// Floored rather than rounded, and never zero: a header saying "come back in zero
/// seconds" is an invitation to a tight loop, and the sub-second part of a delay a
/// dependency named is below what the header can express anyway.
///
/// Seconds rather than a date, because a count needs no agreement about clocks between
/// two machines. A peer must still read both forms — see [`parse_retry_after`].
pub fn retry_after_value(delay: Duration) -> HeaderValue {
    let seconds = delay.as_secs().max(1);
    // Cannot fail: the string is ASCII digits. The fallback is the same floor rather
    // than a panic, because a header this end-point could not write is not worth
    // taking an exchange down for.
    HeaderValue::from_str(&seconds.to_string()).unwrap_or_else(|_| HeaderValue::from_static("1"))
}

/// Read a peer's `Retry-After` as a delay from `now`.
///
/// RFC 9110 §10.2.3, both forms:
///
/// * `delay-seconds` — "120" — is the delay as it stands.
/// * `HTTP-date` — "Wed, 21 Oct 2015 07:28:00 GMT" — is an instant, and the delay is
///   how far it is from `now`. A date already past yields [`Duration::ZERO`] rather
///   than `None`: the end-point did answer the question, and the answer is "now".
///
/// Anything else is `None`. That includes an empty value, a negative count and a date
/// in a format the grammar does not admit — a caller that gets `None` has learned that
/// the end-point said nothing usable, not that it said zero.
///
/// `now` is a parameter and not a call, which is what lets this crate keep no clock;
/// a native binding passes [`SystemTime::now`].
pub fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let when = httpdate::parse_http_date(value).ok()?;
    // `duration_since` is an `Err` exactly when `when` is before `now`, and the error
    // carries the distance backwards — which is not a delay. The peer named a moment
    // that has arrived.
    Some(when.duration_since(now).unwrap_or(Duration::ZERO))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Wed, 21 Oct 2015 07:28:00 GMT", the example RFC 9110 §10.2.3 prints.
    const RFC_EXAMPLE: &str = "Wed, 21 Oct 2015 07:28:00 GMT";

    fn at(unix_seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(unix_seconds)
    }

    #[test]
    fn a_delay_is_written_as_whole_seconds_and_never_as_zero() {
        assert_eq!(retry_after_value(Duration::from_secs(12)), "12");
        // Floored, not rounded: 1500 ms is one second to wait, not two.
        assert_eq!(retry_after_value(Duration::from_millis(1500)), "1");
        // The floor. A dependency that said "immediately" is not an invitation to spin.
        assert_eq!(retry_after_value(Duration::ZERO), "1");
        assert_eq!(retry_after_value(Duration::from_millis(1)), "1");
    }

    #[test]
    fn delay_seconds_is_read_as_it_stands() {
        let now = at(1_000);
        assert_eq!(
            parse_retry_after("7", now),
            Some(Duration::from_secs(7)),
            "the form this binding itself writes"
        );
        // Header values arrive without their optional whitespace in a conforming
        // parser, and are trimmed here anyway rather than refused.
        assert_eq!(
            parse_retry_after("  7  ", now),
            Some(Duration::from_secs(7))
        );
        assert_eq!(parse_retry_after("0", now), Some(Duration::ZERO));
    }

    #[test]
    fn an_http_date_becomes_the_distance_from_the_instant_the_caller_supplied() {
        // 1445412480 is the RFC's example date as a Unix time.
        let when = 1_445_412_480;
        assert_eq!(
            parse_retry_after(RFC_EXAMPLE, at(when - 60)),
            Some(Duration::from_secs(60)),
            "a minute before the named moment is a minute to wait"
        );
        // Already past. The end-point answered; the answer is "now".
        assert_eq!(
            parse_retry_after(RFC_EXAMPLE, at(when + 60)),
            Some(Duration::ZERO)
        );
        assert_eq!(
            parse_retry_after(RFC_EXAMPLE, at(when)),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn anything_the_grammar_does_not_admit_is_no_answer_at_all() {
        let now = at(1_445_412_480);
        // `None` and not `Some(ZERO)`: the caller has to be able to tell "wait none"
        // from "the end-point said nothing I can use".
        assert!(parse_retry_after("", now).is_none());
        assert!(parse_retry_after("soon", now).is_none());
        assert!(parse_retry_after("-1", now).is_none());
        assert!(parse_retry_after("12.5", now).is_none());
        assert!(parse_retry_after("2015-10-21T07:28:00Z", now).is_none());
    }
}
