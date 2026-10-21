//! The out-of-band leg — ECMA-431 §6.1, advertised by ECMA-430 §6.4.
//!
//! **Nothing here is an NLIP exchange, and that is deliberate rather than an
//! oversight.** The body is arbitrary bytes: there is no inbound message to validate,
//! no tokens to echo back (§6.2 speaks of a message carrying them, and this request
//! carries none), no control-ness to carry (§6.3) and nothing for
//! [`Conformity`](nlipsy_core::Conformity) to finish. ECMA-430 §7.3 ¶2 calls this leg
//! a *base transfer protocol* operation rather than a message exchange, and the
//! clause-6 wrapper therefore does not apply to it. Without that written down, the
//! first person to add a layer to the router wonders why one route skips it.
//!
//! What the leg still is: a `POST` that answers with one NLIP message saying what
//! became of the bytes, at the same media type the endpoint's own replies carry —
//! JSON per ECMA-430 §5 ¶2, `application/json` per ECMA-433 §6.1.5, ECMA-431 §6 naming
//! neither. The body limit applies here too, and for the same reason.
//!
//! § 6.1 asks for the upload to be "provided at any port different than the port used
//! by NLIP end point", which is why this is a [`Router`](axum::Router) of its own
//! rather than a route beside `/nlip`: a route on the endpoint's router could only
//! ever answer on the endpoint's port, so a deployment built that way could only
//! advertise an address the clause forbids.

use std::future::Future;

use axum::body::Bytes;
use axum::http::HeaderMap;
use nlipsy_core::{Context, EndpointError, MaybeSend};
use nlipsy_protocol::message::Message;

/// One uploaded payload, as the sink sees it.
///
/// `#[non_exhaustive]`: a later binding hands a sink more — a declared length, a
/// checksum header, the profile of ECMA-434 the deployment claims — and a sink only
/// ever reads this.
#[derive(Debug)]
#[non_exhaustive]
pub struct Upload<'a> {
    /// The bytes, already read under the configured limit.
    pub body: Bytes,
    /// The request's headers. A sink that derives a file extension reads the declared
    /// content type from here — and derives it, rather than taking a name the peer
    /// supplied, because a name from the request is a path-traversal vector and
    /// nothing in the protocol needs one.
    pub headers: &'a HeaderMap,
    /// Peer and correlator, the same two values the endpoint's own exchanges carry,
    /// so a stored payload can be tied to the conversation that announced it.
    pub cx: &'a Context,
}

/// Where the bytes go.
///
/// A trait rather than a directory, because "store it" means a filesystem to one
/// deployment, an object store to another and `/dev/null` to a third — and because
/// ECMA-431 §6.1 makes the whole leg optional, so a deployment that offers none
/// simply never builds the router.
///
/// The reply is the sink's: it is the sink that knows what happened to the bytes, and
/// the message it returns goes to the peer as-is. Nothing in clause 6 is applied to
/// it (see the module documentation), so a sink that wants to say something in a
/// `control` message is free to.
///
/// An [`EndpointError`] is answered at the status
/// [`StatusMap`](crate::StatusMap) gives it, rendered as an NLIP error message
/// through [`public_message`](EndpointError::public_message) — so a detail a sink puts
/// in [`EndpointError::Internal`] reaches the deployment's log and not the peer.
pub trait UploadSink: MaybeSend + Sync + 'static {
    /// Take one payload and say what became of it.
    fn store<'a>(
        &'a self,
        upload: Upload<'a>,
    ) -> impl Future<Output = Result<Message, EndpointError>> + MaybeSend + 'a;
}

/// A sink that acknowledges an upload and keeps nothing.
///
/// The honest default for a deployment that mounts the leg to see whether peers use
/// it, and the one implementation this crate ships: a filesystem sink would have to
/// choose a naming scheme, an extension table and a failure policy, and all three are
/// a deployment's rather than a binding's.
#[derive(Debug, Clone, Copy, Default)]
pub struct DiscardSink;

impl UploadSink for DiscardSink {
    async fn store(&self, upload: Upload<'_>) -> Result<Message, EndpointError> {
        let size = upload.body.len();
        Ok(Message::text(format!(
            "upload received ({size} bytes) and discarded; this endpoint stores nothing"
        )))
    }
}
