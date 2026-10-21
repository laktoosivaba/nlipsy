//! What each outcome looks like on the status line.
//!
//! ECMA-431 fixes the path and the method and nothing else: §6 says an end-point lives
//! at `https://<server_name>:port/nlip` and accepts NLIP messages with `POST`, and that
//! is the whole of the mandatory surface. Every status code below is therefore this
//! binding's *choice*, which is why it is a value a deployment can replace rather than
//! a `match` inside a pipeline.
//!
//! The defaults reproduce the reference server this workspace was read out of, code for
//! code. That is the point of them: a deployment that changes nothing behaves exactly as
//! it did, and a deployment that changes something says so in one place.

use http::StatusCode;
use nlipsy_core::{EndpointError, Outcome};

/// The status codes this binding answers with.
///
/// Deliberately *not* `#[non_exhaustive]`, unlike most of the values in this workspace.
/// The whole purpose of the type is
///
/// ```rust
/// # use nlipsy_http::StatusMap;
/// # use nlipsy_http::http::StatusCode;
/// let map = StatusMap {
///     unsupported: StatusCode::BAD_REQUEST,
///     ..StatusMap::default()
/// };
/// # let _ = map;
/// ```
///
/// and `#[non_exhaustive]` forbids exactly that spelling from outside the crate,
/// leaving a `with_` method per code as the alternative — one per code, for a struct
/// that is nothing but codes. The cost is that adding a field here is a breaking
/// change; [`StatusMap::other`] exists so that a new [`EndpointError`] variant does not
/// force one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusMap {
    /// The bytes were not an NLIP message at all — [`Outcome::Malformed`]. Default
    /// `400`.
    pub malformed: StatusCode,
    /// The message was refused before the end-point saw it — [`Outcome::Refused`]:
    /// clause 5 under a policy that rejects, or more tokens than the end-point will
    /// echo. Default `400`.
    pub refused: StatusCode,
    /// The body was larger than the end-point will read — see
    /// [`DEFAULT_MAX_REQUEST_BODY_SIZE`] and [`too_large`]. Default `413`.
    ///
    /// §6.2 cannot be honoured here — the body was never decoded, so no token was ever
    /// read — and this is the one refusal after which a peer should treat its state as
    /// untouched: nothing was consumed. ECMA-431 §6.1 is honoured: what goes back is an
    /// NLIP message with a `text` first submessage.
    ///
    /// It is not reachable from an [`Outcome`], and cannot be: the limit is enforced
    /// while the body is read, before there are bytes to decode, so the binding names
    /// this code itself.
    ///
    /// [`DEFAULT_MAX_REQUEST_BODY_SIZE`]: crate::DEFAULT_MAX_REQUEST_BODY_SIZE
    /// [`too_large`]: crate::too_large
    pub too_large: StatusCode,
    /// [`EndpointError::Unsupported`] — the end-point will never answer this request as
    /// written. Default `501`.
    pub unsupported: StatusCode,
    /// [`EndpointError::Unavailable`] — a dependency is down or asked for a delay.
    /// Default `503`, with `Retry-After` when the error named one
    /// ([`retry_after_value`](crate::retry_after_value)).
    pub unavailable: StatusCode,
    /// [`EndpointError::TimedOut`] — the end-point ran out of the time a layer gave it.
    /// Default `504`.
    ///
    /// `504` rather than `408`: the peer sent its request promptly and it is this
    /// end-point that failed to finish, so a status blaming the peer's timing would
    /// blame the wrong side.
    pub timed_out: StatusCode,
    /// [`EndpointError::Internal`] — a fault of this end-point's. Default `500`.
    ///
    /// It also answers a request whose body broke while it was being read — the peer's
    /// connection dropped mid-stream. That is neither a fault of this end-point's nor a
    /// message the peer finished sending; nothing in ECMA-431 speaks to it, and the
    /// reply rarely arrives at all, because the peer that would read it is the one that
    /// went away.
    pub internal: StatusCode,
    /// The end-point's own reply broke clause 5 and was replaced —
    /// [`Served::withheld`](nlipsy_core::Served). Default `500`.
    ///
    /// The peer asked a valid question and is getting a protocol-legal error message
    /// instead of an answer, which is this end-point failing — every other way it fails
    /// is a `5xx` here, and a peer reading only the status line is the second audience
    /// this binding's refusals are written for. It is a field of its own rather than
    /// [`StatusMap::internal`] so that a deployment relaying from an end-point it does
    /// not control can tell the two apart in its own logs.
    ///
    /// The layers' view is unchanged: they saw `Ok`, the session commits and a minted
    /// token is persisted, because the peer's *request* was conforming.
    pub withheld: StatusCode,
    /// A variant of [`EndpointError`] — or of [`Outcome`] — this version of the binding
    /// has no arm for. Default `500`.
    ///
    /// It exists because both vocabularies are `#[non_exhaustive]` and a binding
    /// compiled against an older core can still be handed a newer variant through a
    /// layer. A server error is the honest answer: the end-point produced something its
    /// own binding could not classify, and that is not the peer's fault.
    pub other: StatusCode,
}

