//! The server itself: its configuration, its routes, and the pipeline between them.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::post;
use nlipsy_core::{
    Conformity, ConformityConfig, Context, Disposition, DynMiddleware, Endpoint, Findings,
    Middleware, Outcome, Rejection, Served, TokenAuthority,
};
use nlipsy_protocol::content::Wire;
use tracing::{Instrument, error, info, warn};
use uuid::Uuid;

use crate::reply::{json_response, reject};
use crate::status::StatusMap;
use crate::upload::{Upload, UploadSink};

// ECMA-431 §6 and the conventions around it — the path, the correlator header, the
// media type, the body limit — are `nlipsy-http`'s, and re-exported here so that a
// consumer who named this crate keeps the paths it already wrote.
//
// They used to be declared here *and* in `nlipsy-http-client`, with the duplication
// argued for: "the two are separate implementations of the same clause, and a constant
// both sides must agree on fails loudly when it drifts rather than silently". That
// argument was honest while the two lived in different repositories, where agreement
// between them was evidence. In one workspace it became a second copy to keep in step,
// so there is one copy now and the corpus is the witness — see `nlipsy-http`'s README,
// "Why a crate and not a module".
pub use nlipsy_http::{CORRELATOR_HEADER, DEFAULT_MAX_REQUEST_BODY_SIZE, NLIP_PATH};

/// Longest correlator this binding will reuse from a peer.
///
/// Above it the peer's value is discarded and a fresh one minted: a correlator is
/// copied into a log line and a span field on every request, and a peer that sends a
/// four-kilobyte header should not be able to decide how large those get.
///
/// The substitution is visible to the peer rather than silent: the reply's
/// [`CORRELATOR_HEADER`] carries the fresh value, so a client that sent an over-long
/// correlator does not get its own identifier back and sees the correlation fail
/// rather than quietly match the wrong exchange.
///
/// The rule is this binding's and not the concept's. ECMA-431 §6 names no correlator
/// at all — the header is a convention this crate spells, and a bound on it is a
/// deployment's answer to a denial-of-service question — while ECMA-433 §6.1.4 makes
/// the AMQP equivalent normative, saying a server agent "shall copy the contents of
/// the correlation-id field" and naming no length. An AMQP binding must therefore not
/// copy this guard.
pub const MAX_CORRELATOR_LENGTH: usize = 128;

/// Longest principal this binding will carry from a trusted header.
///
/// Above it the value is not carried at all — the exchange arrives with no principal —
/// for the reason [`MAX_CORRELATOR_LENGTH`] gives: a value a peer can influence should
/// not be able to decide how large the things built from it get. An identity a proxy
/// writes — a subject claim, an e-mail address, a SPIFFE ID — fits with room to spare.
pub const MAX_PRINCIPAL_LENGTH: usize = 512;

// ---------------------------------------------------------------------------
// The principal
// ---------------------------------------------------------------------------

/// Who a trusted proxy in front of this binding says the caller is.
///
/// ECMA-434 §7.2.1 asks a profile-2 deployment to authenticate its clients, and TR/113
/// §9 allows an implementation to "rely on just the base transfer protocol to perform
/// the authentication". In an HTTP deployment that is usually a gateway — OIDC or a JWT
/// for a person, a client certificate for a machine — which then tells the end-point
/// whom it let through in a header. This is that header's value, carried into
/// [`Context::extensions`], which is where the core says "an authenticated principal"
/// travels without the core having to name one.
///
/// **What it is worth is entirely the deployment's to establish.** The binding copies
/// the header [`ServerConfig::principal_header`] names and nothing else; it verifies
/// nothing. The value means something only if every request reaches this router
/// through the proxy that writes the header — and strips any copy the caller sent —
/// which is a network property (a policy admitting only the gateway) and not one any
/// code here can check. A deployment that names no header gets no principal on any
/// exchange, whatever the caller sends.
///
/// Opaque, on purpose: what the value says — a class of caller, an identifier, both —
/// is between the proxy and the deployment, and a type that parsed it would settle a
/// format the standard leaves to them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal(String);

impl Principal {
    /// A principal with this value — for a deployment that establishes one by other
    /// means, and for tests.
    pub fn new(value: impl Into<String>) -> Principal {
        Principal(value.into())
    }

