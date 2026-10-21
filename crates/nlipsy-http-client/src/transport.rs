//! The ECMA-431 binding of [`nlipsy_core::Transport`]: one `POST` over HTTP.
//!
//! One exchange is one `POST` carrying one JSON message and returning one. There is no
//! stream, no subscription and no long-lived connection: a conversation is a sequence
//! of independent requests that happen to carry the same identifier, which is why so
//! little of this module is about transport.
//!
//! Everything that is *not* HTTP — the [`Transport`] trait itself, `Client`, `Reply`,
//! `Body` and its opaque variant, the three-way split of [`TransportError`] — belongs to
//! `nlipsy-core`, which this crate's root re-exports whole so that a consumer can name
//! one crate. Everything that is HTTP but is not a *socket* — the path ECMA-431 fixes,
//! the correlator header, the media type, the two directions of `Retry-After`, the
//! canonical chat request — belongs to `nlipsy-http`, which the server binding reads
//! the same values out of.
//!
//! What is left here is the socket, and since 2026-10-04 there are two of it:
//!
//! | type | feature | the stack under it |
//! |---|---|---|
//! | [`HttpTransport`] | `reqwest` (**default**) | reqwest 0.13 on rustls, verifying against the platform's trust store |
//! | [`HyperTransport`] | `hyper` | hyper + hyper-rustls on the `ring` provider, verifying against a compiled-in root bundle |
//!
//! They are two types rather than one type with a knob, because the choice is a
//! *dependency graph* and not a run-time setting: the whole reason the second exists is
//! that reqwest's TLS cannot be built for an Android or an iOS app without `jni` and a
//! vendored BoringSSL, and a knob cannot remove a dependency. [`HyperTransport`]'s own
//! documentation has the measurement and the price.
//!
//! They are in **one module** on purpose, and the reason is the contract rather than
//! tidiness. What a caller of this crate relies on is not which client library ran but
//! which [`TransportError`] case comes back for which situation, that `Retry-After` is
//! read in both of RFC 9110 §10.2.3's forms, and that the correlator is sent only when
//! there is one. Two implementations of that can only be kept honest by being readable
//! against each other, so the two `Transport` impls sit here with one `Retry-After`
//! reader between them and the error mappings a few lines apart.

use std::time::Duration;

#[cfg(any(feature = "hyper", feature = "reqwest"))]
use std::time::SystemTime;

#[cfg(feature = "hyper")]
use std::{io, pin::Pin, sync::Arc, task::Poll};

#[cfg(feature = "hyper")]
use hyper_util::client::legacy::connect::{Connected, Connection, HttpConnector};
#[cfg(feature = "hyper")]
use hyper_util::rt::TokioIo;
#[cfg(any(feature = "hyper", feature = "reqwest"))]
use nlipsy_core::{Context, Exchange, ExchangeDetail, Transport, TransportError};
use thiserror::Error;

/// The header map a deployment hands a builder's `headers`.
///
/// Re-exported because it is in this crate's public API and a consumer has to be able
/// to name the same version of it — the reason jsonrpsee re-exports it from
/// `http-client` too. It is `http`'s type, the same one `nlipsy_core::http` re-exports
/// for `Extensions`, and it is reached through that re-export rather than through a
/// client library: reqwest does not define one of its own (0.13's
/// `reqwest::header::HeaderMap` *is* this type) and neither does hyper, so one spelling
/// serves both transports — and it is still here in a build with neither of them,
/// where the rest of this module is nothing.
pub use nlipsy_core::http::header::{HeaderMap, HeaderValue};

// ECMA-431 §6's path, the correlator header this binding spells, and §6.2.1's token
// subformat. They used to be declared here *and* in `nlipsy-server`, with the
// duplication argued for — "the two are separate implementations of the same clause,
// and a two-token constant that both sides must agree on fails loudly when it drifts,
// not silently". That argument was evidence while the two lived in different
// repositories; in one workspace it was a second copy to keep in step. One copy now, in
// `nlipsy-http`, re-exported here so that the paths a caller already wrote keep working.
//
// `NLIP_PATH` is still not appended to anything by this crate: a transport is given the
// whole URL a deployment published, because an end-point may sit behind a prefix a
// client cannot derive, and the constant is what a caller composing one from a host and
// a port uses.
pub use nlipsy_http::{CONVERSATION_SUBFORMAT, CORRELATOR_HEADER, NLIP_PATH, request};

/// How long a transport waits for a connection before giving up.
///
/// The default for both — [`HttpTransport::new`] and [`HyperTransport::new`] — and the
/// connect phase and nothing else: see [`HttpTransportBuilder::connect_timeout`] for
/// why there is no bound on the exchange as a whole.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Why a transport could not be built.
///
/// Four cases in practice: reqwest could not start a TLS backend; rustls refused the
/// `ring` provider's protocol versions; the end-point a caller gave is not a URI; or a
/// root certificate handed to [`HyperTransportBuilder::add_root_certificate`] is not one
/// rustls can use as a trust anchor. [`HyperTransportBuilder::build`] finds the last two
/// because it checks both once there rather than once per exchange — the bytes are wrong
/// where the caller supplied them, and that is where they are told so.
///
/// It is a `String` because the only thing a caller does with it is put it in a message,
/// and because a `reqwest::Error` or a `rustls::Error` in the public API would oblige
/// every consumer to depend on the same release of that crate to name it — which, for a
/// crate whose two transports exist precisely so that a consumer need not compile both
/// stacks, would defeat the split.
#[derive(Debug, Error)]
#[error("the HTTP client could not be built: {detail}")]
pub struct BuildError {
    detail: String,
}

/// `Retry-After` as a delay, in both of RFC 9110 §10.2.3's forms.
///
/// One reader for both transports, and sharing it is not only about duplication: a
/// caller acts on the delay — a `429` is worth waiting out — so two readers that drifted
/// would make the same end-point pace two builds of this crate differently. The rule is
/// `nlipsy-http`'s and the clock is this crate's: `parse_retry_after` takes `now` as an
/// argument precisely so that the rule has none.
///
/// The parameter is `http`'s [`HeaderMap`], which is what lets one function serve both:
/// reqwest's `Response::headers` and hyper's return the same type.
#[cfg(any(feature = "hyper", feature = "reqwest"))]
fn read_retry_after(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get(nlipsy_core::http::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| nlipsy_http::parse_retry_after(value, SystemTime::now()))
}

/// The real transport: one `POST` per exchange, over HTTP or HTTPS.
///
/// Verification goes through rustls-platform-verifier, so it reads the **system** trust
/// store: an internal CA the machine already trusts is trusted here, and a container
/// image carrying this client needs `ca-certificates`. That is the right default for a
/// server, a daemon or a desktop binary, and it is the wrong one for a mobile app —
/// [`HyperTransport`] is that build, and says why.
#[cfg(feature = "reqwest")]
#[cfg_attr(docsrs, doc(cfg(feature = "reqwest")))]
#[derive(Debug)]
pub struct HttpTransport {
    http: reqwest::Client,
    endpoint: String,
}

#[cfg(feature = "reqwest")]
impl HttpTransport {
    /// Build a transport for `endpoint`, e.g. `https://agent.example/nlip`.
    ///
    /// The whole URL, not a host: an end-point may be published behind a prefix, and
    /// [`NLIP_PATH`] is what a caller composing one from a host and a port appends.
    ///
    /// # Panics
    ///
    /// When reqwest cannot build a client at all — in practice, a TLS backend that
    /// would not start. A transport that cannot make any request is not a condition a
    /// caller can do anything with; [`HttpTransport::builder`] returns the failure as a
    /// value for a caller that would rather decide.
    pub fn new(endpoint: impl Into<String>) -> HttpTransport {
        HttpTransport::builder(endpoint)
            .build()
            .expect("reqwest client")
    }

    /// A builder for `endpoint`, for a caller who wants the connect budget or a header
    /// on every request.
    pub fn builder(endpoint: impl Into<String>) -> HttpTransportBuilder {
        HttpTransportBuilder {
            endpoint: endpoint.into(),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            headers: HeaderMap::new(),
        }
    }

    /// Where this transport sends, as it was given.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

/// Builds an [`HttpTransport`].
///
/// Deliberately small. Every option here is one the reference deployment actually
/// sets; a knob that no caller turns is a promise about behaviour that nothing checks.
#[cfg(feature = "reqwest")]
#[cfg_attr(docsrs, doc(cfg(feature = "reqwest")))]
#[derive(Debug)]
pub struct HttpTransportBuilder {
    endpoint: String,
    connect_timeout: Duration,
    headers: HeaderMap,
}

#[cfg(feature = "reqwest")]
impl HttpTransportBuilder {
    /// How long to wait for a connection. Default [`DEFAULT_CONNECT_TIMEOUT`].
    ///
    /// There is deliberately **no request timeout** to go with it, and that asymmetry
    /// is the whole design. A model-backed end-point may legitimately think for
    /// minutes on one request, and a client has no way to learn its budget. Abandoning
    /// a request the end-point is still working on would leave the turn recorded on one
    /// side and not the other — precisely the divergence
    /// [`TransportError::Interrupted`] exists to warn about. Waiting is the caller's
    /// decision, and a caller that wants a limit puts one around the call, where it can
    /// be the caller's own number.
    ///
    /// The connect phase is different: a connection that was never established cannot
    /// have left a turn anywhere, so giving up on it is safe and reportable.
    pub fn connect_timeout(mut self, timeout: Duration) -> HttpTransportBuilder {
        self.connect_timeout = timeout;
        self
    }

    /// Headers sent with every exchange.
    ///
    /// For what a deployment adds around the protocol — an API gateway's key, a tenant
    /// label — not for the protocol itself: [`CORRELATOR_HEADER`] is per exchange and
    /// comes from [`Context::correlator`], and `Content-Type` is this binding's, fixed
    /// at `application/json` — the media type ECMA-433 §6.1.5 fixes for the same
    /// payload, over a JSON encoding that is ECMA-430 §5 ¶2's. ECMA-431 §6 names
    /// neither: it fixes the URL and the method and no more.
    /// Replaces whatever was set before, as reqwest's own `default_headers` does.
    pub fn headers(mut self, headers: HeaderMap) -> HttpTransportBuilder {
        self.headers = headers;
        self
    }

    /// Build the transport.
    pub fn build(self) -> Result<HttpTransport, BuildError> {
        let http = reqwest::Client::builder()
            .connect_timeout(self.connect_timeout)
            .default_headers(self.headers)
            .build()
            .map_err(|error| BuildError {
                detail: error.to_string(),
            })?;
        Ok(HttpTransport {
            http,
            endpoint: self.endpoint,
        })
    }
}

#[cfg(feature = "reqwest")]
impl Transport for HttpTransport {
    async fn exchange(&self, cx: &Context, body: Vec<u8>) -> Result<Exchange, TransportError> {
        let mut request = self
            .http
            .post(&self.endpoint)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body);
        // Only when the caller established one. The header is this binding's convention
        // rather than ECMA-431's, and sending an empty one would be a claim that there
        // is a turn to correlate with when there is not.
        if let Some(correlator) = &cx.correlator {
            request = request.header(CORRELATOR_HEADER, correlator);
        }