impl Default for StatusMap {
    fn default() -> StatusMap {
        StatusMap {
            malformed: StatusCode::BAD_REQUEST,
            refused: StatusCode::BAD_REQUEST,
            too_large: StatusCode::PAYLOAD_TOO_LARGE,
            unsupported: StatusCode::NOT_IMPLEMENTED,
            unavailable: StatusCode::SERVICE_UNAVAILABLE,
            timed_out: StatusCode::GATEWAY_TIMEOUT,
            internal: StatusCode::INTERNAL_SERVER_ERROR,
            withheld: StatusCode::INTERNAL_SERVER_ERROR,
            other: StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl StatusMap {
    /// The defaults.
    pub fn new() -> StatusMap {
        StatusMap::default()
    }

    /// The status this map assigns to one finished exchange.
    ///
    /// `withheld` is [`Served::withheld`](nlipsy_core::Served): the end-point answered,
    /// and clause 5 refused to let its answer out. It can only be `true` beside
    /// [`Outcome::Ok`], and it is the one case where a successful outcome is not a
    /// `200` — the peer asked a valid question and is getting a replacement it never
    /// asked for, which is this end-point failing.
    ///
    /// `200` is not a field of this struct. Every other code here names a way to fail,
    /// and a deployment that wants `201` for a success has left ECMA-431 §6 behind in a
    /// way one number cannot express.
    pub fn of(&self, outcome: &Outcome, withheld: bool) -> StatusCode {
        match outcome {
            Outcome::Ok if withheld => self.withheld,
            Outcome::Ok => StatusCode::OK,
            Outcome::Malformed(_) => self.malformed,
            Outcome::Refused(_) => self.refused,
            Outcome::Failed(error) => self.of_error(error),
            _ => self.other,
        }
    }

    /// The status this map assigns to an end-point's failure on its own.
    ///
    /// The [`Outcome::Failed`] arm of [`StatusMap::of`], reachable separately because a
    /// binding has failures that are not exchanges: ECMA-431 §6.1's out-of-band upload
    /// leg is answered by an [`UploadSink`] that fails with the same vocabulary and has
    /// no clause 6 around it.
    ///
    /// [`EndpointError::Refused`] maps to [`StatusMap::refused`], the same code the
    /// binding answers when it ran [`check`](nlipsy_core::Conformity::check) itself: a
    /// layer that wraps the check and one that is wrapped by it should not be
    /// distinguishable from outside.
    ///
    /// [`UploadSink`]: https://docs.rs/nlipsy-server/latest/nlipsy_server/upload/trait.UploadSink.html
    pub fn of_error(&self, error: &EndpointError) -> StatusCode {
        match error {
            EndpointError::Unsupported(_) => self.unsupported,
            EndpointError::Unavailable { .. } => self.unavailable,
            EndpointError::TimedOut { .. } => self.timed_out,
            EndpointError::Internal(_) => self.internal,
            EndpointError::Refused(_) => self.refused,
            _ => self.other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_defaults_are_the_reference_servers_codes() {
        let map = StatusMap::default();
        assert_eq!(map.of_error(&EndpointError::Unsupported("x".into())), 501);
        assert_eq!(
            map.of_error(&EndpointError::Unavailable {
                detail: "x".into(),
                retry_after: None
            }),
            503
        );
        assert_eq!(
            map.of_error(&EndpointError::TimedOut {
                after: Duration::from_secs(1)
            }),
            504
        );
        assert_eq!(map.of_error(&EndpointError::Internal("x".into())), 500);
        assert_eq!(map.withheld, 500);
    }

    #[test]
    fn an_outcome_is_mapped_the_same_way_the_error_underneath_it_is() {
        let map = StatusMap::default();
        assert_eq!(map.of(&Outcome::Ok, false), 200);
        // Not an `EndpointError` at all — the end-point answered, and clause 5 refused
        // to let the answer out.
        assert_eq!(map.of(&Outcome::Ok, true), 500);
        assert_eq!(
            map.of(
                &Outcome::Failed(EndpointError::Unavailable {
                    detail: "x".into(),
                    retry_after: None
                }),
                false
            ),
            503
        );
    }

    #[test]
    fn a_deployment_can_move_one_code_without_moving_the_rest() {
        let map = StatusMap {
            unsupported: StatusCode::BAD_REQUEST,
            ..StatusMap::default()
        };
        assert_eq!(map.of_error(&EndpointError::Unsupported("x".into())), 400);
        assert_eq!(map.of_error(&EndpointError::Internal("x".into())), 500);
    }
}
