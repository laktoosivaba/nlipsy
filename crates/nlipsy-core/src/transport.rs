//! The endpoint as a client agent sees it — ECMA-430 §4.7.
//!
//! §4.7 defines a *client agent* as "an agent which initiates the transfer of data to
//! another agent". This module is that half of the protocol and nothing else: it
//! encodes a message, hands the bytes to a [`Transport`], and reads what came back. It
//! decides nothing about what any of it means — no notion of a conversation, of a
//! session, or of what should happen next lives here. That is deliberate: the ends of
//! this protocol are written by different people, and a crate that decided what a
//! conversation *is* would impose one reading of §6.2 on both of them.
//!
//! There is no socket here either. A binding crate implements [`Transport`]; this
//! module is what every binding shares.
//!
//! # Two things this module refuses to pretend
//!
//! **Not every reply is an NLIP message.** The standard fixes the endpoint's own
//! answers, not the answers of everything in front of it. [`Body::Opaque`] exists for
//! that, and a caller renders those bytes rather than failing to decode them — see the
//! variant's own documentation for why, per binding.
//!
//! **A failed exchange is not the same as an unsent one.** [`TransportError`]
//! separates a request that never left ([`TransportError::Unreachable`]) from one that
//! was sent and whose reply never arrived ([`TransportError::Interrupted`]). The
//! difference is the whole of the retry policy: the first can be repeated freely,
//! while the second may already sit in the endpoint's transcript, and repeating it
//! would put the same turn there twice — a divergence between the two records that
//! nothing on the wire can reveal.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use nlipsy_protocol::content::Wire;
use nlipsy_protocol::message::{Message, Submessage};
use thiserror::Error;

use crate::context::Context;
use crate::first_submessage::first_submessage_token;
use crate::send::{BoxFuture, MaybeSend};

/// How much of an unrecognised body is kept for display.
///
/// A proxy's error page can be arbitrarily long and is not worth a screenful. The
/// limit is on display only — the bytes are not truncated before decoding.
pub const OPAQUE_PREVIEW: usize = 512;

/// One exchange, before anything is made of it.
///
/// The body stays bytes. A recording that stored a decoded [`Message`] would be
/// re-encoded through this crate's own encoder when replayed, and would then agree
/// with a systematic mistake in that encoder just as readily as with a correct
/// reading — which is exactly what a recorded fixture exists to rule out.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Exchange {
    /// What came back, verbatim.
    pub body: Vec<u8>,
    /// What the binding knows about the exchange that the body does not say.
    pub detail: ExchangeDetail,
}

impl Exchange {
    /// An exchange with a body and a detail.
    pub fn new(body: Vec<u8>, detail: ExchangeDetail) -> Exchange {
        Exchange { body, detail }
    }

    /// An HTTP exchange — the shorthand a binding and a test both want.
    pub fn http(status: u16, body: impl Into<Vec<u8>>) -> Exchange {
        Exchange {
            body: body.into(),
            detail: ExchangeDetail::Http {
                status,
                retry_after: None,
            },
        }
    }
}

/// What one binding knows about an exchange that another has no word for.
///
/// `#[non_exhaustive]`, and with one variant today on purpose. ECMA-431's status code
/// and `Retry-After` are facts a caller acts on — a `429` is worth waiting out and a
/// `501` is not — so flattening them away would lose real information. A WebSocket
/// frame (ECMA-432) has neither, and AMQP (ECMA-433) has neither; each adds its own
/// variant when it arrives, rather than pretending to have a status.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExchangeDetail {
    /// ECMA-431: the response status, and the delay the endpoint asked for.
    Http {
        /// The HTTP status line, as a number.
        status: u16,
        /// `Retry-After`, parsed into a value.
        retry_after: Option<Duration>,
    },
}

impl ExchangeDetail {
    /// `true` when the binding says the endpoint answered rather than refused.
    ///
    /// A binding with no notion of failure at this level answers `true`.
    pub fn is_ok(&self) -> bool {
        match self {
            ExchangeDetail::Http { status, .. } => (200..300).contains(status),
        }
    }

    /// The HTTP status, when this exchange had one.
    pub fn status(&self) -> Option<u16> {
        match self {
            ExchangeDetail::Http { status, .. } => Some(*status),
        }
    }

    /// The delay the endpoint asked for, when it named one.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            ExchangeDetail::Http { retry_after, .. } => *retry_after,
        }
    }
}