        let response = request.send().await.map_err(|error| {
            // reqwest distinguishes a failure to connect from a failure after the
            // request was sent, and that is the distinction the retry policy turns on —
            // so it is preserved rather than flattened into a string.
            let detail = error.to_string();
            if error.is_connect() {
                TransportError::Unreachable {
                    endpoint: self.endpoint.clone(),
                    detail,
                }
            } else {
                TransportError::Interrupted {
                    endpoint: self.endpoint.clone(),
                    detail,
                }
            }
        })?;

        let status = response.status().as_u16();
        // Both RFC 9110 §10.2.3 forms, because an end-point may send either and this
        // is the side that has to read what it is given.
        let retry_after = read_retry_after(response.headers());

        let body = response
            .bytes()
            .await
            .map_err(|error| TransportError::Interrupted {
                endpoint: self.endpoint.clone(),
                detail: format!("reply headers arrived but the body did not: {error}"),
            })?;

        Ok(Exchange::new(
            body.to_vec(),
            ExchangeDetail::Http {
                status,
                retry_after,
            },
        ))
    }
}

/// What [`HyperTransport`] holds: hyper-util's pooling client over hyper-rustls, sending
/// bodies of known length.
///
/// A type alias because the real one is four nested generics and appears twice.
/// `Full<Bytes>` is the choice that matters: a body whose length is known is what gives
/// the request a `Content-Length` instead of a chunked encoding, and an end-point
/// reading one NLIP message has no reason to be handed a stream.
///
/// One concrete type whether or not a [`Dialer`] was given: the connector under the TLS
/// layer is [`Connector`], an enum of the two, which is what keeps [`HyperTransport`]
/// free of a type parameter.
#[cfg(feature = "hyper")]
type HyperClient = hyper_util::client::legacy::Client<
    hyper_rustls::HttpsConnector<Connector>,
    http_body_util::Full<hyper::body::Bytes>,
>;

/// The same `POST`, for a build that cannot carry a platform verifier.
///
/// # Why this exists
///
/// [`HttpTransport`] is reqwest 0.13, and reqwest 0.13's `rustls` feature — and its
/// `rustls-no-provider` feature too — depends on **rustls-platform-verifier**
/// unconditionally. On `aarch64-linux-android` that means `jni` and a Kotlin companion
/// library the app has to ship and initialise; on every target it means **aws-lc-sys**,
/// a vendored BoringSSL built by **cmake**. None of it can be switched off from a
/// consumer's manifest, because a feature cannot remove a dependency.
///
/// This transport is the same exchange over hyper and hyper-rustls on rustls's `ring`
/// provider, with Mozilla's root bundle compiled in from `webpki-roots`. Measured on
/// 2026-10-04 with `cargo tree -e normal,build` for `aarch64-linux-android` and
/// `aarch64-apple-ios`, what is left of that list is **`cc`, `ring` and
/// `webpki-roots`** — and `make check-mobile-graph` is the gate that keeps it so,
/// failing on any of `jni`, `rustls-platform-verifier`, `aws-lc`, `cmake`,
/// `security-framework`, `openssl` or `native-tls` appearing in either tree.
///
/// # What it gives up
///
/// **The system trust store.** The roots are a bundle frozen at build time, plus whatever
/// the application hands over itself, so:
///
/// - an enterprise or internal CA the device trusts is *not* trusted here because the
///   device trusts it. The application can add it — DER, one call per root, through
///   [`HyperTransportBuilder::add_root_certificate`], *in addition to* Mozilla's bundle
///   — but the transport never reads the device's store, so a CA the machine was told
///   about stays unknown here until the application is told about it too;
/// - the platform's proxy settings are not consulted. A consumer with a route of its
///   own — a proxy, an in-process tunnel — expresses it as a [`Dialer`] instead;
/// - there is no certificate pinning, and no revocation data beyond what the
///   certificates carry;
/// - a root added or removed by Mozilla reaches a deployment only when it is rebuilt
///   against a newer `webpki-roots`.
///
/// A deployment that needs the platform's own judgement has two honest options: take
/// [`HttpTransport`], which asks the platform; or hand this crate's `Transport` seam the
/// platform's own HTTP stack — `URLSession`, `OkHttp` — as an implementation of
/// [`nlipsy_core::Transport`], which is one method and is what the trait is for.
///
/// **It is not a runtime.** Hyper needs Tokio and this type starts none: a
/// [`HyperTransport`] is built and used inside whatever reactor the consumer already
/// has, single-threaded or not, and spawns nothing beyond hyper-util's own connection
/// tasks.
///
/// Everything else is the same contract as [`HttpTransport`], deliberately and
/// checked: `POST`, `application/json`, the correlator only when there is one, both of
/// RFC 9110 §10.2.3's `Retry-After` forms, and the same [`TransportError`] case for the
/// same situation.
#[cfg(feature = "hyper")]
#[cfg_attr(docsrs, doc(cfg(feature = "hyper")))]
pub struct HyperTransport {
    http: HyperClient,
    /// The end-point, parsed. Hyper wants a `Uri` and parsing it once at build time is
    /// what turns "that is not a URL" into a [`BuildError`] a caller is handed where
    /// they supplied the string, instead of into a failed exchange per request.
    uri: hyper::Uri,
    /// And as it was given, which is what every error reports and [`Self::endpoint`]
    /// returns: a caller recognises the string they wrote, not its normalisation.
    endpoint: String,
    /// Applied per request. hyper-util's client has no `default_headers` of its own.
    headers: HeaderMap,
    /// The caller's dialer, when there is one — kept here as well as inside the
    /// connector for `Debug` alone, because the client that holds the connector prints
    /// as `Client` and nothing more.
    dialer: Option<Arc<dyn Dialer>>,
}

/// By hand, for one fact the derived impl would hide: whether this transport opens its
/// own sockets or a caller's [`Dialer`] does. hyper-util's `Client` renders as `Client`
/// and nothing else, so a derived `Debug` would print every field but the one a reader
/// of a log, chasing a connection that went the wrong way, needs first.
#[cfg(feature = "hyper")]
impl std::fmt::Debug for HyperTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HyperTransport")
            .field("endpoint", &self.endpoint)
            .field("headers", &self.headers)
            .field("dialer", &self.dialer)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "hyper")]
impl HyperTransport {
    /// Build a transport for `endpoint`, e.g. `https://agent.example/nlip`.
    ///
    /// The whole URL, not a host, exactly as [`HttpTransport::new`] takes it.
    ///
    /// # Panics
    ///
    /// When the transport cannot be built at all: `endpoint` is not a URI, or rustls
    /// refused the `ring` provider. Neither is a condition a caller can act on at the
    /// point of a request — the first is a bug in what was passed in and the second
    /// would fail every exchange — so, as with [`HttpTransport::new`], the convenience
    /// constructor panics and [`HyperTransport::builder`] returns the failure as a
    /// value. Note the difference from the reqwest constructor, which does not parse
    /// the end-point at all and therefore cannot panic on a malformed one.
    pub fn new(endpoint: impl Into<String>) -> HyperTransport {
        HyperTransport::builder(endpoint)
            .build()
            .expect("hyper client")
    }

    /// A builder for `endpoint`, for a caller who wants the connect budget or a header
    /// on every request.
    pub fn builder(endpoint: impl Into<String>) -> HyperTransportBuilder {
        HyperTransportBuilder {
            endpoint: endpoint.into(),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            headers: HeaderMap::new(),
            dialer: None,
            extra_roots: Vec::new(),
        }
    }

    /// Where this transport sends, as it was given.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

/// Builds a [`HyperTransport`].
///
/// The same two options as [`HttpTransportBuilder`] — the connect budget and the headers
/// — meaning the same two things. A transport whose builder offered a different set *of
/// those* would be a second contract wearing the first one's name.
///
/// The other two are this transport's alone, and neither touches that contract:
/// [`dialer`](Self::dialer) changes where the bytes go and
/// [`add_root_certificate`](Self::add_root_certificate) who is trusted to receive them,
/// and the same situation still produces the same [`TransportError`] case through
/// either. They are not on [`HttpTransportBuilder`] because nothing asks for them there:
/// the reqwest transport already reads the platform's store, which is where an internal
/// CA lives on the machines it runs on, and the consumer that routes its own traffic —
/// an app with a tunnel inside it — is the FFI build, which is this one.
#[cfg(feature = "hyper")]
#[cfg_attr(docsrs, doc(cfg(feature = "hyper")))]
pub struct HyperTransportBuilder {
    endpoint: String,
    connect_timeout: Duration,
    headers: HeaderMap,
    dialer: Option<Arc<dyn Dialer>>,
    /// DER, as given, and parsed only in `build` — which is what makes a bad one a
    /// [`BuildError`] there rather than a panic or a `Result` from every setter call.
    extra_roots: Vec<Vec<u8>>,
}

/// By hand for one field: a DER certificate is a kilobyte of numbers, and what a reader of
/// a builder's `Debug` wants to know is how many roots were added, not their bytes.
#[cfg(feature = "hyper")]
impl std::fmt::Debug for HyperTransportBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HyperTransportBuilder")
            .field("endpoint", &self.endpoint)
            .field("connect_timeout", &self.connect_timeout)
            .field("headers", &self.headers)
            .field("dialer", &self.dialer)
            .field("extra_roots", &self.extra_roots.len())
            .finish()
    }
}

#[cfg(feature = "hyper")]
impl HyperTransportBuilder {
    /// How long to wait for a connection. Default [`DEFAULT_CONNECT_TIMEOUT`].
    ///
    /// The same budget, the same asymmetry, and the same reason there is **no request
    /// timeout** beside it — see [`HttpTransportBuilder::connect_timeout`], which is
    /// where that argument is written out.
    ///
    /// It is applied by `HttpConnector::set_connect_timeout` rather than by a timeout
    /// around the exchange, which is the only place it *can* be applied and still mean
    /// the connect phase: hyper-util's pooling client has no timeout of its own, and a
    /// `tokio::time::timeout` around `Client::request` would bound the end-point's
    /// thinking time as well — the very thing the paragraph above refuses to do.
    /// hyper-util divides the budget evenly across a name's addresses, so a host that
    /// resolves to four of them still gives up at this number rather than at four times
    /// it.
    ///
    /// **With a [`dialer`](Self::dialer) the same number bounds something different**,
    /// and the two are worth telling apart before choosing it. On the default path the
    /// name is resolved *first, with no budget at all* — `getaddrinfo` takes as long as
    /// the system resolver takes — and only then is the budget split across the
    /// addresses that came back. With a dialer there is no resolution step this crate can
    /// see, so the budget is a `tokio::time::timeout` around the whole of
    /// [`Dialer::dial`]: the dialer's own name lookup, its choice of route and its
    /// connect, all inside this one number, and an expiry is
    /// [`TransportError::Unreachable`] with `connect timed out after …` in its detail.
    /// That timeout wraps the dial and nothing after it, so the refusal above — no bound
    /// on the exchange — holds on both paths. On neither is the TLS handshake that
    /// follows the connect inside the budget.
    pub fn connect_timeout(mut self, timeout: Duration) -> HyperTransportBuilder {
        self.connect_timeout = timeout;
        self
    }