    /// The value, as the proxy wrote it, trimmed.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// What this binding does, as values.
///
/// Everything here is *protocol-adjacent*: the body limit, the correlator's spelling,
/// the clause-6 switches and the status codes are what an ECMA-431 end-point has to
/// decide and what a peer can observe. Everything that is not — rate limiting, TLS,
/// timeouts' values, tracing subscribers, session storage — is the deployment's, and
/// reaches the exchange as a [`Middleware`] or as a `tower` layer around the router.
///
/// `#[non_exhaustive]`: §6 has more optional corners than these five. Build one with
/// [`ServerConfig::new`] and the `with_*` shorthands.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ServerConfig {
    /// Largest request body read, in bytes. Default
    /// [`DEFAULT_MAX_REQUEST_BODY_SIZE`].
    ///
    /// Enforced while the body is read, inside this crate, so that exceeding it is
    /// answered with an NLIP error message at [`StatusMap::too_large`] rather than
    /// with a framework's plain text. A `tower-http` `RequestBodyLimitLayer` outside
    /// the router would refuse in its own voice, which is the one rejection that
    /// would break this crate's stated principle — jsonrpsee moves the same check
    /// inside for the same reason (`server/src/transport/http.rs`).
    pub max_request_body_size: usize,
    /// Header the correlator travels in, or `None` to carry none.
    ///
    /// The peer's value is reused when it is sane — non-empty and at most
    /// [`MAX_CORRELATOR_LENGTH`] — so that a chain of endpoints can be followed end to
    /// end; otherwise one is minted here. With `None` no header is read and none is
    /// echoed, but [`Context::correlator`] is still filled: an exchange that cannot be
    /// named in a log line is an exchange nobody can investigate, and a deployment
    /// keying per-conversation state on it (as one does when there is no session)
    /// would otherwise share one key between callers.
    pub correlator_header: Option<HeaderName>,
    /// The optional halves of clause 6: which tokens this endpoint issues, and how
    /// many it will echo.
    pub conformity: ConformityConfig,
    /// What a clause-5 finding does, inbound and outbound.
    pub rejection: Rejection,
    /// What each refusal looks like on the status line.
    pub status: StatusMap,
    /// Header a trusted proxy writes the caller's identity in, or `None` — the default
    /// — to carry no principal at all. See [`Principal`] for what trusting it takes.
    pub principal_header: Option<HeaderName>,
}

impl Default for ServerConfig {
    fn default() -> ServerConfig {
        ServerConfig {
            max_request_body_size: DEFAULT_MAX_REQUEST_BODY_SIZE,
            correlator_header: Some(HeaderName::from_static(CORRELATOR_HEADER)),
            conformity: ConformityConfig::default(),
            rejection: Rejection::default(),
            status: StatusMap::default(),
            principal_header: None,
        }
    }
}

impl ServerConfig {
    /// The defaults, which reproduce the reference server.
    pub fn new() -> ServerConfig {
        ServerConfig::default()
    }

    /// Change the body limit.
    pub fn with_max_request_body_size(mut self, bytes: usize) -> ServerConfig {
        self.max_request_body_size = bytes;
        self
    }

    /// Change — or remove, with `None` — the correlator header.
    pub fn with_correlator_header(mut self, header: Option<HeaderName>) -> ServerConfig {
        self.correlator_header = header;
        self
    }

    /// Change the clause-6 switches.
    pub fn with_conformity(mut self, conformity: ConformityConfig) -> ServerConfig {
        self.conformity = conformity;
        self
    }

    /// Change what a clause-5 finding does.
    pub fn with_rejection(mut self, rejection: Rejection) -> ServerConfig {
        self.rejection = rejection;
        self
    }

    /// Change the status codes.
    pub fn with_status(mut self, status: StatusMap) -> ServerConfig {
        self.status = status;
        self
    }

    /// Name — or remove, with `None` — the header a trusted proxy writes the caller's
    /// identity in.
    pub fn with_principal_header(mut self, header: Option<HeaderName>) -> ServerConfig {
        self.principal_header = header;
        self
    }