/// Why an exchange did not produce a reply.
///
/// `#[non_exhaustive]`: a binding will want to name a failure these three do not
/// cover. Match with a `_` arm.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TransportError {
    /// The connection was never established, so the endpoint cannot have seen the
    /// request. Repeating it cannot duplicate anything.
    #[error("could not reach {endpoint}: {detail}")]
    Unreachable {
        /// Where the request was going.
        endpoint: String,
        /// What the binding said about it.
        detail: String,
    },

    /// The request went out and the reply never came back.
    ///
    /// Whether the endpoint acted on it is genuinely unknown — it may have recorded
    /// the exchange before the connection broke. Repeating this risks the same turn
    /// appearing twice in the endpoint's transcript.
    #[error("no reply from {endpoint}: {detail}")]
    Interrupted {
        /// Where the request was going.
        endpoint: String,
        /// What the binding said about it.
        detail: String,
    },

    /// A reply arrived and could not be read as the binding's framing at all.
    #[error("unusable reply: {detail}")]
    Malformed {
        /// What the binding said about it.
        detail: String,
    },
}

/// Something that can carry one exchange.
///
/// The correlator is not in the signature. ECMA-433 §6.1.4 makes correlation normative
/// as an AMQP property while ECMA-431 leaves it to convention (an `X-NLIP-Correlator`
/// header, in the reference server), so it is one concept with a spelling per binding:
/// it travels in [`Context::correlator`] and the transport decides how to write it.
///
/// The trait exists so that a test can drive a client from a recorded wire instead of
/// a socket, and so that a recorder can wrap a real transport without the client
/// knowing.
pub trait Transport: MaybeSend + Sync {
    /// Send `body` to the endpoint and return what came back.
    fn exchange(
        &self,
        cx: &Context,
        body: Vec<u8>,
    ) -> impl Future<Output = Result<Exchange, TransportError>> + MaybeSend;
}

/// The object-safe form of [`Transport`].
///
/// A return-position `impl Trait` cannot appear in a `dyn` trait, and a binary does
/// have to choose its transport at run time: `--record` wraps the real one, `--replay`
/// replaces it, and neither is known until the arguments have been parsed.
pub trait DynTransport: MaybeSend + Sync + 'static {
    /// Send `body` to the endpoint, as a boxed future.
    fn exchange_boxed<'a>(
        &'a self,
        cx: &'a Context,
        body: Vec<u8>,
    ) -> BoxFuture<'a, Result<Exchange, TransportError>>;
}

impl<T: Transport + 'static> DynTransport for T {
    fn exchange_boxed<'a>(
        &'a self,
        cx: &'a Context,
        body: Vec<u8>,
    ) -> BoxFuture<'a, Result<Exchange, TransportError>> {
        Box::pin(Transport::exchange(self, cx, body))
    }
}

/// A boxed transport is a transport, so a run-time choice can be passed where a
/// [`Transport`] is expected — to a recorder, or to [`Client::new`].
impl Transport for Box<dyn DynTransport> {
    // `async fn` rather than the boxed future directly, for the reason given on
    // `impl Endpoint for Box<dyn DynEndpoint>`: E0700.
    async fn exchange(&self, cx: &Context, body: Vec<u8>) -> Result<Exchange, TransportError> {
        DynTransport::exchange_boxed(&**self, cx, body).await
    }
}

/// A shared transport is a transport.
///
/// [`Client::new`] takes its transport by value and keeps it boxed, with no way to get
/// it back. A caller that needs to look at what was sent — every test that drives a
/// client does — would otherwise wrap the `Arc` in a newtype and write this same
/// delegation by hand.
///
/// See the `where` clause on `impl Endpoint for Arc<T>`: `Arc<T>: Sync` needs
/// `T: Send`, which `MaybeSend` gives on a native target and deliberately withholds on
/// `wasm32`.
impl<T: Transport + ?Sized> Transport for Arc<T>
where
    Arc<T>: MaybeSend + Sync,
{
    fn exchange(
        &self,
        cx: &Context,
        body: Vec<u8>,
    ) -> impl Future<Output = Result<Exchange, TransportError>> + MaybeSend {
        T::exchange(&**self, cx, body)
    }
}