    /// Open connections through `dialer`, instead of resolving names and opening sockets
    /// here.
    ///
    /// Without this — the default — the transport is exactly what it was before dialers
    /// existed: hyper-util's `HttpConnector`, the system resolver, one TCP socket per
    /// connection, the connect budget split across a name's addresses. With it, the
    /// transport resolves **no** name and opens **no** socket of its own: each new
    /// connection is one call to [`Dialer::dial`] with the end-point's host and port, and
    /// TLS, HTTP/1.1 and the pool run over whatever stream comes back. [`Dialer`] is
    /// where the contract an implementation is held to is written out — one dial per
    /// connection, cancel safety, the URI's host as the name TLS verifies, the budget
    /// around the whole dial.
    ///
    /// The pool is this transport's own, so a dialed stream is only ever reused by the
    /// transport that dialed it. A consumer whose route changes — a tunnel coming up, so
    /// that connections opened outside it must not outlive the switch — builds a new
    /// transport and drops the old one, and the old route's connections go with it.
    /// That is the whole of the invalidation story, deliberately: there is no "flush the
    /// pool" call to forget.
    ///
    /// An `Arc<dyn Dialer>` rather than a type parameter, so that [`HyperTransport`] stays
    /// one type whatever is dialing for it. Replaces a dialer set before.
    pub fn dialer(mut self, dialer: Arc<dyn Dialer>) -> HyperTransportBuilder {
        self.dialer = Some(dialer);
        self
    }

    /// Trust `der` as a root certificate **in addition to** Mozilla's bundle.
    ///
    /// For the end-point behind a private CA — a home or a company service whose
    /// certificate no public root signed — which the compiled-in bundle cannot verify and
    /// the system store, which might, is never consulted (see "What it gives up" on
    /// [`HyperTransport`]). `der` is one DER-encoded X.509 certificate, the CA's own; call
    /// this once per root, as many times as there are roots. Nothing is ever removed:
    /// the Mozilla roots stay trusted, because a transport that stopped trusting the
    /// public internet the moment a private root was added would be a surprise nobody
    /// asked for.
    ///
    /// DER, and there is no PEM variant: PEM is a text wrapper around these same bytes,
    /// a consumer that keeps its CA as PEM already has a parser for its own
    /// configuration, and a second entry point would be a second set of failure modes
    /// for one input.
    ///
    /// A `der` that is not a certificate rustls can use as a trust anchor fails
    /// [`build`](Self::build) with a [`BuildError`] saying which one it was, rather than
    /// failing every exchange later. And `Vec<u8>` rather than rustls's `CertificateDer`
    /// for the reason [`BuildError`] is a string: a rustls type in this signature would
    /// oblige every caller to depend on the same release of rustls to name it.
    pub fn add_root_certificate(mut self, der: Vec<u8>) -> HyperTransportBuilder {
        self.extra_roots.push(der);
        self
    }

    /// Headers sent with every exchange.
    ///
    /// For what a deployment adds around the protocol, exactly as
    /// [`HttpTransportBuilder::headers`] describes it, and the same `HeaderMap` type:
    /// `Content-Type` and [`CORRELATOR_HEADER`] are this binding's and are written
    /// after these, so a map that carries either does not displace the protocol's own.
    /// Replaces whatever was set before.
    pub fn headers(mut self, headers: HeaderMap) -> HyperTransportBuilder {
        self.headers = headers;
        self
    }

    /// Build the transport.
    pub fn build(self) -> Result<HyperTransport, BuildError> {
        let uri = self
            .endpoint
            .parse::<hyper::Uri>()
            .map_err(|error| BuildError {
                detail: format!("{} is not a URI: {error}", self.endpoint),
            })?;

        // Mozilla's bundle, compiled in. This is the line the whole feature is about,
        // and `HyperTransport`'s documentation is where its price is written.
        let mut roots = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.into(),
        };
        // And the caller's own roots *added* to it, never in place of it. Parsed here,
        // once, so that a certificate rustls cannot use is a `BuildError` naming which
        // of the caller's roots it was — counted from one, as a person counts the calls
        // they wrote — instead of a handshake failure on every exchange.
        for (index, der) in self.extra_roots.into_iter().enumerate() {
            roots
                .add(rustls::pki_types::CertificateDer::from(der))
                .map_err(|error| BuildError {
                    detail: format!(
                        "root certificate #{} given to add_root_certificate is not a \
                         usable trust anchor: {error}",
                        index + 1
                    ),
                })?;
        }

        // `builder_with_provider`, and **never** `ClientConfig::builder()`. The implicit
        // builder reads a process-wide default provider and, when none has been
        // installed, panics at run time if rustls was compiled with more than one — and
        // in this workspace it is: a build that takes both of this crate's features gets
        // `ring` from here and `aws-lc-rs` through reqwest's platform verifier, because
        // cargo unifies features additively and `rustls/aws_lc_rs` cannot be unasked
        // for. Naming the provider is what makes the transport independent of what else
        // is in the binary.
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|error| BuildError {
                detail: format!("rustls refused the ring provider: {error}"),
            })?
            .with_root_certificates(roots)
            .with_no_client_auth();

        let connector = match &self.dialer {
            // The default path, and it is 0.1.1's connector line for line, held as it is
            // in the enum's arm — nothing boxed and nothing re-implemented — so a build
            // that never names a dialer resolves, connects and times out exactly as it
            // did before dialers existed.
            None => {
                let mut connector = HttpConnector::new();
                // `enforce_http(false)` so the connector will accept an `https` URI at
                // all — hyper-rustls wraps it and does the handshake itself — and it is
                // also what hyper-rustls's own `build()` does to the connector it makes.
                connector.enforce_http(false);
                connector.set_connect_timeout(Some(self.connect_timeout));
                Connector::Tcp(connector)
            }
            // The caller's route. The budget travels with the dialer because there is no
            // `HttpConnector` to hold it; `Connector::call` is where it is applied.
            Some(dialer) => Connector::Dialed {
                dialer: Arc::clone(dialer),
                connect_timeout: self.connect_timeout,
            },
        };

        // `https_or_http`, not `https_only`, and that is a deliberate refusal rather
        // than an oversight: ECMA-430 §7.1 obliges encryption of a *deployment* ("must
        // happen over a communication channel that uses encryption"), which is the
        // reading `nlipsy-server`'s README states for the other end of the same binding
        // — a `Router` is served by whatever serves it. A transport that refused `http`
        // would not be enforcing that clause, it would be deciding where a deployment
        // may terminate TLS, and it would stop this crate's own tests and every
        // loopback fake provider from working. `HttpTransport` accepts plain `http` for
        // the same reason, so refusing it here would also make the two transports two
        // contracts.
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config(config)
            .https_or_http()
            .enable_http1()
            .wrap_connector(connector);

        let http =
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                // The pool's idle timeout is a duration and a duration needs a clock.
                // Without a timer hyper-util keeps the setting and never acts on it, so an
                // idle connection is held until the peer drops it — which on a mobile
                // radio is the one cost worth avoiding.
                .pool_timer(hyper_util::rt::TokioTimer::new())
                .build(connector);

        Ok(HyperTransport {
            http,
            uri,
            endpoint: self.endpoint,
            headers: self.headers,
            dialer: self.dialer,
        })
    }
}

/// Opens the byte stream a [`HyperTransport`] speaks HTTP over, in place of a socket of
/// its own.
///
/// # What it is for
///
/// By default the transport resolves the end-point's name with the system resolver —
/// `getaddrinfo`, through hyper-util's `HttpConnector` — and opens a TCP socket itself.
/// That is the wrong thing for a consumer that owns the route: an in-process userspace
/// tunnel (WireGuard over smoltcp, say, whose names are answered by the DNS server in the
/// tunnel's configuration), a SOCKS hop, a test harness with no network at all. Handed a
/// dialer through [`HyperTransportBuilder::dialer`], the transport resolves **no** name
/// and opens **no** socket: every connection it makes starts as one call to
/// [`dial`](Self::dial) with the end-point's host and port, and the stream that comes back
/// is where the request goes. Everything above the stream is unchanged — TLS for `https`,
/// HTTP/1.1, the pool, which [`TransportError`] case comes back for which situation.
///
/// # What an implementation can rely on, and what it owes
///
/// **One `dial` per new connection, and reuse only through the pool.** The transport
/// calls `dial` once for each connection it opens, and not at all for a request an idle
/// pooled connection can carry. That pool belongs to the one [`HyperTransport`] it was
/// built into: two transports share nothing, dropping a transport closes its idle
/// connections (one still carrying an exchange closes when that exchange ends), and so a
/// consumer whose route changes builds a new transport rather than hoping the old one
/// forgets.
///
/// **The future may be dropped at any point, so it has to be cancel-safe.** The connect
/// budget running out drops it; so does a caller abandoning an exchange while its
/// connection is still being dialed. Whatever a half-finished dial holds — a socket
/// being opened inside a tunnel, a DNS query in flight, a slot in a connection table —
/// must be released when its future is dropped, not only when it completes.
///
/// One case is the opposite of a cancellation, and an implementation should expect it:
/// when a pooled connection to the same end-point becomes idle while a new one is still
/// being dialed, hyper-util hands the waiting request the pooled connection and lets the
/// started dial run to completion in the background, pooling its stream rather than
/// wasting it. So a burst of exchanges can produce a `dial` whose stream ends up idle in
/// the pool rather than carrying the request that prompted it. Never one shared across
/// transports, though: the pool it lands in is the same transport's.
///
/// **The name is verified, not the route.** For an `https` end-point the transport layers
/// TLS over the dialed stream with the same rustls configuration as the default path, and
/// the name it sends as SNI and verifies the certificate against is **the URI's host**,
/// not wherever the dialer actually connected. A dialer may carry `nlip.example` through a
/// tunnel to `10.0.0.7` and the certificate must still be valid for `nlip.example` —
/// which is what makes it safe to let a dialer choose the route at all. An `http`
/// end-point gets plain HTTP over the stream, exactly as the default path would.
///
/// **The connect budget bounds the whole `dial`.**
/// [`HyperTransportBuilder::connect_timeout`] is applied to the future `dial` returns,
/// whatever name resolution happens inside it included, and when it runs out the future
/// is dropped and the exchange fails as [`TransportError::Unreachable`] with
/// `connect timed out after …` in its detail. That is a different bound from the default
/// path's — `connect_timeout` says how — and on neither path does it cover the TLS
/// handshake that follows.
///
/// **A failure is the caller's to read.** The `io::Error` a dial fails with reaches the
/// caller as [`TransportError::Unreachable`], with its message and its whole `source`
/// chain in the detail. So the reason worth showing a person — "the tunnel has had no
/// handshake for 35 s", "the tunnel's DNS did not answer in 5 s" — belongs in that error,
/// where it will be read word for word. `Unreachable` rather than `Interrupted`, because
/// a request whose connection never opened cannot have reached the end-point and is safe
/// to repeat.
///
/// # Why this shape
///
/// A trait object, `Arc<dyn Dialer>`, rather than a type parameter, so that
/// [`HyperTransport`] stays one concrete type a consumer can store, name across an FFI
/// boundary, and rebuild with a different dialer without its own type changing. That is
/// also why [`dial`](Self::dial) returns a boxed future ([`DialFuture`]) rather than
/// being an `async fn`: an `async fn` in a trait cannot be called through `dyn`. And the
/// stream is in Tokio's `AsyncRead` + `AsyncWrite` terms ([`DialStream`]) rather than in
/// hyper's own I/O traits, because those are the traits a `tokio::net::TcpStream` and a
/// tunnel's socket adapter already implement: an implementation names Tokio, which it
/// has, and not hyper, which it should not need.
///
/// `Debug` is a supertrait so that [`HyperTransport`]'s own `Debug` can say which dialer
/// it was built with.
#[cfg(feature = "hyper")]
#[cfg_attr(docsrs, doc(cfg(feature = "hyper")))]
pub trait Dialer: Send + Sync + std::fmt::Debug + 'static {
    /// Open a byte stream to `target`.
    ///
    /// The name in `target.host` is the dialer's to resolve, through whichever resolver
    /// its route calls for, or to pass on unresolved; this transport never looks it up.
    fn dial(&self, target: DialTarget) -> DialFuture;
}

