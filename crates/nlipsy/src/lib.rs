// The facade. It holds no code of its own: every line of NLIP lives in a `nlipsy-*`
// crate, and this one re-exports those crates as modules behind feature flags — the
// shape jsonrpsee gives `jsonrpsee_types` as `jsonrpsee::types`.
//
// What that buys is a name and a pattern fixed before the remaining bindings arrive.
// ECMA-432 and ECMA-433 each add a crate over the same message model and the same
// clause 6; a consumer who depends on `nlipsy` today gains each of them by adding a
// word to `features`, and never by editing the name of a dependency.
//
// As in the protocol crate, the crate documentation *is* the README: one text
// rendered by GitHub, crates.io and rustdoc, so the feature table a reader sees on
// the repository page is the one docs.rs shows. The examples in the README are
// therefore fenced `ignore` and their compiled copies live on the modules below — a
// doctest in the README is compiled by `cargo test` with no features on, where none
// of these modules exists.
//
// `doc_cfg` is nightly-only and enabled under `--cfg docsrs`, which docs.rs passes
// (see `[package.metadata.docs.rs]`). It is what labels each module with the feature
// that produces it — worth more here than in a normal crate, since with no features
// on there is nothing else on the page at all.
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc = include_str!("../README.md")]

// Not exposed: the macros are how this crate writes its own re-exports, not something
// a consumer of NLIP has any use for.
#[macro_use]
mod macros;

cfg_feature! {
    "protocol",
    /// The NLIP message model — ECMA-430 clause 5.
    ///
    /// This module *is* the [`nlipsy-protocol`] crate, re-exported. Depending on that
    /// crate directly is equally supported and gets the identical types; the facade
    /// exists so that a consumer who later wants a transport binding adds a feature
    /// rather than a dependency.
    ///
    /// The example the README shows, compiled. It is written here rather than in the
    /// README because a doctest in the README runs on every `cargo test`, including
    /// the one with no features on, where `nlipsy::protocol` does not exist — the
    /// same reason the protocol crate keeps its CBOR example on the CBOR entry
    /// points. The doctest exists only when the feature does, because the item it
    /// documents does.
    ///
    /// ```
    /// use nlipsy::protocol::{Format, Message, Submessage};
    ///
    /// let question = Message::from_json_str(
    ///     r#"{"format": "text", "subformat": "english", "content": "What is Ecma?"}"#,
    /// )
    /// .unwrap();
    ///
    /// assert_eq!(question.format, Format::Text);
    ///
    /// let answer = Message::text("An international standards body.")
    ///     .with_submessage(Submessage::token("conversation", "c1"));
    ///
    /// assert!(answer.to_json_string().contains("conversation"));
    /// ```
    ///
    /// [`nlipsy-protocol`]: https://docs.rs/nlipsy-protocol
    pub use nlipsy_protocol as protocol;
}

cfg_client_or_server_core! {
    /// The runtime core — ECMA-430 clause 6, and the seams a runtime plugs into.
    ///
    /// This module *is* the [`nlipsy-core`] crate, re-exported. It is the one module
    /// two features produce: `client-core` turns on the outbound half (`Transport`,
    /// `Client`, `Reply`, record and replay) and `server-core` the inbound half
    /// (`Conformity`, `Endpoint`, `TokenAuthority`, `Middleware`, `Dispatcher`), and a
    /// deployment that federates asks for both. `Context` is in either.
    ///
    /// Neither `client` nor `server` is a *default* of this crate, so asking for
    /// `http-client` or `server` is what brings the matching half; the module appears
    /// the moment one of them does.
    ///
    /// The example here is on [`http_client`] and [`server`] instead, where the role
    /// the code takes is what the feature name says.
    ///
    /// [`nlipsy-core`]: https://docs.rs/nlipsy-core
    /// [`http_client`]: crate::http_client
    /// [`server`]: crate::server
    pub use nlipsy_core as core;
}

cfg_http! {
    /// ECMA-431 §6 as values — the status map, the header spellings, `Retry-After` in
    /// both directions, the messages the binding builds itself.
    ///
    /// This module *is* the [`nlipsy-http`] crate, re-exported. It is the second module
    /// more than one feature produces, and for the reason [`core`] is the first: a
    /// client agent *reads* the status and the `Retry-After` a server agent *wrote*, so
    /// both roles need the same values and neither owns them. `http-client`,
    /// `http-client-hyper` and `server` each bring it — the client binding has a
    /// transport feature each for reqwest and hyper, and the values are the binding's
    /// either way; there is no `http` feature of its own.
    ///
    /// ```
    /// use nlipsy::core::Outcome;
    /// use nlipsy::http::{StatusMap, request};
    ///
    /// // What a peer sees when clause 5 rejected this end-point's own answer: the
    /// // question was valid and got no answer to it, which is the end-point failing.
    /// let codes = StatusMap::default();
    /// assert_eq!(codes.of(&Outcome::Ok, false), 200);
    /// assert_eq!(codes.of(&Outcome::Ok, true), 500);
    ///
    /// // TR/113 §11.1's chat turn, with §6.2.1's conversation token beside the text.
    /// let turn = request("What is Ecma?", "english", "conv-1");
    /// assert!(turn.submessages[0].is_conversation_token());
    /// ```
    ///
    /// [`nlipsy-http`]: https://docs.rs/nlipsy-http
    /// [`core`]: crate::core
    pub use nlipsy_http as http;
}