/// What the endpoint sent back.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// Bytes that decode as an NLIP message.
    Message(Message),
    /// Bytes that do not, with the reason.
    ///
    /// Kept for display: the text is what the user needs to see, and the reason is
    /// what explains why it is being shown raw. A JSON-RPC client treats a body it
    /// cannot parse as an error and stops; an NLIP client has to *render* it.
    ///
    /// Why it happens is a property of the binding, not of the protocol:
    ///
    /// * **ECMA-431 (HTTP)** — the endpoint is behind intermediaries that answer in
    ///   their own voice. A request over a body limit is refused by the HTTP layer
    ///   with plain text; a proxy that cannot reach the backend answers with its own
    ///   HTML; a route that does not match produces a JSON document that is not a
    ///   message. None of those bytes came from an NLIP implementation at all.
    /// * **ECMA-432 (WebSocket)** — §11 gives a decode failure a *prescribed* answer
    ///   instead: fall back to the text endpoint. That is a different behaviour, not
    ///   this one renamed, so a WebSocket binding reaches this variant rarely if ever.
    /// * **ECMA-433 (AMQP)** — the peer is addressed directly and a broker does not
    ///   answer in the peer's place, so this variant may be unreachable there.
    Opaque {
        /// The body, rendered for a reader and cut to [`OPAQUE_PREVIEW`] characters.
        text: String,
        /// Why it could not be read as a message.
        why: String,
    },
}

/// One reply, as the rest of a client consumes it.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Reply {
    /// What the binding knows about the exchange.
    pub detail: ExchangeDetail,
    /// How many bytes the body was, before anything was made of it.
    ///
    /// Kept because a journal reports sizes, and the decoded length is not the same
    /// number: it is measured after whitespace and escapes have been resolved. A body
    /// that did not decode at all has no decoded length to report and a real one on
    /// the wire, which is the case that settles it.
    pub bytes: usize,
    /// The body, decoded if it could be.
    pub body: Body,
}

impl Reply {
    /// A reply from what the binding reported and what the bytes turned out to be.
    ///
    /// [`Client::send`] builds these; the constructor is public so a test can write one
    /// without a transport, which is what most of a client's own tests want.
    pub fn new(detail: ExchangeDetail, bytes: usize, body: Body) -> Reply {
        Reply {
            detail,
            bytes,
            body,
        }
    }

    /// `true` when the endpoint answered rather than refused.
    pub fn is_ok(&self) -> bool {
        self.detail.is_ok()
    }

    /// The HTTP status, when the exchange had one.
    pub fn status(&self) -> Option<u16> {
        self.detail.status()
    }

    /// The delay the endpoint asked for, when it named one.
    pub fn retry_after(&self) -> Option<Duration> {
        self.detail.retry_after()
    }

    /// The message, when the body was one.
    pub fn message(&self) -> Option<&Message> {
        match &self.body {
            Body::Message(message) => Some(message),
            Body::Opaque { .. } => None,
        }
    }

    /// The text to show the user, whoever wrote it.
    ///
    /// The language filter is deliberately not used. A reply's `subformat` states the
    /// language of its *content*, and an endpoint that answers in a language other
    /// than the one asked for is answering, not failing; filtering on the requested
    /// language would turn a perfectly good reply into an empty string.
    pub fn text(&self) -> Option<String> {
        match &self.body {
            Body::Message(message) => message.extract_text(None),
            Body::Opaque { text, .. } => Some(text.clone()),
        }
    }

    /// Every `token` submessage the reply carries, as it arrived.
    ///
    /// §6.2 ¶2 obliges an end-point that receives a token submessage to include "the
    /// same submessage with the exact same format, subformat and content" in its
    /// response, "except when the token was initially created by the end-point". A
    /// client is an end-point and its next request is its response, so **this is what
    /// the caller sends on that request** — before removing the ones it created
    /// itself. That exception is not applied here and cannot be: it needs the
    /// end-point's memory of what it minted, which is the application's and not this
    /// crate's. A caller that mints its own conversation identifier filters this list
    /// on that value and sends the rest.
    ///
    /// The first submessage leads, then the `submessages` array in document order
    /// ([`first_submessage_token`] in front of [`Message::tokens`]): §5 ¶2 calls the
    /// leading object "the first submessage", so a message that *is* a token carries
    /// one for §6.2's purposes and the array cannot show it. The order is this crate's
    /// and a peer must not rely on it — §6.2 fixes none, and §6.2.1 lets the other
    /// agent "optionally add another" conversation token, so two of them in one reply
    /// is the state the clause describes rather than a mistake. Picking "the last one"
    /// is therefore not a rule.
    ///
    /// Everything travels verbatim, including a `Label` and a content that is not a
    /// string: "the exact same content" leaves nothing to normalise, and a token whose
    /// content is a number is still a token that must come back.
    ///
    /// [`Reply::conversation_tokens`] and [`Reply::authentication_tokens`] are derived
    /// from this list and are keys — for a session table, for a status line — not wire
    /// material. What goes back out is this.
    pub fn tokens(&self) -> Vec<Submessage> {
        match &self.body {
            Body::Message(message) => first_submessage_token(message)
                .into_iter()
                .chain(message.tokens().cloned())
                .collect(),
            Body::Opaque { .. } => Vec::new(),
        }
    }