/// Where a [`Dialer`] is asked to connect: the end-point's host and port, as its URI
/// spells them.
///
/// `host` is the URI's host **as written** — a DNS name for the dialer to resolve, or an
/// IP literal — with the brackets of an IPv6 literal removed, which is the form a resolver
/// or a socket-address parser expects: `https://[::1]:8443/nlip` gives `::1`. `port` is
/// the URI's explicit port or, when it has none, the scheme's default: 443 for `https`, 80
/// for anything else. Both are the rule hyper-util's `HttpConnector` applies to the same
/// URI, so a dialer is asked for exactly the host and port the default path would have
/// connected to.
///
/// `#[non_exhaustive]` because only this crate constructs one, and a field it may want to
/// add later — the scheme, so that a dialer can tell a TLS connection from a plain one —
/// must not break every implementation that reads these two.
#[cfg(feature = "hyper")]
#[cfg_attr(docsrs, doc(cfg(feature = "hyper")))]
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DialTarget {
    /// The URI's host: a DNS name or an IP literal, an IPv6 one without its brackets.
    pub host: String,
    /// The URI's port, or the scheme's default — 443 for `https`, 80 otherwise.
    pub port: u16,
}

/// What [`Dialer::dial`] returns: the stream, or why there is none, as a boxed future.
///
/// Boxed because a method called through `dyn` cannot return `impl Future` (see
/// [`Dialer`]); `Send` because hyper-util drives a connect on whichever thread the
/// runtime picks.
#[cfg(feature = "hyper")]
#[cfg_attr(docsrs, doc(cfg(feature = "hyper")))]
pub type DialFuture = Pin<Box<dyn Future<Output = io::Result<DialedStream>> + Send>>;

/// A byte stream in Tokio's terms, which is all a [`Dialer`] has to produce.
///
/// Implemented for every `AsyncRead + AsyncWrite + Send + Unpin + 'static` type — a
/// `tokio::net::TcpStream`, a `tokio::io::DuplexStream`, a tunnel's socket adapter — so
/// there is nothing to implement: the trait exists to give that bound a name a trait
/// object can carry, which is [`DialedStream`]. `Unpin` because the transport keeps the
/// stream in a box and polls it through a plain `&mut`; `Send` and `'static` because a
/// connection lives in a task of its own for as long as the pool keeps it.
#[cfg(feature = "hyper")]
#[cfg_attr(docsrs, doc(cfg(feature = "hyper")))]
pub trait DialStream:
    tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static
{
}

#[cfg(feature = "hyper")]
impl<T> DialStream for T where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static
{
}

/// The stream a [`Dialer`] hands back, boxed: one type whatever the implementation.
#[cfg(feature = "hyper")]
#[cfg_attr(docsrs, doc(cfg(feature = "hyper")))]
pub type DialedStream = Box<dyn DialStream>;

/// Every connector error hyper-util and hyper-rustls pass along, whichever arm made it.
#[cfg(feature = "hyper")]
type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The connector under the TLS layer: hyper-util's own, or a caller's [`Dialer`].
///
/// An enum, and that is the decision that keeps [`HyperTransport`] one concrete type
/// without charging the path most builds take for it. The alternatives were a type
/// parameter on the transport — which every consumer, and every FFI signature, would then
/// have to spell — or `tower`'s `BoxCloneService`, which is a new dependency and an
/// allocation and a dynamic call per connect on the default path too. With the enum, the
/// `Tcp` arm *is* the connector 0.1.1 built: the same `HttpConnector`, configured by the
/// same lines, its future and its stream held unboxed. What it costs is a `match`.
///
/// `HttpsConnector<Connector>` stays the TLS layer for both arms, which makes "TLS is
/// unchanged by a dialer" true by construction rather than by care: SNI, verification and
/// `https_or_http` are decided a layer above this type, from the URI, and cannot tell
/// which arm produced the stream.
#[cfg(feature = "hyper")]
#[derive(Clone, Debug)]
enum Connector {
    Tcp(HttpConnector),
    Dialed {
        dialer: Arc<dyn Dialer>,
        connect_timeout: Duration,
    },
}

/// What `HttpConnector`'s connect returns. Named by projection because hyper-util does
/// not export the type itself (`HttpConnecting` lives in a private module), and holding
/// it by name rather than boxing it is what leaves the default path unboxed.
#[cfg(feature = "hyper")]
type TcpConnecting = <HttpConnector as tower_service::Service<hyper::Uri>>::Future;

#[cfg(feature = "hyper")]
impl tower_service::Service<hyper::Uri> for Connector {
    type Response = Stream;
    type Error = BoxError;
    type Future = Connecting;

    fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> Poll<Result<(), BoxError>> {
        match self {
            Connector::Tcp(http) => http.poll_ready(cx).map_err(Into::into),
            // Nothing to wait for: whatever a dialer has to wait on, it waits on inside
            // the future `dial` returns, where the connect budget can see it.
            Connector::Dialed { .. } => Poll::Ready(Ok(())),
        }
    }

    fn call(&mut self, destination: hyper::Uri) -> Connecting {
        match self {
            Connector::Tcp(http) => Connecting::Tcp(http.call(destination)),
            Connector::Dialed {
                dialer,
                connect_timeout,
            } => {
                let dialer = Arc::clone(dialer);
                let budget = *connect_timeout;
                Connecting::Dialed(Box::pin(async move {
                    let target = dial_target(&destination)?;
                    // Around the whole dial, the dialer's own name lookup included: on
                    // this path there is no resolution step outside it for the budget to
                    // skip, and `HyperTransportBuilder::connect_timeout` says so. On
                    // expiry the dial future is dropped, which is why `Dialer` asks for
                    // cancel safety.
                    match tokio::time::timeout(budget, dialer.dial(target)).await {
                        Ok(dialed) => dialed,
                        Err(_elapsed) => Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!("connect timed out after {budget:?}"),
                        )),
                    }
                }))
            }
        }
    }
}

/// The [`DialTarget`] for a URI, by the rule `HttpConnector` applies to the same URI.
///
/// The default path runs `enforce_http(false)`, so any scheme is accepted here too and
/// only a missing one is refused, in hyper-util's words. Neither refusal can happen
/// through [`HyperTransport`] as built — hyper-util's client turns away a URI without a
/// scheme and an authority before it reaches a connector — but the function is total
/// rather than trusting that.
#[cfg(feature = "hyper")]
fn dial_target(uri: &hyper::Uri) -> io::Result<DialTarget> {
    let scheme = uri.scheme().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid URL, scheme is missing",
        )
    })?;
    let host = uri.host().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "invalid URL, host is missing")
    })?;
    let port = match uri.port_u16() {
        Some(port) => port,
        None if *scheme == hyper::http::uri::Scheme::HTTPS => 443,
        None => 80,
    };
    Ok(DialTarget {
        // `Uri::host` keeps an IPv6 literal's brackets; a resolver and a socket address
        // want it without them, and `HttpConnector` strips them the same way.
        host: host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned(),
        port,
    })
}

/// The future of a [`Connector`] connect, matching its two arms.
#[cfg(feature = "hyper")]
enum Connecting {
    Tcp(TcpConnecting),
    Dialed(DialFuture),
}

#[cfg(feature = "hyper")]
impl Future for Connecting {
    type Output = Result<Stream, BoxError>;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        // Both arms are `Unpin` — `HttpConnector`'s future is a box inside, and a
        // `DialFuture` is one — so the enum is too and needs no pin projection.
        match self.get_mut() {
            Connecting::Tcp(connecting) => Pin::new(connecting)
                .poll(cx)
                .map_ok(Stream::Tcp)
                .map_err(Into::into),
            Connecting::Dialed(dialing) => dialing
                .as_mut()
                .poll(cx)
                .map_ok(|stream| Stream::Dialed(TokioIo::new(stream)))
                .map_err(Into::into),
        }
    }
}

/// What a [`Connector`] produces, matching its two arms: hyper-util's TCP stream as it
/// comes, or a dialer's stream behind `TokioIo`, which turns Tokio's I/O traits into the
/// ones hyper reads and writes through.
///
/// `Connection` is implemented on the enum rather than per arm, which is what lets the
/// dialed arm hold `TokioIo<DialedStream>` directly: hyper-util implements `Connection`
/// for `TokioIo<T>` only when `T` does, and a caller's stream has no reason to.
#[cfg(feature = "hyper")]
enum Stream {
    Tcp(TokioIo<tokio::net::TcpStream>),
    Dialed(TokioIo<DialedStream>),
}

#[cfg(feature = "hyper")]
impl Connection for Stream {
    fn connected(&self) -> Connected {
        match self {
            // The peer and local addresses, as before.
            Stream::Tcp(stream) => stream.connected(),
            // Nothing to report: what the far end of a caller's stream is — a tunnel
            // peer, a proxy, a pipe — is the caller's knowledge, not this crate's.
            Stream::Dialed(_) => Connected::new(),
        }
    }
}