cfg_http_client! {
    /// The ECMA-431 client binding — one `POST` to `/nlip`, over reqwest or over hyper.
    ///
    /// This module *is* the [`nlipsy-http-client`] crate, re-exported. It supplies the
    /// [`Transport`] that [`core::Client`] sends over, the two constants ECMA-431 §6
    /// fixes (`NLIP_PATH`, `CORRELATOR_HEADER`), and the file-backed half of the core's
    /// recorder.
    ///
    /// **Two transports, two features, one module.** `http-client` brings
    /// `HttpTransport`, which verifies against the platform's trust store through
    /// reqwest; `http-client-hyper` brings `HyperTransport`, which verifies against a
    /// root bundle compiled in from `webpki-roots` and whose whole reason to exist is
    /// that reqwest's TLS cannot be built for an Android or an iOS app — it needs `jni`
    /// and a vendored BoringSSL, and a feature cannot remove a dependency. Asking for
    /// `http-client-hyper` alone compiles no reqwest at all; asking for both gives this
    /// module with both types in it. Which to pick, and what the second one gives up, is
    /// in [`nlipsy-http-client`]'s own README.
    ///
    /// `http-client-hyper` also brings that transport's two extensions, under the same
    /// paths: `Dialer` — with `DialTarget`, `DialFuture`, `DialStream` and `DialedStream`
    /// — through which a consumer that routes its own traffic (an in-process tunnel, a
    /// proxy) opens the connections instead of the system resolver and a socket; and
    /// `HyperTransportBuilder::add_root_certificate`, for an end-point behind a private
    /// CA.
    ///
    /// The client role, compiled — a message, the per-exchange context, and the
    /// transport that carries them:
    ///
    /// ```
    /// use nlipsy::core::{Client, Context};
    /// use nlipsy::protocol::{Message, Submessage};
    ///
    /// // ECMA-430 §6.1: a client agent's initial request is a message with a first
    /// // submessage. §6.2's conversation token rides beside it.
    /// let request = Message::text("What is Ecma?")
    ///     .with_submessage(Submessage::token("conversation", "c-1"));
    ///
    /// // What the runtime knows about this exchange and the message does not. The
    /// // correlator is ECMA-431's `x-nlip-correlator`, sent only when one is set.
    /// let cx = Context::new().with_correlator("turn-1");
    ///
    /// // The one line that differs between the two features, and which of the two is
    /// // compiled here is which of them this build asked for. Everything above and
    /// // below is the same either way, which is the point of there being two.
    /// #[cfg(feature = "http-client")]
    /// let client = Client::new(
    ///     nlipsy::http_client::HttpTransport::new("https://agent.example.com"),
    /// );
    /// #[cfg(all(feature = "http-client-hyper", not(feature = "http-client")))]
    /// let client = Client::new(
    ///     nlipsy::http_client::HyperTransport::new("https://agent.example.com"),
    /// );
    ///
    /// // One `await` over the transport: encode, exchange, decode. Not called here,
    /// // because a doctest has no endpoint to talk to — this is the shape.
    /// async fn ask(client: &Client, request: &Message, cx: &Context) -> Option<String> {
    ///     client.send(request, cx).await.ok()?.text()
    /// }
    /// let _ = ask(&client, &request, &cx);
    /// ```
    ///
    /// [`nlipsy-http-client`]: https://docs.rs/nlipsy-http-client
    /// [`Transport`]: crate::core::Transport
    /// [`core::Client`]: crate::core::Client
    pub use nlipsy_http_client as http_client;
}

cfg_feature! {
    "server",
    /// The ECMA-431 server binding — one `axum::Router` for `POST /nlip`.
    ///
    /// This module *is* the [`nlipsy-server`] crate, re-exported. [`Server::new`] is
    /// the only way in, and it builds [`core::Conformity`] itself: a deployment's
    /// layers stack *outside* clause 6, so the §6.2 echo and the §6.3 carry cannot be
    /// displaced by one.
    ///
    /// The server role, compiled. It stops at the `axum::Router`, which is where this
    /// crate stops: binding a listener and calling `axum::serve` on it is the
    /// deployment's line, and writing it here would make the facade carry a tokio
    /// dev-dependency for one doctest — jsonrpsee's facade has none either.
    ///
    /// ```
    /// use nlipsy::core::{Context, Endpoint, EndpointError, TokenAuthority};
    /// use nlipsy::protocol::Message;
    /// use nlipsy::server::{Server, ServerConfig};
    ///
    /// struct Echo;
    ///
    /// impl Endpoint for Echo {
    ///     fn name(&self) -> &str {
    ///         "echo"
    ///     }
    ///
    ///     async fn handle(&self, request: &Message, _cx: &Context) -> Result<Message, EndpointError> {
    ///         Ok(Message::text(request.content_as_str().unwrap_or_default()))
    ///     }
    /// }
    ///
    /// // §6.2's exception, answered by the deployment: this endpoint creates no
    /// // tokens, so it created none of the peer's either and every one comes back.
    /// struct NoTokens;
    ///
    /// impl TokenAuthority for NoTokens {
    ///     fn is_own(&self, _token: &str, _cx: &Context) -> bool {
    ///         false
    ///     }
    /// }
    ///
    /// // The only constructor there is, so clause 6 is underneath the router by
    /// // construction rather than by remembering to put it there.
    /// let server = Server::new(Echo, NoTokens, ServerConfig::new());
    ///
    /// // `POST /nlip` and `POST /nlip/`, and nothing else. A deployment merges its
    /// // own routes onto this and hands the result to `axum::serve`.
    /// let app: nlipsy::server::axum::Router = server.router();
    /// let _ = app;
    /// ```
    ///
    /// [`nlipsy-server`]: https://docs.rs/nlipsy-server
    /// [`Server::new`]: nlipsy_server::Server::new
    /// [`core::Conformity`]: crate::core::Conformity
    pub use nlipsy_server as server;
}