    /// The principal of an exchange arriving with these headers, when this
    /// configuration names a header for one.
    ///
    /// Carried only when the header appears exactly once, is visible ASCII, and holds
    /// between one and [`MAX_PRINCIPAL_LENGTH`] bytes once trimmed. Anything else is no
    /// principal — never a guess at one — and is logged, without the value: a proxy that
    /// strips the caller's copy and writes its own sends exactly one, so two are a
    /// copy that got past it, and the exchange is safer treated as unauthenticated than
    /// attributed to either.
    pub fn principal_for(&self, headers: &HeaderMap) -> Option<Principal> {
        let name = self.principal_header.as_ref()?;
        let mut values = headers.get_all(name).iter();
        let value = values.next()?;
        if values.next().is_some() {
            warn!(header = %name, "the principal header arrived more than once; no principal is carried");
            return None;
        }
        let Ok(text) = value.to_str() else {
            warn!(header = %name, "the principal header is not visible ASCII; no principal is carried");
            return None;
        };
        let text = text.trim();
        if text.is_empty() || text.len() > MAX_PRINCIPAL_LENGTH {
            warn!(
                header = %name,
                length = text.len(),
                "the principal header is empty or too long; no principal is carried"
            );
            return None;
        }
        Some(Principal(text.to_string()))
    }

    /// The correlator for an exchange arriving with these headers.
    ///
    /// Public because a deployment's own layer — one that refuses before this crate
    /// sees the request — wants the same value in its log line and on its reply, and
    /// the rule for deriving it is this binding's.
    pub fn correlator_for(&self, headers: &HeaderMap) -> String {
        self.correlator_header
            .as_ref()
            .and_then(|name| headers.get(name))
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty() && value.len() <= MAX_CORRELATOR_LENGTH)
            .map(str::to_string)
            .unwrap_or_else(|| Uuid::new_v4().to_string())
    }

    /// Put `correlator` on a response, when this configuration names a header for it.
    ///
    /// Every reply this crate produces goes through here; it is public so that a
    /// deployment's own refusal — see [`reject`] — carries the header
    /// too rather than being the one answer a peer cannot correlate.
    pub fn echo_correlator(&self, response: &mut Response, correlator: &str) {
        // No let-chains: the workspace's MSRV is 1.85 and they are not stable there.
        let (Some(name), Ok(value)) = (
            self.correlator_header.as_ref(),
            HeaderValue::from_str(correlator),
        ) else {
            return;
        };
        response.headers_mut().insert(name.clone(), value);
    }
}

// ---------------------------------------------------------------------------
// The peer
// ---------------------------------------------------------------------------

/// The peer's address, when the router was served with connect information.
///
/// axum's own [`ConnectInfo`] extractor is not optional — it *fails* when the
/// extension is missing, which turns a direct call from a test or from a nested
/// router into an error. Throttling and audit both want an identity rather than a
/// failure, so this yields `None` instead and an absent address is an unknown peer.
///
/// A deployment mounts the router with
/// `into_make_service_with_connect_info::<SocketAddr>()` to fill it, and a test
/// inserts `ConnectInfo(addr)` into the request's extensions by hand.
#[derive(Debug, Clone, Copy)]
pub struct Peer(
    /// The address, when there is one.
    pub Option<SocketAddr>,
);

impl<S> FromRequestParts<S> for Peer
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Peer(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|ConnectInfo(addr)| *addr),
        ))
    }
}

impl Peer {
    /// The label that goes into [`Context::peer`] — the address without its port, or
    /// `"unknown"`.
    ///
    /// Without the port on purpose: a peer is an identity that throttling and audit
    /// key on, and the ephemeral port changes on every connection.
    pub fn label(self) -> String {
        self.0
            .map(|addr| addr.ip().to_string())
            .unwrap_or_else(|| "unknown".to_string())
    }
}

// ---------------------------------------------------------------------------
// The server
// ---------------------------------------------------------------------------

struct Inner<E, A> {
    /// Shared rather than owned outright, because `nlipsy_core::serve` needs an
    /// [`Arc`] of it: the value it puts at the bottom of the layer stack is itself an
    /// [`Endpoint`], so it cannot borrow the wrapper. One allocation per server.
    conformity: Arc<Conformity<E, A>>,
    layers: Vec<Arc<dyn DynMiddleware>>,
    config: Arc<ServerConfig>,
}