#[cfg(feature = "hyper")]
impl hyper::rt::Read for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Stream::Tcp(stream) => Pin::new(stream).poll_read(cx, buf),
            Stream::Dialed(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

#[cfg(feature = "hyper")]
impl hyper::rt::Write for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Stream::Tcp(stream) => Pin::new(stream).poll_write(cx, buf),
            Stream::Dialed(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Stream::Tcp(stream) => Pin::new(stream).poll_flush(cx),
            Stream::Dialed(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Stream::Tcp(stream) => Pin::new(stream).poll_shutdown(cx),
            Stream::Dialed(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }

    // Forwarded, not defaulted: the defaults would report "not vectored" and write one
    // buffer at a time, which would quietly change how the default path's TCP stream is
    // written to. Delegating keeps it exactly what it was.
    fn is_write_vectored(&self) -> bool {
        match self {
            Stream::Tcp(stream) => stream.is_write_vectored(),
            Stream::Dialed(stream) => stream.is_write_vectored(),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Stream::Tcp(stream) => Pin::new(stream).poll_write_vectored(cx, bufs),
            Stream::Dialed(stream) => Pin::new(stream).poll_write_vectored(cx, bufs),
        }
    }
}

/// Every link of an error's `source` chain, joined.
///
/// hyper-util's `Error` renders as `client error (Connect)` and no more: the reason a
/// caller or a log actually needs — "connection refused", a certificate that did not
/// verify, a connection closed before the body — is one or two `source`s below it. A
/// `detail` that said only the kind would be the one thing
/// [`TransportError`]'s two string fields exist to carry and could not. reqwest's own
/// `Display` already walks its chain, which is why [`HttpTransport`] passes
/// `error.to_string()` straight through and this is only on this side.
#[cfg(feature = "hyper")]
fn error_detail(error: &(dyn std::error::Error + 'static)) -> String {
    let mut detail = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        detail.push_str(": ");
        detail.push_str(&cause.to_string());
        source = cause.source();
    }
    detail
}

#[cfg(feature = "hyper")]
impl Transport for HyperTransport {
    async fn exchange(&self, cx: &Context, body: Vec<u8>) -> Result<Exchange, TransportError> {
        use http_body_util::BodyExt;

        let mut request =
            hyper::Request::new(http_body_util::Full::new(hyper::body::Bytes::from(body)));
        *request.method_mut() = hyper::Method::POST;
        *request.uri_mut() = self.uri.clone();

        let headers = request.headers_mut();
        // The deployment's first, then the protocol's over the top. reqwest arrives at
        // the same order from the other direction — its `default_headers` fill in only
        // the names a request did not set — so a `Content-Type` in a deployment's map is
        // overridden by this binding's in both transports rather than appended to in
        // one of them.
        headers.extend(self.headers.clone());
        headers.insert(
            nlipsy_core::http::header::CONTENT_TYPE,
            HeaderValue::from_static(nlipsy_http::CONTENT_TYPE),
        );
        // Only when the caller established one. The header is this binding's convention
        // rather than ECMA-431's, and sending an empty one would be a claim that there
        // is a turn to correlate with when there is not.
        if let Some(correlator) = &cx.correlator {
            let value = HeaderValue::from_str(correlator).map_err(|error| {
                // Nothing left the process, so this is `Unreachable` by the definition
                // in `nlipsy_core::TransportError`: the end-point cannot have seen the
                // request, and repeating it cannot duplicate a turn. (The reqwest
                // transport reports `Interrupted` for the same input, because reqwest
                // defers a bad header to `send` and `is_connect` is false for it. That
                // is an accident of its error model rather than a reading of the case,
                // and it is not copied here.)
                TransportError::Unreachable {
                    endpoint: self.endpoint.clone(),
                    detail: format!("the correlator is not a header value: {error}"),
                }
            })?;
            headers.insert(nlipsy_http::correlator_header(), value);
        }

        let response = self.http.request(request).await.map_err(|error| {
            // The same distinction `HttpTransport` draws and for the same reason: a
            // retry policy turns on whether the request can have been seen.
            // `is_connect` is hyper-util's own answer to that question. A `Dialer`'s
            // failure, and its connect budget running out, arrive here as `Connect`
            // too, because hyper-util wraps every error its connector returns that way
            // — which `hyper_dialer_tests` checks rather than trusts, since it is the
            // line between "safe to repeat" and "may already be in the transcript".
            let detail = error_detail(&error);
            if error.is_connect() {
                TransportError::Unreachable {
                    endpoint: self.endpoint.clone(),
                    detail,
                }
            } else {
                TransportError::Interrupted {
                    endpoint: self.endpoint.clone(),
                    detail,
                }
            }
        })?;

        let status = response.status().as_u16();
        // Both RFC 9110 §10.2.3 forms, through the one reader above.
        let retry_after = read_retry_after(response.headers());

        let body = response
            .into_body()
            .collect()
            .await
            .map_err(|error| TransportError::Interrupted {
                endpoint: self.endpoint.clone(),
                // Word for word the sentence `HttpTransport` uses, because it is the
                // same situation and a caller matching on the text — or a reader of two
                // logs — should not have to learn two of them.
                detail: format!(
                    "reply headers arrived but the body did not: {}",
                    error_detail(&error)
                ),
            })?
            .to_bytes();

        Ok(Exchange::new(
            body.to_vec(),
            ExchangeDetail::Http {
                status,
                retry_after,
            },
        ))
    }
}

/// One exchange against a real socket, for either transport.
///
/// A hand-written HTTP/1.1 reply rather than axum or hyper as a dev-dependency. What
/// these tests are about is the bytes: which headers this crate puts on the wire, and
/// what it makes of a reply that no NLIP implementation wrote. A server framework would
/// answer those questions through its own parser and its own defaults — a second
/// implementation standing between the test and the evidence — and it would be a far
/// heavier dependency than the code under test.
///
/// It is a module of its own because both transports' suites drive it, and that is the
/// point of the arrangement: the two are held to the same reply bytes rather than to two
/// fixtures that happen to agree today.
///
/// The responder itself — [`respond`](wire::respond), [`answer`](wire::answer),
/// [`read_request`](wire::read_request) — is generic over the stream, so the same bytes
/// are served from a loopback socket for the two mirrored suites and from the far half of
/// a `tokio::io::duplex` for the dialer's, where there is no socket at all.
#[cfg(all(test, any(feature = "hyper", feature = "reqwest")))]
mod wire {
    use nlipsy_core::{Context, Exchange, TransportError};
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
    use tokio::net::TcpListener;

    pub fn correlated(correlator: &str) -> Context {
        Context::new().with_correlator(correlator)
    }

    /// What one exchange against a real socket produced: the bytes the end-point read,
    /// and what the transport made of the answer.
    pub struct Served {
        pub request: String,
        pub result: Result<Exchange, TransportError>,
    }

    /// An end-point on loopback that answers one request with `response`, verbatim, and
    /// hands back the bytes it read. Returns the URL to send to.
    pub async fn serving(response: &'static [u8]) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a listener");
        let endpoint = format!(
            "http://{}{}",
            listener.local_addr().unwrap(),
            super::NLIP_PATH
        );

        let served = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("one connection");
            respond(&mut socket, response).await
        });

        (endpoint, served)
    }

    /// Answer one request on `stream` with `response`, verbatim, and hand back the bytes
    /// read.
    pub async fn respond<S>(stream: &mut S, response: &[u8]) -> String
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let request = read_request(stream).await;
        stream.write_all(response).await.expect("the reply is sent");
        stream.flush().await.expect("the reply is flushed");
        request
    }

    /// Answer requests on one `stream` in turn, one per entry of `responses`, and hand
    /// back what was read — the keep-alive variant, which is what lets a test see two
    /// exchanges travel over one connection. Stops early when the client hangs up
    /// instead of sending the next request, so a connection the client is done with
    /// ends the task rather than failing it.
    #[cfg(feature = "hyper")]
    pub async fn answer<S>(mut stream: S, responses: &[&[u8]]) -> Vec<String>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let mut requests = Vec::new();
        for response in responses {
            let request = read_request(&mut stream).await;
            if request.is_empty() {
                break;
            }
            stream.write_all(response).await.expect("the reply is sent");
            stream.flush().await.expect("the reply is flushed");
            requests.push(request);
        }
        requests
    }

    /// A URL on loopback that nothing is listening on: bound, read back, dropped.
    pub async fn nothing_listening() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a listener");
        let endpoint = format!(
            "http://{}{}",
            listener.local_addr().unwrap(),
            super::NLIP_PATH
        );
        drop(listener);
        endpoint
    }

    /// An end-point that announces 512 bytes of body and sends nine, then hangs up.
    pub async fn truncating() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a listener");
        let endpoint = format!(
            "http://{}{}",
            listener.local_addr().unwrap(),
            super::NLIP_PATH
        );
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("one connection");
            read_request(&mut socket).await;
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 512\r\n\r\n{\"format\"",
                )
                .await
                .expect("the head is sent");
            socket.flush().await.expect("the head is flushed");
        });
        endpoint
    }

    /// Read one HTTP/1.1 request: the head, and the body `Content-Length` announces.
    /// Empty when the peer hung up before sending anything.
    pub async fn read_request<S: AsyncRead + Unpin>(socket: &mut S) -> String {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let read = socket.read(&mut chunk).await.expect("the request arrives");
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..read]);
            let text = String::from_utf8_lossy(&buffer);
            if let Some(head) = text.find("\r\n\r\n") {
                let length = content_length(&text[..head]);
                if buffer.len() >= head + 4 + length {
                    break;
                }
            }
        }
        String::from_utf8_lossy(&buffer).into_owned()
    }

    fn content_length(head: &str) -> usize {
        head.lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().ok())
                    .flatten()
            })
            .unwrap_or(0)
    }

    /// A minimal conforming reply. `content-length` is written out and has to match the
    /// body below it exactly: both clients read that many bytes and no more, so a wrong
    /// number here fails as a truncated reply rather than as a broken fixture.
    pub const OK_MESSAGE: &[u8] = b"HTTP/1.1 200 OK\r\n\
        content-type: application/json\r\n\
        content-length: 54\r\n\
        \r\n\
        {\"format\":\"text\",\"subformat\":\"english\",\"content\":\"hi\"}";

    /// A `429` with `Retry-After` as delay-seconds.
    pub const THROTTLED_SECONDS: &[u8] = b"HTTP/1.1 429 Too Many Requests\r\n\
        content-type: text/plain\r\n\
        retry-after: 7\r\n\
        content-length: 10\r\n\
        \r\n\
        slow down!";

    /// A `503` with `Retry-After` as an HTTP-date. RFC 9110 §10.2.3 allows both, and an
    /// end-point behind a cache or a gateway often sends the date: a client that read
    /// only the count would turn what the end-point paced into a blind retry.
    pub const THROTTLED_DATE: &[u8] = b"HTTP/1.1 503 Service Unavailable\r\n\
        content-type: text/plain\r\n\
        retry-after: Wed, 21 Oct 2015 07:28:00 GMT\r\n\
        content-length: 4\r\n\
        \r\n\
        busy";

    /// The case `Body::Opaque` exists for, in the voice it really has: an intermediary
    /// in front of the end-point refusing the request in plain text. Nothing here came
    /// from an NLIP implementation, and the caller still has to be able to show it.
    pub const REFUSED: &[u8] = b"HTTP/1.1 413 Payload Too Large\r\n\
        content-type: text/plain; charset=utf-8\r\n\
        content-length: 21\r\n\
        \r\n\
        length limit exceeded";
}

/// What neither transport decides: the request `nlipsy-http` builds.
///
/// Ungated, because `request` is: it is `nlipsy-http`'s TR/113 §11.1 chat turn,
/// re-exported here, and a build with no transport at all still has it. `Context` and
/// `Client` are imported by name rather than reached through `super::*` for the same
/// reason — the module's own `use nlipsy_core::{…}` is behind the two transports.
#[cfg(test)]
mod tests {
    use super::*;
    use nlipsy_core::testing::{Scripted, ok};
    use nlipsy_core::{Client, Context};