    /// The content of every conversation token the reply carries, in order.
    ///
    /// §6.2.1's identifier as a **key**: what a client files a transcript under, shows
    /// on a status line, or compares against the identifier it minted itself. It is
    /// not what goes back on the next request — [`Reply::tokens`] is. A client that
    /// rebuilt a token from one of these strings would choose a subformat and drop the
    /// label, and §6.2 ¶2 asks for "the exact same format, subformat and content": the
    /// suffix after `_` is §6.2.1's identity of the agent that created the
    /// conversation, and rewriting it is exactly what the clause forbids.
    ///
    /// A reply may carry two of them — the client's own, echoed, and the end-point's
    /// under §6.2.1 — in no order the standard fixes. A token whose content is not a
    /// string has no entry here and is still in [`Reply::tokens`].
    pub fn conversation_tokens(&self) -> Vec<String> {
        self.token_contents(Submessage::is_conversation_token)
    }

    /// The content of every authentication token the reply carries, in order.
    ///
    /// §6.2.2's credential as a **key**, on the same terms as
    /// [`Reply::conversation_tokens`]: for a caller that needs to notice one arriving
    /// or to tell two apart. The obligation the clause states — the other end-point
    /// "is obligated to return the authentication token unmodified so that
    /// authentication can be performed seamlessly" — is the caller's, on its next
    /// request, and what it returns is [`Reply::tokens`]. Rebuilding the submessage
    /// from this string would modify it.
    pub fn authentication_tokens(&self) -> Vec<String> {
        self.token_contents(Submessage::is_authentication_token)
    }

    /// The string contents of the tokens of one reserved kind, in [`Reply::tokens`]'s
    /// order, so that the two views and the list they come from cannot disagree.
    fn token_contents(&self, kind: fn(&Submessage) -> bool) -> Vec<String> {
        self.tokens()
            .iter()
            .filter(|token| kind(token))
            .filter_map(|token| token.content_as_str().map(str::to_string))
            .collect()
    }

    /// Whether the endpoint marked the reply as an error in NLIP terms.
    ///
    /// A body that is not a message counts as one: the caller must not be left
    /// believing an endpoint that never spoke NLIP said something in it.
    pub fn is_error(&self) -> bool {
        match &self.body {
            Body::Message(message) => message
                .message_type
                .as_deref()
                .is_some_and(|kind| kind.eq_ignore_ascii_case("error")),
            Body::Opaque { .. } => true,
        }
    }
}

/// Encode a request for the wire.
///
/// A free function rather than a method on [`Client`] so that a binding — or a
/// JavaScript package over this crate — can encode without owning a client. It is also
/// where a client's encoding is pinned: the recorded fixtures were taken with *this*
/// encoding, and a fixture that stored only the reply would not notice the request
/// changing shape underneath it.
///
/// # Panics
///
/// On [`Wire::Cbor`] when the `cbor` feature is off. There is then no CBOR codec in
/// the build at all, and the two alternatives are both worse: encoding JSON instead
/// would put a frame on an ECMA-432 §7.1 binary channel that the peer cannot read, and
/// returning an empty body would look like a message the peer sent nothing in.
pub fn encode_request(message: &Message, wire: Wire) -> Vec<u8> {
    match wire {
        Wire::Json => message.to_json_vec(),
        #[cfg(feature = "cbor")]
        Wire::Cbor => message.to_cbor_vec(),
        #[cfg(not(feature = "cbor"))]
        Wire::Cbor => panic!(
            "Wire::Cbor needs the `cbor` feature of nlipsy-core (ECMA-432 §7.1); \
             this build has no CBOR codec"
        ),
    }
}