/// An ECMA-431 end-point: an endpoint, wrapped in clause 6, behind a router.
///
/// # The only way in
///
/// There is no constructor that takes a bare [`Endpoint`] and no way to reach the
/// router without one. [`Conformity`] is built *inside* [`Server::new`], so a
/// deployment's layers stack outside it and cannot displace it. This is jsonrpsee's
/// arrangement exactly — its innermost service is always its own `RpcService`,
/// constructed by the server and never by the caller (`server/src/transport/http.rs`)
/// — and it matters here because the token echo is "the easiest one to lose track of:
/// a reply built from scratch silently drops it".
///
/// A constructor is not the whole of it, because a [`Middleware`] can answer `Ok`
/// without calling [`Next::run`](nlipsy_core::Next::run) and never reach
/// [`Conformity::finish`], and can edit the finished reply on the way out. So
/// [`nlipsy_core::serve()`] — the composer this binding drives, which owns the order the
/// clause-6 steps run in — applies [`Conformity::close`] over whatever the layer stack
/// returned, unconditionally: nothing a layer does can remove what clause 6 owes the
/// peer. A layer can still break clause 5 after `finish` — it is holding the reply by
/// value — and the outbound policy does not see it; that is the deployment's bug and
/// this crate cannot catch it.
///
/// # The router
///
/// [`Server::router`] mounts `POST /nlip` and `POST /nlip/` and nothing else: no
/// health routes, no fallback, no `tower` layers. That is what makes it *mergeable* —
/// a deployment adds its own routes with [`Router::merge`], its own fallback, and its
/// own layers around the result — and merging two routers that both set a fallback
/// panics, which is why this one does not.
pub struct Server<E, A> {
    inner: Arc<Inner<E, A>>,
}

impl<E, A> Clone for Server<E, A> {
    /// A handle, not a copy: the endpoint, the layers and the configuration are
    /// shared. axum clones the state once per request, so this has to be cheap.
    fn clone(&self) -> Server<E, A> {
        Server {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<E: Endpoint, A> std::fmt::Debug for Server<E, A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("endpoint", &Endpoint::name(self.inner.conformity.inner()))
            .field("layers", &self.inner.layers.len())
            .field("config", &self.inner.config)
            .finish()
    }
}

/// A [`Server`] under construction, for the deployments that add layers.
///
/// It starts at [`Server::builder`] and ends at [`ServerBuilder::build`], which is
/// the same place [`Server::new`] ends: the endpoint is wrapped in [`Conformity`]
/// either way.
pub struct ServerBuilder<E, A> {
    endpoint: E,
    authority: A,
    config: ServerConfig,
    layers: Vec<Arc<dyn DynMiddleware>>,
}

impl<E: Endpoint, A> std::fmt::Debug for ServerBuilder<E, A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerBuilder")
            .field("endpoint", &Endpoint::name(&self.endpoint))
            .field("layers", &self.layers.len())
            .field("config", &self.config)
            .finish()
    }
}

impl<E: Endpoint, A: TokenAuthority> ServerBuilder<E, A> {
    /// Add a message-level layer, outside the ones already added.
    ///
    /// Outermost first, so the reading order is the running order: the first layer
    /// added sees the exchange first and unwinds last. A layer runs after the message
    /// is decoded and after [`Conformity::check`], and before the endpoint — which is
    /// where a session is admitted, a principal is authenticated or a deadline is
    /// imposed, because all three want to *write* to the [`Context`] the endpoint and
    /// the clause-6 steps then read.
    pub fn layer<M: Middleware>(mut self, middleware: M) -> ServerBuilder<E, A> {
        self.layers.push(Arc::new(middleware));
        self
    }

    /// Wrap the endpoint in clause 6 and hand back the server.
    pub fn build(self) -> Server<E, A> {
        Server {
            inner: Arc::new(Inner {
                conformity: Arc::new(Conformity::new(
                    self.endpoint,
                    self.authority,
                    self.config.conformity,
                    self.config.rejection,
                )),
                layers: self.layers,
                config: Arc::new(self.config),
            }),
        }
    }
}