    #[tokio::test]
    async fn the_request_carries_the_conversation_token() {
        let scripted = std::sync::Arc::new(Scripted::new(vec![ok(
            200,
            r#"{"format":"text","subformat":"english","content":"x"}"#,
        )]));
        let client = Client::new(std::sync::Arc::clone(&scripted));

        client
            .send(
                &request("hi", "english", "conv-42"),
                &Context::new().with_correlator("corr"),
            )
            .await
            .unwrap();

        let sent = scripted.sent_json();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["format"], "text");
        assert_eq!(sent[0]["subformat"], "english");
        assert_eq!(sent[0]["submessages"][0]["format"], "token");
        assert_eq!(sent[0]["submessages"][0]["subformat"], "conversation");
        assert_eq!(sent[0]["submessages"][0]["content"], "conv-42");
        // TR/113 §11.1: the canonical chat exchange carries no messagetype.
        assert!(sent[0].get("messagetype").is_none());
    }
}

#[cfg(all(test, feature = "reqwest"))]
mod reqwest_tests {
    use super::wire::*;
    use super::*;
    use nlipsy_core::{Body, Client};

    /// One exchange through a default transport against [`serving`].
    async fn exchange(response: &'static [u8], cx: &Context) -> Served {
        let (endpoint, served) = serving(response).await;

        let transport = HttpTransport::new(&endpoint);
        let result = transport
            .exchange(
                cx,
                br#"{"format":"text","subformat":"english","content":"hi"}"#.to_vec(),
            )
            .await;

        Served {
            request: served.await.expect("the listener finished"),
            result,
        }
    }

    #[tokio::test]
    async fn the_correlator_travels_as_a_header_when_the_context_carries_one() {
        let served = exchange(OK_MESSAGE, &correlated("corr-1")).await;

        let head = served.request.to_ascii_lowercase();
        assert!(
            head.contains(&format!("{CORRELATOR_HEADER}: corr-1")),
            "the correlator should be on the wire: {}",
            served.request
        );
        // And the body is JSON (ECMA-430 §5 ¶2) at the media type ECMA-433 §6.1.5
        // fixes for it, so the request says so — ECMA-431 §6 names neither.
        assert!(head.contains("content-type: application/json"), "{head}");
        assert!(served.result.is_ok());
    }

    #[tokio::test]
    async fn no_correlator_header_is_sent_when_the_context_has_none() {
        // Not an empty header: sending one would be a claim that there is a turn to
        // correlate with when the caller never named one.
        let served = exchange(OK_MESSAGE, &Context::new()).await;

        assert!(
            !served
                .request
                .to_ascii_lowercase()
                .contains(CORRELATOR_HEADER),
            "no correlator was set, so none should be on the wire: {}",
            served.request
        );
    }

    #[tokio::test]
    async fn retry_after_is_read_into_the_exchange_detail() {
        let exchange = exchange(THROTTLED_SECONDS, &Context::new())
            .await
            .result
            .expect("the end-point answered");

        assert_eq!(
            exchange.detail,
            ExchangeDetail::Http {
                status: 429,
                retry_after: Some(Duration::from_secs(7)),
            }
        );
    }

    #[tokio::test]
    async fn a_retry_after_given_as_a_date_is_read_as_a_delay_too() {
        let exchange = exchange(THROTTLED_DATE, &Context::new())
            .await
            .result
            .expect("the end-point answered");

        assert_eq!(
            exchange.detail,
            ExchangeDetail::Http {
                status: 503,
                // The date is long past, so the delay is zero: the end-point answered
                // the question and the answer is "now". `None` would mean it said
                // nothing a caller can use, which is a different thing to act on.
                retry_after: Some(Duration::ZERO),
            }
        );
    }

    #[tokio::test]
    async fn a_reply_that_is_not_a_message_reaches_the_caller_as_opaque() {
        let (endpoint, _served) = serving(REFUSED).await;

        // Through `Client`, because the decision that these bytes are not a message is
        // the core's and the point is that the two halves meet.
        let client = Client::new(HttpTransport::new(&endpoint));
        let reply = client
            .send(&request("hi", "english", "c1"), &Context::new())
            .await
            .expect("the exchange itself succeeded");

        assert_eq!(reply.status(), Some(413));
        assert!(reply.is_error());
        match &reply.body {
            Body::Opaque { text, why } => {
                assert_eq!(text, "length limit exceeded");
                assert!(!why.is_empty(), "the reason is what explains the raw text");
            }
            other => panic!("plain text is not a message: {other:?}"),
        }
        assert_eq!(reply.text().as_deref(), Some("length limit exceeded"));
    }

    #[tokio::test]
    async fn a_refused_connection_is_unreachable() {
        // Bound and dropped, so the port is one nothing is listening on. The
        // distinction matters beyond the wording: a request that never left cannot
        // have been recorded by the end-point, so repeating it cannot duplicate a turn.
        let endpoint = nothing_listening().await;

        let transport = HttpTransport::new(&endpoint);
        let error = transport
            .exchange(&Context::new(), b"{}".to_vec())
            .await
            .expect_err("nothing is listening");

        assert!(
            matches!(error, TransportError::Unreachable { .. }),
            "a refused connection is not an interrupted exchange: {error:?}"
        );
    }