/// Read a reply's bytes into a [`Body`].
///
/// Never fails: bytes that are not a message become [`Body::Opaque`] with the reason,
/// because over HTTP they usually came from something in front of the endpoint and the
/// user still has to see them.
///
/// # Panics
///
/// On [`Wire::Cbor`] when the `cbor` feature is off, for the reason given on
/// [`encode_request`] — and with more force here, since a `Body::Opaque` would blame
/// the peer for a codec this build does not have.
pub fn decode_reply(bytes: &[u8], wire: Wire) -> Body {
    let decoded = match wire {
        Wire::Json => Message::from_json_bytes(bytes),
        #[cfg(feature = "cbor")]
        Wire::Cbor => Message::from_cbor_slice(bytes),
        #[cfg(not(feature = "cbor"))]
        Wire::Cbor => panic!(
            "Wire::Cbor needs the `cbor` feature of nlipsy-core (ECMA-432 §7.1); \
             this build has no CBOR codec"
        ),
    };
    match decoded {
        Ok(message) => Body::Message(message),
        Err(error) => Body::Opaque {
            text: preview(bytes),
            why: error.to_string(),
        },
    }
}

/// A client for one endpoint, on one wire.
///
/// One `await` over a [`Transport`]: encode, exchange, decode. Everything else — which
/// endpoint, how long to wait, whether to ask again, what the conversation token is —
/// belongs to the caller, because a terminal client and a server fanning out to five
/// peers want opposite things from the same exchange.
///
/// # The wire belongs to the client, not to the call
///
/// A client agent talks to one endpoint over one binding, and the binding fixes the
/// encoding: **ECMA-431 is JSON** — ECMA-430 §5 ¶2, "primarily represented as a JSON
/// message", with §6 of ECMA-431 naming neither an encoding nor a media type — and
/// **ECMA-432 §7.1 is CBOR**, the WebSocket binding's binary frame. So the choice is
/// made once, where the endpoint is chosen, rather than passed again on every
/// [`send`](Client::send); a caller that speaks both talks to two endpoints and holds
/// two clients.
///
/// [`Client::new`] means [`Wire::Json`] and [`Client::on_wire`] is the other one.
/// [`Wire::Cbor`] needs the `cbor` feature exactly as [`encode_request`] does: without
/// it there is no CBOR codec in the build at all, and `send` panics there rather than
/// putting a JSON frame on a binary channel the peer cannot read.
///
/// The wire is a field rather than an argument of `send` because of the one caller that
/// has to choose it per client: the interface contract's `client` takes its wire at
/// construction, and while `send` was JSON-only the WebAssembly guest composed
/// [`encode_request`] and [`decode_reply`] around its own transport instead — this
/// method, written a second time. The wire is here so that "encode, exchange, decode"
/// has one implementation and not one per binding.
pub struct Client {
    transport: Box<dyn DynTransport>,
    wire: Wire,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").finish_non_exhaustive()
    }
}

impl Client {
    /// A client over `transport`, on [`Wire::Json`].
    ///
    /// The signature and the behaviour are ECMA-431's and are unchanged: a caller that
    /// names nothing else gets the JSON wire, which is what an endpoint reached over
    /// HTTP expects. [`Client::on_wire`] is how the other one is asked for.
    pub fn new(transport: impl Transport + 'static) -> Client {
        Client {
            transport: Box::new(transport),
            wire: Wire::Json,
        }
    }

    /// Encode and decode on `wire` instead of on [`Wire::Json`].
    ///
    /// A builder rather than a second constructor, and deliberately the same spelling as
    /// [`Replay::on_wire`](crate::Replay::on_wire): the two answer one question — which
    /// encoding these bytes are — and a client driven from a recording sets both, so a
    /// second name for it would be a second thing to remember. It also keeps
    /// [`Client::new`] the one constructor, which is what makes this addition additive
    /// rather than a fork of the type's construction.
    ///
    /// # Panics
    ///
    /// Not here. [`Wire::Cbor`] in a build without the `cbor` feature panics on the
    /// first [`Client::send`], where [`encode_request`] does, and for its reason.
    pub fn on_wire(mut self, wire: Wire) -> Client {
        self.wire = wire;
        self
    }

    /// Which wire this client encodes and decodes on.
    ///
    /// A binding that has to name the encoding somewhere else — an ECMA-431
    /// `Content-Type`, an ECMA-432 frame kind — asks the client rather than keeping its
    /// own copy of the answer.
    pub fn wire(&self) -> Wire {
        self.wire
    }