impl<E: Endpoint, A: TokenAuthority> Server<E, A> {
    /// The only way in: an endpoint, the authority that answers §6.2's exception, and
    /// the binding's configuration.
    pub fn new(endpoint: E, authority: A, config: ServerConfig) -> Server<E, A> {
        Server::builder(endpoint, authority, config).build()
    }

    /// The same, for a deployment that adds [`Middleware`] layers.
    pub fn builder(endpoint: E, authority: A, config: ServerConfig) -> ServerBuilder<E, A> {
        ServerBuilder {
            endpoint,
            authority,
            config,
            layers: Vec::new(),
        }
    }

    /// The endpoint underneath clause 6.
    ///
    /// For a deployment that has something to ask its own handler — the names behind
    /// a dispatcher, for a status page. It is a `&`: the way to *reach* the endpoint
    /// with a message is through the router, which is the point of the wrapper.
    pub fn endpoint(&self) -> &E {
        self.inner.conformity.inner()
    }

    /// The authority answering §6.2's exception.
    pub fn authority(&self) -> &A {
        self.inner.conformity.authority()
    }

    /// The configuration this server was built with.
    pub fn config(&self) -> &ServerConfig {
        &self.inner.config
    }

    /// ECMA-431 §6's endpoint: `POST /nlip` and `POST /nlip/`.
    ///
    /// Both spellings, because the reference client POSTs to `/nlip` while the
    /// reference server mounts `/nlip/`; serving both avoids depending on a redirect,
    /// and on which of the two a given client's HTTP stack follows.
    ///
    /// A `GET` on either is `405` with `Allow: POST` — the path exists and the method
    /// does not — carrying an NLIP error message as its body. §6 obliges `POST` and
    /// forbids nothing, so the *status* is this binding's choice like every other; the
    /// *body* is the crate's principle, that a peer reading only the payload learns
    /// what happened, applied to the one method mismatch the engine controls. The
    /// `Allow` header is axum's own and is attached to the fallback's response by the
    /// method router that knows which methods it has; both survive [`Router::merge`],
    /// which `tests/endpoint.rs` pins. A mismatch on some *other* path is a fallback,
    /// and the fallback stays the deployment's.
    pub fn router(&self) -> Router {
        Router::new()
            .route(NLIP_PATH, post(nlip::<E, A>))
            .route(&format!("{NLIP_PATH}/"), post(nlip::<E, A>))
            // After both routes: it is applied to the method routers that exist when
            // it is called.
            .method_not_allowed_fallback(method_not_allowed)
            .with_state(self.clone())
    }

    /// The optional large-upload leg — ECMA-431 §6.1, advertised by ECMA-430 §6.4.
    ///
    /// A router of its own because it belongs on a listener of its own: §6.1 asks for
    /// "any port different than the port used by NLIP end point". See
    /// [`upload`](crate::upload) for why nothing in clause 6 applies to it.
    pub fn upload_router<S: UploadSink>(&self, sink: S) -> Router {
        let state = UploadState {
            config: Arc::clone(&self.inner.config),
            sink: Arc::new(sink),
        };
        Router::new()
            .route(&format!("{NLIP_PATH}/upload"), post(upload::<S>))
            .route(&format!("{NLIP_PATH}/upload/"), post(upload::<S>))
            .with_state(state)
    }