    #[tokio::test]
    async fn a_reply_cut_off_mid_body_is_interrupted() {
        // The head arrives, the body does not, and the end-point may well have acted on
        // the request — which is exactly the case a caller must not retry blindly.
        let endpoint = truncating().await;

        let transport = HttpTransport::new(&endpoint);
        let error = transport
            .exchange(&Context::new(), b"{}".to_vec())
            .await
            .expect_err("the body never arrived");

        match error {
            TransportError::Interrupted { detail, .. } => {
                assert!(
                    detail.contains("reply headers arrived but the body did not"),
                    "the detail should say where it broke: {detail}"
                );
            }
            other => panic!("a truncated reply is not {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_header_the_builder_was_given_is_on_every_request() {
        let mut headers = HeaderMap::new();
        headers.insert("x-tenant", HeaderValue::from_static("acme"));

        let (endpoint, served) = serving(OK_MESSAGE).await;

        let transport = HttpTransport::builder(&endpoint)
            .headers(headers)
            .build()
            .expect("a client");
        transport
            .exchange(&Context::new(), b"{}".to_vec())
            .await
            .expect("the end-point answered");

        let sent = served.await.expect("the listener finished");
        assert!(
            sent.to_ascii_lowercase().contains("x-tenant: acme"),
            "the deployment's header should be on the wire: {sent}"
        );
    }
}

/// The same suite as `reqwest_tests`, test for test, against the same reply bytes.
///
/// Mirrored rather than shared through a generic helper on purpose. What is being
/// checked is that two *independent* implementations of one contract agree, and a
/// harness that drove both through one function would prove only that the function was
/// called twice — the assertions, not the plumbing, are where the agreement is.
#[cfg(all(test, feature = "hyper"))]
mod hyper_tests {
    use super::wire::*;
    use super::*;
    use nlipsy_core::{Body, Client};

    /// One exchange through a default transport against [`serving`].
    async fn exchange(response: &'static [u8], cx: &Context) -> Served {
        let (endpoint, served) = serving(response).await;

        let transport = HyperTransport::new(&endpoint);
        let result = transport
            .exchange(
                cx,
                br#"{"format":"text","subformat":"english","content":"hi"}"#.to_vec(),
            )
            .await;

        Served {
            request: served.await.expect("the listener finished"),
            result,
        }
    }

    #[tokio::test]
    async fn the_request_line_is_a_post_to_the_published_path() {
        // ECMA-431 §6 fixes two things — the URL and the method — and this is both of
        // them, read off the wire rather than off the builder.
        let served = exchange(OK_MESSAGE, &Context::new()).await;

        let first = served
            .request
            .lines()
            .next()
            .expect("a request line")
            .to_string();
        assert_eq!(first.trim_end(), format!("POST {NLIP_PATH} HTTP/1.1"));
        assert!(served.result.is_ok(), "{:?}", served.result);
    }

    #[tokio::test]
    async fn the_request_announces_its_length_rather_than_chunking_it() {
        // `Full<Bytes>` is what makes this true, and it is why the transport uses it:
        // an end-point reading one NLIP message under a body limit should be told how
        // much is coming, not handed a chunked stream to discover it from.
        let served = exchange(OK_MESSAGE, &Context::new()).await;

        let head = served.request.to_ascii_lowercase();
        assert!(head.contains("content-length: 54"), "{head}");
        assert!(
            !head.contains("transfer-encoding"),
            "a body of known length needs none: {head}"
        );
    }

    #[tokio::test]
    async fn the_correlator_travels_as_a_header_when_the_context_carries_one() {
        let served = exchange(OK_MESSAGE, &correlated("corr-1")).await;

        let head = served.request.to_ascii_lowercase();
        assert!(
            head.contains(&format!("{CORRELATOR_HEADER}: corr-1")),
            "the correlator should be on the wire: {}",
            served.request
        );
        // And the body is JSON (ECMA-430 §5 ¶2) at the media type ECMA-433 §6.1.5
        // fixes for it, so the request says so — ECMA-431 §6 names neither.
        assert!(head.contains("content-type: application/json"), "{head}");
        assert!(served.result.is_ok());
    }

    #[tokio::test]
    async fn no_correlator_header_is_sent_when_the_context_has_none() {
        // Not an empty header: sending one would be a claim that there is a turn to
        // correlate with when the caller never named one.
        let served = exchange(OK_MESSAGE, &Context::new()).await;

        assert!(
            !served
                .request
                .to_ascii_lowercase()
                .contains(CORRELATOR_HEADER),
            "no correlator was set, so none should be on the wire: {}",
            served.request
        );
    }

    #[tokio::test]
    async fn retry_after_is_read_into_the_exchange_detail() {
        let exchange = exchange(THROTTLED_SECONDS, &Context::new())
            .await
            .result
            .expect("the end-point answered");

        assert_eq!(
            exchange.detail,
            ExchangeDetail::Http {
                status: 429,
                retry_after: Some(Duration::from_secs(7)),
            }
        );
    }

    #[tokio::test]
    async fn a_retry_after_given_as_a_date_is_read_as_a_delay_too() {
        let exchange = exchange(THROTTLED_DATE, &Context::new())
            .await
            .result
            .expect("the end-point answered");

        assert_eq!(
            exchange.detail,
            ExchangeDetail::Http {
                status: 503,
                // Long past, so the delay is zero rather than `None`: the end-point
                // answered the question and the answer is "now".
                retry_after: Some(Duration::ZERO),
            }
        );
    }

    #[tokio::test]
    async fn a_reply_that_is_not_a_message_reaches_the_caller_as_opaque() {
        let (endpoint, _served) = serving(REFUSED).await;

        // Through `Client`, because the decision that these bytes are not a message is
        // the core's and the point is that the two halves meet.
        let client = Client::new(HyperTransport::new(&endpoint));
        let reply = client
            .send(&request("hi", "english", "c1"), &Context::new())
            .await
            .expect("the exchange itself succeeded");

        assert_eq!(reply.status(), Some(413));
        assert!(reply.is_error());
        match &reply.body {
            Body::Opaque { text, why } => {
                assert_eq!(text, "length limit exceeded");
                assert!(!why.is_empty(), "the reason is what explains the raw text");
            }
            other => panic!("plain text is not a message: {other:?}"),
        }
        assert_eq!(reply.text().as_deref(), Some("length limit exceeded"));
    }

    #[tokio::test]
    async fn the_client_drives_a_whole_turn_through_this_transport() {
        // The end-to-end join: `nlipsy_core::Client::send` encodes the TR/113 §11.1
        // request, this transport puts it on a socket, and the reply decodes into a
        // message with text in it. Nothing between the two halves is mocked.
        let (endpoint, served) = serving(OK_MESSAGE).await;

        let client = Client::new(HyperTransport::new(&endpoint));
        let reply = client
            .send(&request("hi", "english", "conv-1"), &correlated("corr-e2e"))
            .await
            .expect("the end-point answered");

        assert_eq!(reply.status(), Some(200));
        assert!(!reply.is_error());
        assert_eq!(reply.text().as_deref(), Some("hi"));
        assert!(reply.message().is_some(), "the reply decodes as a message");

        let sent = served.await.expect("the listener finished");
        assert!(
            sent.contains("\"conv-1\""),
            "the conversation token should be in the body the socket read: {sent}"
        );
        assert!(
            sent.to_ascii_lowercase()
                .contains(&format!("{CORRELATOR_HEADER}: corr-e2e")),
            "{sent}"
        );
    }

    #[tokio::test]
    async fn a_refused_connection_is_unreachable() {
        // Bound and dropped, so the port is one nothing is listening on. The
        // distinction matters beyond the wording: a request that never left cannot
        // have been recorded by the end-point, so repeating it cannot duplicate a turn.
        let endpoint = nothing_listening().await;

        let transport = HyperTransport::new(&endpoint);
        let error = transport
            .exchange(&Context::new(), b"{}".to_vec())
            .await
            .expect_err("nothing is listening");

        match error {
            TransportError::Unreachable { detail, .. } => {
                // And the chain is in the detail rather than hyper-util's bare
                // `client error (Connect)`, which is what `error_detail` is for.
                assert!(
                    detail.len() > "client error (Connect)".len(),
                    "the detail should name the cause: {detail}"
                );
            }
            other => panic!("a refused connection is not {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_reply_cut_off_mid_body_is_interrupted() {
        // The head arrives, the body does not, and the end-point may well have acted on
        // the request — which is exactly the case a caller must not retry blindly.
        let endpoint = truncating().await;

        let transport = HyperTransport::new(&endpoint);
        let error = transport
            .exchange(&Context::new(), b"{}".to_vec())
            .await
            .expect_err("the body never arrived");

        match error {
            TransportError::Interrupted { detail, .. } => {
                assert!(
                    detail.contains("reply headers arrived but the body did not"),
                    "the detail should say where it broke, in the same words the \
                     reqwest transport uses: {detail}"
                );
            }
            other => panic!("a truncated reply is not {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_header_the_builder_was_given_is_on_every_request() {
        let mut headers = HeaderMap::new();
        headers.insert("x-tenant", HeaderValue::from_static("acme"));

        let (endpoint, served) = serving(OK_MESSAGE).await;

        let transport = HyperTransport::builder(&endpoint)
            .headers(headers)
            .build()
            .expect("a client");
        transport
            .exchange(&Context::new(), b"{}".to_vec())
            .await
            .expect("the end-point answered");

        let sent = served.await.expect("the listener finished");
        assert!(
            sent.to_ascii_lowercase().contains("x-tenant: acme"),
            "the deployment's header should be on the wire: {sent}"
        );
    }

    #[tokio::test]
    async fn a_plain_http_endpoint_is_accepted() {
        // `https_or_http`, and the reason is in the comment where the connector is
        // built: ECMA-430 §7.1 obliges encryption of a *deployment*, and a transport
        // that refused `http` would be deciding where a deployment may terminate TLS
        // rather than enforcing the clause. Every test above happens to prove this; one
        // of them says so, because the day someone "hardens" the connector to
        // `https_only` this is the test that explains what broke.
        let (endpoint, served) = serving(OK_MESSAGE).await;
        assert!(endpoint.starts_with("http://"), "{endpoint}");

        let exchange = HyperTransport::new(&endpoint)
            .exchange(&Context::new(), b"{}".to_vec())
            .await
            .expect("a plain http end-point is reachable");

        assert_eq!(exchange.detail.status(), Some(200));
        served.await.expect("the listener finished");
    }

    #[test]
    fn an_endpoint_that_is_not_a_uri_is_a_build_error_rather_than_a_failed_exchange() {
        // The one behavioural difference from the reqwest transport, and it is the
        // better half of it: hyper needs a `Uri`, so the string is parsed once at build
        // time and a caller is told where they wrote it.
        let error = HyperTransport::builder("not a url")
            .build()
            .expect_err("that is not a URI");

        assert!(error.to_string().contains("not a url"), "{error}");
    }
}

/// The [`Dialer`] seam and the extra trust roots, against the same hand-written responder
/// — on the far half of a `tokio::io::duplex`, so that no test here listens on a port or
/// opens a socket.
///
/// A module of its own rather than more of `hyper_tests`, because that suite is the
/// reqwest suite's mirror, test for test, and these have no reqwest counterpart to
/// mirror: the reqwest transport has neither a dialer nor extra roots.
///
/// The TLS tests run against a test PKI checked in under `tests/fixtures/tls/` — a CA, a
/// leaf for `nlip.test` and the leaf's key, regenerated by the OpenSSL recipe in that
/// directory's README — rather than one minted per run; `FIXTURE_CA` says why.
///
/// Every end-point here is under `.test`, which RFC 6761 §6.2 reserves so that it never
/// resolves. An exchange with `nlip.test` that succeeds is therefore proof on its own that
/// the name went to the dialer and not to a resolver — there is nothing for a resolver to
/// find and nothing listening for a socket to reach.
#[cfg(all(test, feature = "hyper"))]
mod hyper_dialer_tests {
    use super::wire::*;
    use super::*;
    use nlipsy_core::Client;
    use std::sync::Mutex;
    use std::time::Instant;
    use tokio::io::DuplexStream;
    use tokio::task::JoinHandle;

    /// A dialer with no network in it: each `dial` is a fresh `tokio::io::duplex` pair,
    /// the far half answered in-process by [`answer`] — after a TLS handshake when there
    /// is a server configuration — and the near half handed to the transport.
    #[derive(Debug)]
    struct InProcess {
        /// What the far end answers on each connection, one entry per request, in order.
        responses: Vec<&'static [u8]>,
        /// For an `https` end-point: the server side of the handshake.
        tls: Option<Arc<rustls::ServerConfig>>,
        /// Every target a dial was asked for, in order.
        dialed: Mutex<Vec<DialTarget>>,
        /// The SNI each TLS handshake the far end completed was sent.
        sni: Arc<Mutex<Vec<Option<String>>>>,
        /// The far end of every connection, finishing with the requests it read.
        served: Mutex<Vec<JoinHandle<Vec<String>>>>,
    }

    impl InProcess {
        fn plain(responses: Vec<&'static [u8]>) -> Arc<InProcess> {
            InProcess::new(responses, None)
        }

        fn tls(responses: Vec<&'static [u8]>, config: Arc<rustls::ServerConfig>) -> Arc<InProcess> {
            InProcess::new(responses, Some(config))
        }

        fn new(
            responses: Vec<&'static [u8]>,
            tls: Option<Arc<rustls::ServerConfig>>,
        ) -> Arc<InProcess> {
            Arc::new(InProcess {
                responses,
                tls,
                dialed: Mutex::default(),
                sni: Arc::default(),
                served: Mutex::default(),
            })
        }

        fn dialed(&self) -> Vec<DialTarget> {
            self.dialed.lock().unwrap().clone()
        }

        fn sni(&self) -> Vec<Option<String>> {
            self.sni.lock().unwrap().clone()
        }

        /// Every request every connection read, once the far ends have finished.
        async fn requests(&self) -> Vec<String> {
            let served = std::mem::take(&mut *self.served.lock().unwrap());
            let mut requests = Vec::new();
            for connection in served {
                requests.extend(connection.await.expect("the far end finished"));
            }
            requests
        }
    }

    impl Dialer for InProcess {
        fn dial(&self, target: DialTarget) -> DialFuture {
            self.dialed.lock().unwrap().push(target);

            let (near, far): (DuplexStream, DuplexStream) = tokio::io::duplex(64 * 1024);
            let responses = self.responses.clone();
            let tls = self.tls.clone();
            let sni = Arc::clone(&self.sni);
            let served = tokio::spawn(async move {
                match tls {
                    None => answer(far, &responses).await,
                    Some(config) => match tokio_rustls::TlsAcceptor::from(config).accept(far).await
                    {
                        Ok(stream) => {
                            let name = stream.get_ref().1.server_name().map(str::to_owned);
                            sni.lock().unwrap().push(name);
                            answer(stream, &responses).await
                        }
                        // The client refused the certificate, which is the point of the
                        // tests that expect it to.
                        Err(_) => Vec::new(),
                    },
                }
            });
            self.served.lock().unwrap().push(served);

            Box::pin(async move { Ok(Box::new(near) as DialedStream) })
        }
    }

    /// A dialer that fails, the way a tunnel that is not up fails.
    #[derive(Debug)]
    struct Failing(fn() -> io::Error);

    impl Dialer for Failing {
        fn dial(&self, _: DialTarget) -> DialFuture {
            let error = (self.0)();
            Box::pin(async move { Err(error) })
        }
    }

    /// A dialer that never answers: a tunnel whose handshake never completes.
    #[derive(Debug)]
    struct Silent;

    impl Dialer for Silent {
        fn dial(&self, _: DialTarget) -> DialFuture {
            Box::pin(std::future::pending())
        }
    }

    fn target(host: &str, port: u16) -> DialTarget {
        DialTarget {
            host: host.to_owned(),
            port,
        }
    }

    fn unreachable_detail(error: TransportError) -> String {
        match error {
            TransportError::Unreachable { detail, .. } => detail,
            other => panic!("a connection that never opened is Unreachable, not {other:?}"),
        }
    }

    /// The test PKI, checked in under `tests/fixtures/tls/`: a CA (`CA:TRUE`), a leaf for
    /// DNS `nlip.test` alone that the CA signed (SAN only, EKU `serverAuth`), and the
    /// leaf's PKCS#8 key — all ECDSA P-256, which rustls's `ring` provider both signs and
    /// verifies with. Generated once by the OpenSSL recipe in that directory's README and
    /// valid until 2126, so nothing expires in practice; the key protects nothing.
    ///
    /// Checked in rather than minted per run, which was tried with rcgen and given up:
    /// rcgen brings `time`, whose current releases need rustc 1.88, and `--all-targets`
    /// compiles dev-dependencies — so the MSRV job would have hung on an exact pin, a
    /// lockfile hold and a weekly Dependabot bump to refuse, all for a crate that only
    /// writes two validity dates. Three files of bytes have no rust-version.
    const FIXTURE_CA: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/tls/ca.der"
    ));
    const FIXTURE_LEAF: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/tls/leaf.der"
    ));
    const FIXTURE_LEAF_KEY: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/tls/leaf.key.der"
    ));