    /// Perform one exchange, on this client's [`wire`](Client::wire).
    ///
    /// Encode the request, hand the bytes to the transport, read what came back on the
    /// same wire. Both directions use the one setting: a peer that was asked in CBOR
    /// answers in CBOR (ECMA-432 §7.1), and decoding its frame as JSON would report the
    /// peer as having sent something unreadable when it was this end that misread it.
    ///
    /// # Panics
    ///
    /// On [`Wire::Cbor`] when the `cbor` feature is off, for the reason given on
    /// [`encode_request`].
    pub async fn send(&self, request: &Message, cx: &Context) -> Result<Reply, TransportError> {
        let body = encode_request(request, self.wire);
        let exchange = self.transport.exchange_boxed(cx, body).await?;

        let bytes = exchange.body.len();
        let body = decode_reply(&exchange.body, self.wire);

        Ok(Reply {
            detail: exchange.detail,
            bytes,
            body,
        })
    }
}

/// Render bytes that did not decode, for a reader.
///
/// Lossy on purpose: the bytes may be a partial UTF-8 sequence from a truncated
/// response, and refusing to show them would hide the very thing being diagnosed.
fn preview(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    if trimmed.chars().count() <= OPAQUE_PREVIEW {
        return trimmed.to_string();
    }
    let cut: String = trimmed.chars().take(OPAQUE_PREVIEW).collect();
    format!("{cut}…")
}

#[cfg(all(test, feature = "testing"))]
mod tests {
    use super::*;
    use crate::testing::{Scripted, ok};
    use nlipsy_protocol::content::Content;

    /// A request with a conversation token, which is what most of these exercise.
    fn ask(text: &str) -> Message {
        Message::text(text).with_submessage(Submessage::token("conversation", "c1"))
    }

    fn client(replies: Vec<Result<Exchange, TransportError>>) -> (Client, Arc<Scripted>) {
        // The `Arc` is what lets the test keep a handle on the transport after the
        // client has taken it: `Client::new` boxes what it is given and never gives it
        // back, and `impl Transport for Arc<T>` is what makes an `Arc` enough.
        let scripted = Arc::new(Scripted::new(replies));
        let client = Client::new(Arc::clone(&scripted));
        (client, scripted)
    }

    #[tokio::test]
    async fn a_reply_that_is_a_message_is_decoded() {
        let (client, _) = client(vec![ok(
            200,
            r#"{"format":"text","subformat":"russian","content":"привет"}"#,
        )]);

        let reply = client.send(&ask("привет"), &Context::new()).await.unwrap();

        assert!(reply.is_ok());
        assert_eq!(reply.text().as_deref(), Some("привет"));
        assert!(!reply.is_error());
    }