    /// The whole pipeline for one request, with the span the rest of it logs under.
    ///
    /// Three things happen here and nothing else: the body is read under the limit, the
    /// exchange is handed to [`nlipsy_core::serve`], and what came back is turned into
    /// an HTTP response. The *order* clause 6 runs in — decode, `check`, the layers with
    /// `finish` at the bottom of them, `close` over whatever the stack returned, the
    /// failure rules — is the core's, because none of it is HTTP and a second binding
    /// would otherwise re-derive it and get one of the two properties wrong.
    async fn serve(
        &self,
        peer: String,
        correlator: String,
        principal: Option<Principal>,
        body: Body,
    ) -> Response {
        let config = &self.inner.config;

        // The context the whole exchange is answered against. The layers below fill
        // it in; the endpoint and the clause-6 steps only read it. It is built before
        // the body is even decoded because the failure paths need it too: §6.2's echo
        // is decided by a `TokenAuthority` that reads this.
        let mut cx = Context::new()
            .with_peer(peer)
            .with_correlator(correlator.clone());
        // Before every layer, so that a layer deciding what a caller may do reads the
        // same principal the endpoint does.
        if let Some(principal) = principal {
            cx.extensions.insert(principal);
        }

        let body = match axum::body::to_bytes(body, config.max_request_body_size).await {
            Ok(body) => body,
            Err(error) => return self.unreadable_body(&error, &correlator),
        };

        // The request's `Content-Type` is not read, and that is deliberate: ECMA-431
        // §6 fixes the URL and the method and names no media type, so this binding has
        // none to enforce, and a peer that sends JSON under `text/plain` has sent a
        // message. What comes back does carry one — see `reply::CONTENT_TYPE`.
        //
        // `Wire::Json` and not a choice: ECMA-431 §6 carries JSON. A frame-based
        // binding (ECMA-432 §7.1) passes `Wire::Cbor` to the same composer.
        let served = nlipsy_core::serve(
            &self.inner.conformity,
            &self.inner.layers,
            &body,
            Wire::Json,
            &mut cx,
        )
        .await;

        self.log(&served);

        let status = config.status.of(&served.outcome, served.withheld);
        // A dependency that named a delay is telling the peer when to come back;
        // passing it on turns a blind retry into a paced one.
        let retry_after = served.outcome.retry_after();
        let mut response = json_response(status, &served.reply, retry_after);
        config.echo_correlator(&mut response, &correlator);
        response
    }

    /// What an operator reads afterwards.
    ///
    /// `nlipsy-core` returns its outcome and its clause-5 findings as data rather than
    /// logging them, because it has no logger and should not acquire one. This is the
    /// crate that does.
    ///
    /// The "handled" line no longer carries the request's `format`, `subformat` and
    /// submessage count, and that is the one thing moving the sequence into the core
    /// cost an operator: the composer decodes, so the binding never holds the request.
    /// A deployment that wants those fields decodes the body itself and drives
    /// [`nlipsy_core::serve_message`] instead, which exists for exactly that. The
    /// correlator is on the span either way, so a line here and the reply on the wire
    /// still join up.
    fn log(&self, served: &Served) {
        let endpoint = Endpoint::name(self.inner.conformity.inner());
        match &served.outcome {
            Outcome::Ok => info!(
                endpoint,
                echoed_tokens = served.reply.tokens().count(),
                withheld = served.withheld,
                "handled"
            ),
            Outcome::Malformed(error) => warn!(%error, "rejected: message could not be decoded"),
            Outcome::Refused(rejected) => warn!(%rejected, "rejected: {rejected}"),
            // `error` rather than `public_message`: the detail belongs in the log
            // and, for `Internal`, nowhere else (ECMA-434 §7.3.3).
            Outcome::Failed(error) => warn!(endpoint, %error, "the endpoint could not answer"),
            // `Outcome` is `#[non_exhaustive]`: a core newer than this binding can end
            // an exchange in a way it has no line for, and silence would be worse.
            other => warn!(endpoint, "the exchange ended as {other:?}"),
        }
        log_findings(&served.findings);
    }

    /// The body could not be collected: too large, or the stream broke.
    ///
    /// `axum::body::to_bytes` returns one error for both, so the two are told apart
    /// by the source it wraps. They are worth telling apart: one is the peer's doing
    /// and has a well-known status, the other is nobody's message to read.
    ///
    /// §6.2 cannot be honoured on either — the body was never decoded, so no token was
    /// ever read — and §6.1 still is: what goes back is an NLIP message with a first
    /// submessage. This is also the one refusal after which a peer should treat its
    /// state as untouched: nothing was consumed.
    ///
    /// The broken-stream half answers with
    /// [`StatusMap::internal`](nlipsy_http::StatusMap::internal), whose doc carries the
    /// argument for that code: nothing in ECMA-431 speaks to the case, and the reply
    /// rarely reaches a peer that is no longer there.
    fn unreadable_body(&self, error: &axum::Error, correlator: &str) -> Response {
        let config = &self.inner.config;
        let limit = config.max_request_body_size;
        let too_large = std::error::Error::source(error)
            .is_some_and(|source| source.is::<http_body_util::LengthLimitError>());

        let mut response = if too_large {
            warn!(limit, "rejected: the request body is larger than the limit");
            json_response(
                config.status.too_large,
                &nlipsy_http::too_large(limit),
                None,
            )
        } else {
            warn!(%error, "the request body could not be read");
            reject(
                config.status.internal,
                "the request body could not be read",
                None,
            )
        };
        config.echo_correlator(&mut response, correlator);
        response
    }
}