    /// A server configuration presenting the fixture leaf for `nlip.test`.
    fn fixture_server() -> Arc<rustls::ServerConfig> {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

        // `builder_with_provider(ring)` on the server side too, for the client's reason:
        // a test build with both features has a rustls carrying two providers.
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let server = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("ring's protocol versions")
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(FIXTURE_LEAF)],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(FIXTURE_LEAF_KEY)),
            )
            .expect("the fixture leaf and its key");
        Arc::new(server)
    }

    const HELLO: &[u8] = br#"{"format":"text","subformat":"english","content":"hi"}"#;

    #[tokio::test]
    async fn a_dialer_carries_the_whole_exchange_and_the_transport_opens_no_socket() {
        let dialer = InProcess::plain(vec![OK_MESSAGE]);
        let transport = HyperTransport::builder("http://nlip.test/nlip")
            .dialer(dialer.clone())
            .build()
            .expect("a transport");

        // Through `Client`, so the whole turn — encode, dial, send, read, decode — runs
        // over the dialed stream with nothing between the halves mocked.
        let reply = Client::new(transport)
            .send(
                &request("hi", "english", "conv-1"),
                &correlated("corr-dial"),
            )
            .await
            .expect("the end-point answered through the dialer");
        assert_eq!(reply.status(), Some(200));
        assert_eq!(reply.text().as_deref(), Some("hi"));

        // Asked for the URI's host as written — unresolved — and the scheme's port.
        assert_eq!(dialer.dialed(), vec![target("nlip.test", 80)]);

        let requests = dialer.requests().await;
        assert_eq!(requests.len(), 1, "{requests:?}");
        let head = requests[0].to_ascii_lowercase();
        assert!(
            head.starts_with(&format!("post {NLIP_PATH} http/1.1")),
            "{head}"
        );
        assert!(head.contains("host: nlip.test"), "{head}");
        assert!(
            head.contains(&format!("{CORRELATOR_HEADER}: corr-dial")),
            "{head}"
        );
        assert!(requests[0].contains("\"conv-1\""), "{}", requests[0]);
    }

    #[tokio::test]
    async fn an_ipv6_literal_reaches_the_dialer_without_its_brackets() {
        // End to end rather than only through `dial_target`: hyper-util hands the
        // connector a URI rebuilt from its pool key, and this is what proves the rebuilt
        // one still yields the host a socket-address parser accepts.
        let dialer = InProcess::plain(vec![OK_MESSAGE]);
        let transport = HyperTransport::builder("http://[::1]:8443/nlip")
            .dialer(dialer.clone())
            .build()
            .expect("a transport");

        transport
            .exchange(&Context::new(), HELLO.to_vec())
            .await
            .expect("the end-point answered");

        assert_eq!(dialer.dialed(), vec![target("::1", 8443)]);
    }

    #[tokio::test]
    async fn a_failed_dial_is_unreachable_and_carries_the_dialers_reason() {
        let transport = HyperTransport::builder("http://nlip.test/nlip")
            .dialer(Arc::new(Failing(|| io::Error::other("tunnel down"))))
            .build()
            .expect("a transport");

        let error = transport
            .exchange(&Context::new(), HELLO.to_vec())
            .await
            .expect_err("the tunnel is down");

        // `Unreachable`: hyper-util classified the dialer's error as `Connect`, which is
        // the claim the error mapping in `exchange` rests on.
        let detail = unreachable_detail(error);
        assert!(detail.contains("tunnel down"), "{detail}");
    }

    #[tokio::test]
    async fn a_dial_errors_whole_source_chain_reaches_the_detail() {
        // The reason a consumer puts *under* its own error — the cause it wraps — is as
        // much the user's to read as the top line.
        #[derive(Debug)]
        struct TunnelDown(io::Error);
        impl std::fmt::Display for TunnelDown {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("the tunnel is not up")
            }
        }
        impl std::error::Error for TunnelDown {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }

        let transport = HyperTransport::builder("http://nlip.test/nlip")
            .dialer(Arc::new(Failing(|| {
                io::Error::other(TunnelDown(io::Error::other("no handshake for 35 s")))
            })))
            .build()
            .expect("a transport");

        let detail = unreachable_detail(
            transport
                .exchange(&Context::new(), HELLO.to_vec())
                .await
                .expect_err("the tunnel is down"),
        );
        assert!(detail.contains("the tunnel is not up"), "{detail}");
        assert!(detail.contains("no handshake for 35 s"), "{detail}");
    }

    #[tokio::test]
    async fn a_dial_that_never_finishes_is_cut_off_by_the_connect_budget() {
        let transport = HyperTransport::builder("http://nlip.test/nlip")
            .dialer(Arc::new(Silent))
            .connect_timeout(Duration::from_millis(100))
            .build()
            .expect("a transport");

        let started = Instant::now();
        // An outer bound, so that a budget that was never applied fails this test
        // instead of hanging it.
        let error = tokio::time::timeout(
            Duration::from_secs(5),
            transport.exchange(&Context::new(), HELLO.to_vec()),
        )
        .await
        .expect("the connect budget, not the test's own bound, ended the exchange")
        .expect_err("the dialer never answered");
        let elapsed = started.elapsed();

        assert!(
            elapsed >= Duration::from_millis(100),
            "gave up early: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "gave up late: {elapsed:?}"
        );
        let detail = unreachable_detail(error);
        assert!(detail.contains("connect timed out after 100ms"), "{detail}");
    }

    #[tokio::test]
    async fn two_exchanges_in_turn_share_one_dial() {
        let dialer = InProcess::plain(vec![OK_MESSAGE, OK_MESSAGE]);
        let transport = HyperTransport::builder("http://nlip.test/nlip")
            .dialer(dialer.clone())
            .build()
            .expect("a transport");

        transport
            .exchange(&Context::new(), HELLO.to_vec())
            .await
            .expect("the first exchange");
        // hyper-util returns an HTTP/1.1 connection to the pool from a background task
        // once the reply has been read to the end, so that task gets its turn before
        // the second request looks in the pool. Without the pause the second request
        // could find the pool still empty and dial — correctly, by hyper-util's rules,
        // and the very race `Dialer`'s documentation warns of. What this test is about
        // is that, with an idle connection there to take, it does not dial again.
        tokio::time::sleep(Duration::from_millis(50)).await;
        transport
            .exchange(&Context::new(), HELLO.to_vec())
            .await
            .expect("the second exchange");

        assert_eq!(dialer.dialed().len(), 1, "{:?}", dialer.dialed());
        // And both requests really did travel over that one stream.
        assert_eq!(dialer.requests().await.len(), 2);
    }

    #[tokio::test]
    async fn https_through_a_dialer_verifies_against_an_added_root_by_the_uri_host() {
        let dialer = InProcess::tls(vec![OK_MESSAGE], fixture_server());
        let transport = HyperTransport::builder("https://nlip.test/nlip")
            .dialer(dialer.clone())
            .add_root_certificate(FIXTURE_CA.to_vec())
            .build()
            .expect("a transport");

        let exchange = transport
            .exchange(&Context::new(), HELLO.to_vec())
            .await
            .expect("the certificate verifies against the added root");

        assert_eq!(exchange.detail.status(), Some(200));
        assert_eq!(dialer.dialed(), vec![target("nlip.test", 443)]);
        // The name TLS sent is the URI's: the dialer was never asked for one, and it
        // handed back a pipe with no name at all.
        assert_eq!(dialer.sni(), vec![Some("nlip.test".to_owned())]);
    }

    #[tokio::test]
    async fn the_same_https_exchange_without_the_added_root_fails_verification() {
        let dialer = InProcess::tls(vec![OK_MESSAGE], fixture_server());
        let transport = HyperTransport::builder("https://nlip.test/nlip")
            .dialer(dialer.clone())
            .build()
            .expect("a transport");

        let detail = unreachable_detail(
            transport
                .exchange(&Context::new(), HELLO.to_vec())
                .await
                .expect_err("no root this transport trusts signed the certificate"),
        );
        // Mozilla's bundle alone does not know the test CA: the roots are *added to*,
        // and without the addition the handshake is what fails — `Unreachable`, because
        // no request was sent over a connection that never finished opening.
        assert!(detail.contains("invalid peer certificate"), "{detail}");
        assert!(detail.contains("UnknownIssuer"), "{detail}");
    }

    #[tokio::test]
    async fn a_certificate_for_another_name_is_refused_although_the_dialer_reached_its_server() {
        // The dialer routes every name to the same server, as a tunnel may. The
        // certificate is good and its issuer trusted — but it is for `nlip.test`, and the
        // URI asked for `other.test`, so the route the dialer chose does not get to stand
        // in for the name.
        let dialer = InProcess::tls(vec![OK_MESSAGE], fixture_server());
        let transport = HyperTransport::builder("https://other.test/nlip")
            .dialer(dialer.clone())
            .add_root_certificate(FIXTURE_CA.to_vec())
            .build()
            .expect("a transport");

        let detail = unreachable_detail(
            transport
                .exchange(&Context::new(), HELLO.to_vec())
                .await
                .expect_err("the certificate is not for other.test"),
        );
        assert!(detail.contains("invalid peer certificate"), "{detail}");
        assert!(
            detail.contains("not valid for name \"other.test\""),
            "the name checked should be the URI's: {detail}"
        );
    }

    #[test]
    fn an_added_root_that_is_not_a_certificate_is_a_build_error() {
        let error = HyperTransport::builder("https://nlip.test/nlip")
            .add_root_certificate(vec![1, 2, 3])
            .build()
            .expect_err("three bytes are not a certificate");

        assert!(
            error.to_string().contains("root certificate #1"),
            "the error should say which of the caller's roots it was: {error}"
        );
    }

    #[test]
    fn the_dial_target_follows_http_connectors_rule() {
        let of = |uri: &str| dial_target(&uri.parse().expect("a URI")).expect("a target");

        // An IPv6 literal loses its brackets and keeps its explicit port.
        assert_eq!(of("https://[::1]:8443/nlip"), target("::1", 8443));
        // The scheme's default when there is no port: 443 for `https`, 80 otherwise.
        assert_eq!(of("https://nlip.test/nlip"), target("nlip.test", 443));
        assert_eq!(of("http://nlip.test/nlip"), target("nlip.test", 80));
        // A name is passed on as written, never resolved; an IPv4 literal likewise.
        assert_eq!(of("http://10.0.0.7:8080/nlip"), target("10.0.0.7", 8080));
        // And a URI with no scheme is refused, as `HttpConnector` refuses it.
        assert!(dial_target(&"/nlip".parse().expect("a URI")).is_err());
    }

    #[test]
    fn debug_says_whether_a_dialer_opens_the_connections() {
        let dialed = HyperTransport::builder("http://nlip.test/nlip")
            .dialer(Arc::new(Silent))
            .build()
            .expect("a transport");
        let rendered = format!("{dialed:?}");
        assert!(rendered.contains("dialer: Some(Silent)"), "{rendered}");

        let rendered = format!("{:?}", HyperTransport::new("http://nlip.test/nlip"));
        assert!(rendered.contains("dialer: None"), "{rendered}");
    }
}