    #[tokio::test]
    async fn the_request_reaches_the_transport_as_encoded_bytes() {
        let (client, sent) = client(vec![ok(
            200,
            r#"{"format":"text","subformat":"english","content":"x"}"#,
        )]);

        client.send(&ask("hi"), &Context::new()).await.unwrap();

        let sent = sent.sent_json();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["format"], "text");
        assert_eq!(sent[0]["submessages"][0]["format"], "token");
        assert_eq!(sent[0]["submessages"][0]["content"], "c1");
    }

    #[tokio::test]
    async fn conversation_tokens_are_collected_from_the_reply() {
        let (client, _) = client(vec![ok(
            200,
            r#"{"format":"text","subformat":"english","content":"x",
                "submessages":[{"format":"token","subformat":"conversation","content":"c1"},
                               {"format":"token","subformat":"authentication","content":"a1"}]}"#,
        )]);

        let reply = client.send(&ask("hi"), &Context::new()).await.unwrap();

        // Only conversation tokens, and the authentication one is not one.
        assert_eq!(reply.conversation_tokens(), vec!["c1".to_string()]);
    }

    #[tokio::test]
    async fn a_token_comes_back_with_its_subformat_and_its_label_untouched() {
        // §6.2 ¶2 asks for "the exact same format, subformat and content", and §6.2.1
        // gives the suffix a meaning — the identity of the agent that created the
        // conversation. A client that rebuilt this from `conversation_tokens` alone
        // would send back `conversation` and no label, which is a different submessage.
        let (client, _) = client(vec![ok(
            200,
            r#"{"format":"text","subformat":"english","content":"x",
                "submessages":[{"label":"seq-1","format":"token",
                                "subformat":"conversation_9.2.3.5","content":"6f1c0b12"}]}"#,
        )]);

        let reply = client.send(&ask("hi"), &Context::new()).await.unwrap();

        assert_eq!(
            reply.tokens(),
            vec![Submessage::token("conversation_9.2.3.5", "6f1c0b12").with_label("seq-1")]
        );
        assert_eq!(reply.conversation_tokens(), vec!["6f1c0b12".to_string()]);
    }

    #[tokio::test]
    async fn an_authentication_token_is_in_the_list_and_in_its_own_view() {
        // §6.2.2's credential, which the caller is "obligated to return … unmodified":
        // the submessage is what goes back, the string is the key beside it.
        let (client, _) = client(vec![ok(
            200,
            r#"{"format":"text","subformat":"english","content":"x",
                "submessages":[{"format":"token","subformat":"conversation_srv","content":"c-1"},
                               {"format":"token","subformat":"authentication_srv","content":"a-1"}]}"#,
        )]);

        let reply = client.send(&ask("hi"), &Context::new()).await.unwrap();

        assert_eq!(
            reply.tokens(),
            vec![
                Submessage::token("conversation_srv", "c-1"),
                Submessage::token("authentication_srv", "a-1"),
            ]
        );
        assert_eq!(reply.authentication_tokens(), vec!["a-1".to_string()]);
        assert_eq!(reply.conversation_tokens(), vec!["c-1".to_string()]);
    }

    #[tokio::test]
    async fn a_reply_that_is_only_a_token_still_carries_one() {
        // §5 ¶2's first submessage, which `submessages` cannot show — the shape the
        // reference SDK's `create_token` emits (`conformance/cases/readings/36`). The
        // string views are derived from the list, so it reaches them too.
        let (client, _) = client(vec![ok(
            200,
            r#"{"format":"token","subformat":"authentication_9.2.3.5","content":"opaque"}"#,
        )]);

        let reply = client.send(&ask("hi"), &Context::new()).await.unwrap();

        assert_eq!(
            reply.tokens(),
            vec![Submessage::token("authentication_9.2.3.5", "opaque")]
        );
        assert_eq!(reply.authentication_tokens(), vec!["opaque".to_string()]);
    }

    #[tokio::test]
    async fn a_token_whose_content_is_not_a_string_is_returned_all_the_same() {
        // "The exact same content" leaves nothing to normalise and nothing to drop.
        // The string views have no entry for it, which is the whole reason they are not
        // what goes back on the wire.
        let (client, _) = client(vec![ok(
            200,
            r#"{"format":"text","subformat":"english","content":"x",
                "submessages":[{"format":"token","subformat":"conversation","content":42}]}"#,
        )]);

        let reply = client.send(&ask("hi"), &Context::new()).await.unwrap();

        let tokens = reply.tokens();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].subformat, "conversation");
        assert!(matches!(tokens[0].content, Content::Number(_)));
        assert!(reply.conversation_tokens().is_empty());
        assert!(reply.authentication_tokens().is_empty());
    }

    #[tokio::test]
    async fn a_body_that_is_not_a_message_is_kept_verbatim() {
        // What an HTTP body limit returns: plain text, not NLIP.
        let (client, _) = client(vec![ok(413, "length limit exceeded")]);

        let reply = client.send(&ask("hi"), &Context::new()).await.unwrap();

        assert!(!reply.is_ok());
        assert_eq!(reply.text().as_deref(), Some("length limit exceeded"));
        // Nothing to decode means nothing to echo, and the caller must not be left
        // believing an endpoint that never spoke NLIP said something in it.
        assert!(reply.message().is_none());
        assert!(reply.is_error());
        assert!(reply.tokens().is_empty());
        assert!(reply.conversation_tokens().is_empty());
        assert!(reply.authentication_tokens().is_empty());
    }

    #[tokio::test]
    async fn an_html_error_page_is_kept_as_text_rather_than_rejected() {
        let (client, _) = client(vec![ok(502, "<html><body>Bad Gateway</body></html>")]);

        let reply = client.send(&ask("hi"), &Context::new()).await.unwrap();

        assert!(reply.text().unwrap().contains("Bad Gateway"));
    }

    #[tokio::test]
    async fn an_error_message_is_still_a_message() {
        let (client, _) = client(vec![ok(
            503,
            r#"{"messagetype":"Error","format":"text","subformat":"english",
                "content":"the endpoint is temporarily unable to answer",
                "submessages":[{"format":"token","subformat":"conversation","content":"c1"}]}"#,
        )]);

        let reply = client.send(&ask("hi"), &Context::new()).await.unwrap();

        assert!(!reply.is_ok());
        assert!(reply.is_error());
        assert!(reply.message().is_some());
        // The endpoint echoes the peer's token even in a refusal (§6.2), so the
        // conversation survives a failed turn.
        assert_eq!(reply.conversation_tokens(), vec!["c1".to_string()]);
    }

    #[tokio::test]
    async fn the_bindings_own_facts_survive_into_the_reply() {
        let scripted = Arc::new(Scripted::new(vec![Ok(Exchange {
            body: br#"{"messagetype":"Error","format":"text","subformat":"english","content":"slow down"}"#
                .to_vec(),
            detail: ExchangeDetail::Http {
                status: 429,
                retry_after: Some(Duration::from_secs(7)),
            },
        })]));
        let client = Client::new(scripted);

        let reply = client.send(&ask("hi"), &Context::new()).await.unwrap();

        assert_eq!(reply.status(), Some(429));
        assert_eq!(reply.retry_after(), Some(Duration::from_secs(7)));
    }

    #[test]
    fn a_failure_to_connect_is_distinguishable_from_a_lost_reply() {
        // The distinction the retry policy rests on: only the first is safe to repeat
        // without risking a duplicated turn in the endpoint's transcript.
        let unreachable = TransportError::Unreachable {
            endpoint: "https://example.invalid/nlip".to_string(),
            detail: "dns".to_string(),
        };
        let interrupted = TransportError::Interrupted {
            endpoint: "https://example.invalid/nlip".to_string(),
            detail: "reset".to_string(),
        };

        assert!(!matches!(unreachable, TransportError::Interrupted { .. }));
        assert!(matches!(interrupted, TransportError::Interrupted { .. }));
    }

    #[test]
    fn a_long_unrecognised_body_is_cut_for_display() {
        let long = "x".repeat(OPAQUE_PREVIEW * 2);
        let shown = preview(long.as_bytes());
        assert!(shown.ends_with('…'));
        assert_eq!(shown.chars().count(), OPAQUE_PREVIEW + 1);
    }

    #[test]
    fn a_truncated_utf8_body_does_not_panic() {
        // Bytes, not a string: a response cut mid-character is exactly what a dropped
        // connection produces, and it must render rather than blow up.
        let bytes = "привет".as_bytes();
        let shown = preview(&bytes[..bytes.len() - 1]);
        assert!(shown.starts_with("приве"));
    }

    #[test]
    fn a_client_is_on_the_json_wire_unless_it_is_told_otherwise() {
        // ECMA-431's wire, and the behaviour every test above asserts: `Client::new` is
        // unchanged in signature and in what it means.
        let (client, _) = client(Vec::new());
        assert_eq!(client.wire(), Wire::Json);
    }

    #[cfg(feature = "cbor")]
    #[tokio::test]
    async fn a_client_on_the_cbor_wire_round_trips_a_message() {
        // ECMA-432 §7.1's binary frame, in both directions. A client that encoded on one
        // wire and decoded on the other would pass every JSON test above and fail here:
        // the request would not be a frame the peer can read, and the reply would come
        // back as `Body::Opaque` blaming the peer for this end's misreading.
        let asked = ask("What is Ecma?");
        let answered = Message::text("Ecma is a standards body.")
            .with_submessage(Submessage::token("conversation", "c1"));

        let scripted = Arc::new(Scripted::new(vec![Ok(Exchange::http(
            200,
            answered.to_cbor_vec(),
        ))]));
        let client = Client::new(Arc::clone(&scripted)).on_wire(Wire::Cbor);

        assert_eq!(client.wire(), Wire::Cbor);

        let reply = client.send(&asked, &Context::new()).await.unwrap();

        assert_eq!(reply.text().as_deref(), Some("Ecma is a standards body."));
        assert_eq!(reply.conversation_tokens(), vec!["c1".to_string()]);

        // And the request left as CBOR: it decodes on that wire and is the message that
        // was asked. `sent_json` would panic on these bytes, which is the other half of
        // the same statement.
        let sent = scripted
            .sent
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(sent.len(), 1);
        assert_eq!(Message::from_cbor_slice(&sent[0]).unwrap(), asked);
    }

    #[test]
    fn encoding_and_decoding_are_reachable_without_a_client() {
        // A JavaScript package over this crate has its own transport and wants only
        // these two, which is why they are free functions.
        let message = Message::text("What is Ecma?");
        let bytes = encode_request(&message, Wire::Json);
        assert_eq!(decode_reply(&bytes, Wire::Json), Body::Message(message));
    }
}