/// ECMA-431 §6 obliges `POST` and forbids nothing, so another method on the endpoint's
/// own path is `405` — with a body, because a status line alone tells a peer reading
/// the payload nothing. axum adds `Allow: POST` to whatever this returns.
async fn method_not_allowed() -> Response {
    json_response(
        StatusCode::METHOD_NOT_ALLOWED,
        &nlipsy_http::method_not_allowed(),
        None,
    )
}

/// `POST /nlip` — the mandatory endpoint.
///
/// The correlator and the span are established here and attached to everything the
/// pipeline does. Note the instrumented future rather than a `span.enter()` guard: an
/// `Entered` guard held across an await makes the handler future non-`Send`, which
/// axum rejects at compile time.
async fn nlip<E, A>(
    State(server): State<Server<E, A>>,
    peer: Peer,
    headers: HeaderMap,
    body: Body,
) -> Response
where
    E: Endpoint,
    A: TokenAuthority,
{
    let client = peer.label();
    let correlator = server.inner.config.correlator_for(&headers);
    let span = tracing::info_span!("nlip", client = %client, correlator = %correlator);
    let principal = span.in_scope(|| server.inner.config.principal_for(&headers));
    server
        .serve(client, correlator, principal, body)
        .instrument(span)
        .await
}

struct UploadState<S> {
    config: Arc<ServerConfig>,
    sink: Arc<S>,
}

impl<S> Clone for UploadState<S> {
    fn clone(&self) -> UploadState<S> {
        UploadState {
            config: Arc::clone(&self.config),
            sink: Arc::clone(&self.sink),
        }
    }
}

/// `POST /nlip/upload` — the out-of-band leg, outside clause 6.
async fn upload<S: UploadSink>(
    State(state): State<UploadState<S>>,
    peer: Peer,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let client = peer.label();
    let correlator = state.config.correlator_for(&headers);
    let span = tracing::info_span!("upload", client = %client, correlator = %correlator);

    async move {
        let config = &state.config;
        let body = match axum::body::to_bytes(body, config.max_request_body_size).await {
            Ok(body) => body,
            Err(error) => {
                // The same two cases as the endpoint's, minus the echo there is
                // nothing to echo from.
                let limit = config.max_request_body_size;
                let too_large = std::error::Error::source(&error)
                    .is_some_and(|source| source.is::<http_body_util::LengthLimitError>());
                warn!(%error, limit, too_large, "the upload body could not be read");
                let (status, why) = if too_large {
                    (
                        config.status.too_large,
                        format!("the upload exceeds this endpoint's limit of {limit} bytes"),
                    )
                } else {
                    (
                        config.status.internal,
                        "the upload could not be read".to_string(),
                    )
                };
                let mut response = reject(status, &why, None);
                config.echo_correlator(&mut response, &correlator);
                return response;
            }
        };

        let size = body.len();
        let mut cx = Context::new()
            .with_peer(client)
            .with_correlator(correlator.clone());
        // The upload leg has no clause 6 around it, but a sink deciding whether to
        // keep what it is given wants to know who sent it as much as an endpoint does.
        if let Some(principal) = config.principal_for(&headers) {
            cx.extensions.insert(principal);
        }
        let upload = Upload {
            body,
            headers: &headers,
            cx: &cx,
        };

        let mut response = match state.sink.store(upload).await {
            Ok(reply) => {
                info!(size, "upload accepted");
                json_response(StatusCode::OK, &reply, None)
            }
            Err(error) => {
                warn!(%error, size, "the upload could not be stored");
                reject(
                    config.status.of_error(&error),
                    &error.public_message(),
                    error.retry_after(),
                )
            }
        };
        config.echo_correlator(&mut response, &correlator);
        response
    }
    .instrument(span)
    .await
}

/// Write out what clause 5 said about the exchange and what the policy did about it.
///
/// A rejected finding is an error, a kept one is a warning, an ignored one says nothing
/// at all. `violation.path` is what names the message and the submessage — the findings
/// arrive as one list, the request's before the reply's, and the path is what tells an
/// operator where to look.
fn log_findings(findings: &Findings) {
    for finding in findings.iter() {
        let violation = &finding.violation;
        match finding.disposition {
            Disposition::Reject => error!(
                rule = violation.rule,
                at = %violation.path,
                "the exchange is not conforming: {violation}"
            ),
            Disposition::Log => warn!(
                rule = violation.rule,
                at = %violation.path,
                "the exchange is off-spec: {violation}"
            ),
            Disposition::Ignore => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_correlator_is_reused_when_it_is_sane_and_minted_when_it_is_not() {
        let config = ServerConfig::new();

        let mut headers = HeaderMap::new();
        headers.insert(CORRELATOR_HEADER, HeaderValue::from_static("abc-123"));
        assert_eq!(config.correlator_for(&headers), "abc-123");

        assert!(!config.correlator_for(&HeaderMap::new()).is_empty());

        let mut headers = HeaderMap::new();
        headers.insert(CORRELATOR_HEADER, HeaderValue::from_static(""));
        assert!(!config.correlator_for(&headers).is_empty());

        // A 4 KiB header must not become a 4 KiB span field.
        let long = "x".repeat(4096);
        let mut headers = HeaderMap::new();
        headers.insert(CORRELATOR_HEADER, HeaderValue::from_str(&long).unwrap());
        assert_ne!(config.correlator_for(&headers), long);
    }

    #[test]
    fn without_a_header_the_correlator_is_still_minted() {
        // The header is the spelling; the correlator is the concept. A deployment
        // that carries no header still gets an identifier for its logs.
        let config = ServerConfig::new().with_correlator_header(None);
        let mut headers = HeaderMap::new();
        headers.insert(CORRELATOR_HEADER, HeaderValue::from_static("abc-123"));
        assert_ne!(config.correlator_for(&headers), "abc-123");
        assert!(!config.correlator_for(&headers).is_empty());
    }

    fn principal_config() -> ServerConfig {
        ServerConfig::new().with_principal_header(Some(HeaderName::from_static("x-principal")))
    }

    #[test]
    fn a_principal_is_carried_only_when_a_header_is_named_for_one() {
        let mut headers = HeaderMap::new();
        headers.insert("x-principal", HeaderValue::from_static(" human:alice "));

        assert_eq!(
            principal_config().principal_for(&headers),
            Some(Principal::new("human:alice"))
        );
        // A deployment that named none trusts no header, whatever the caller sends.
        assert_eq!(ServerConfig::new().principal_for(&headers), None);
        assert_eq!(principal_config().principal_for(&HeaderMap::new()), None);
    }

    #[test]
    fn a_principal_header_that_a_proxy_would_not_have_written_carries_nothing() {
        // Two values: a copy that got past the proxy beside the one it wrote.
        let mut twice = HeaderMap::new();
        twice.append("x-principal", HeaderValue::from_static("human:alice"));
        twice.append("x-principal", HeaderValue::from_static("machine:mallory"));
        assert_eq!(principal_config().principal_for(&twice), None);

        let mut empty = HeaderMap::new();
        empty.insert("x-principal", HeaderValue::from_static("   "));
        assert_eq!(principal_config().principal_for(&empty), None);

        let long = "x".repeat(MAX_PRINCIPAL_LENGTH + 1);
        let mut too_long = HeaderMap::new();
        too_long.insert("x-principal", HeaderValue::from_str(&long).unwrap());
        assert_eq!(principal_config().principal_for(&too_long), None);

        let mut opaque = HeaderMap::new();
        opaque.insert("x-principal", HeaderValue::from_bytes(b"caf\xe9").unwrap());
        assert_eq!(principal_config().principal_for(&opaque), None);
    }

    #[test]
    fn an_unknown_peer_is_a_label_and_not_a_failure() {
        assert_eq!(Peer(None).label(), "unknown");
        assert_eq!(
            Peer(Some("203.0.113.7:44321".parse().unwrap())).label(),
            "203.0.113.7",
            "the ephemeral port changes per connection and is not an identity"
        );
    }
}
