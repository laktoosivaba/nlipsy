//! The ECMA-431 binding, end to end.
//!
//! These drive the real `Router` through `tower`'s `oneshot`, so everything the
//! binding does to a request — reading the body under the limit, decoding, checking,
//! running the layers, closing clause 6 over the answer, encoding — is exercised the
//! way a peer would exercise it, minus the socket. The device is deliberate: a
//! pipeline asserted on through its public surface is a pipeline a refactor cannot
//! quietly change.

use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request, StatusCode, header};
use nlipsy_core::{
    Context, Endpoint, EndpointError, Middleware, Next, Rejection, TokenAuthority, TokenKind,
};
use nlipsy_protocol::message::{Message, Submessage};
use nlipsy_server::{
    CORRELATOR_HEADER, DiscardSink, Peer, Principal, Server, ServerConfig, StatusMap, Upload,
    UploadSink, reject,
};
use serde_json::Value;
use tower::ServiceExt;

const PEER: &str = "203.0.113.7:44321";

// ---------------------------------------------------------------------------
// The endpoint, the authority, and the harness
// ---------------------------------------------------------------------------

/// Answers with what it was sent, so a test can tell the endpoint's own answer from
/// everything clause 6 added around it.
struct Echo;

impl Endpoint for Echo {
    fn name(&self) -> &str {
        "echo"
    }

    async fn handle(&self, request: &Message, _cx: &Context) -> Result<Message, EndpointError> {
        Ok(Message::text(request.content_as_str().unwrap_or("")))
    }
}

/// Fails the same way every time, so each `EndpointError` can be checked against the
/// status it is supposed to become.
struct Failing(EndpointError);

impl Endpoint for Failing {
    fn name(&self) -> &str {
        "failing"
    }

    async fn handle(&self, _request: &Message, _cx: &Context) -> Result<Message, EndpointError> {
        Err(self.0.clone())
    }
}

/// Returns a message that breaks clause 5, to check the reply sanitiser is in the
/// path.
struct NonConforming;

impl Endpoint for NonConforming {
    fn name(&self) -> &str {
        "non-conforming"
    }

    async fn handle(&self, _request: &Message, _cx: &Context) -> Result<Message, EndpointError> {
        Ok(Message::new(
            nlipsy_protocol::message::Format::Text,
            "english",
            serde_json::json!({"not": "a string"}),
        ))
    }
}

/// Takes as long as it is told, so a deadline layer has something to cut off.
struct Slow(Duration);

impl Endpoint for Slow {
    fn name(&self) -> &str {
        "slow"
    }

    async fn handle(&self, _request: &Message, _cx: &Context) -> Result<Message, EndpointError> {
        tokio::time::sleep(self.0).await;
        Ok(Message::text("eventually"))
    }
}

/// §6.2's exception, in memory: this endpoint's tokens are the ones it minted, and it
/// mints at most one of each kind.
#[derive(Default)]
struct Memory {
    conversation: Mutex<Option<String>>,
}

impl Memory {
    /// An endpoint that has already issued a conversation token and is confirming it —
    /// what a peer's second and every later request meets.
    fn in_conversation(token: &str) -> Memory {
        Memory {
            conversation: Mutex::new(Some(token.to_string())),
        }
    }
}

impl TokenAuthority for Memory {
    fn is_own(&self, token: &str, _cx: &Context) -> bool {
        self.conversation.lock().unwrap().as_deref() == Some(token)
    }

    fn issued(&self, _kind: TokenKind, _cx: &Context) -> Option<String> {
        self.conversation.lock().unwrap().clone()
    }

    fn mint(&self, _kind: TokenKind, cx: &Context) -> Option<String> {
        // Reads what a layer wrote, which is the property the layer seam exists for:
        // `finish` runs under the layers, so whatever they put in the context is
        // there when §6.2.1 asks for a token.
        let tenant = cx
            .extensions
            .get::<Tenant>()
            .map(|tenant| tenant.0.as_str())
            .unwrap_or("anonymous");
        let minted = format!("conv-for-{tenant}");
        *self.conversation.lock().unwrap() = Some(minted.clone());
        Some(minted)
    }
}

/// A deployment's own value, which the core has never heard of.
#[derive(Debug, Clone)]
struct Tenant(String);

/// A layer that writes to the context before anything below it runs.
struct WithTenant(&'static str);

impl Middleware for WithTenant {
    async fn call(
        &self,
        request: &Message,
        cx: &mut Context,
        next: Next<'_>,
    ) -> Result<Message, EndpointError> {
        cx.extensions.insert(Tenant(self.0.to_string()));
        next.run(request, cx).await
    }
}

/// A layer that bounds how long the endpoint may take — the deployment's, because the
/// *value* of a deadline is a deployment's and the clock is outside the core.
struct Deadline(Duration);

impl Middleware for Deadline {
    async fn call(
        &self,
        request: &Message,
        cx: &mut Context,
        next: Next<'_>,
    ) -> Result<Message, EndpointError> {
        match tokio::time::timeout(self.0, next.run(request, cx)).await {
            Ok(outcome) => outcome,
            Err(_) => Err(EndpointError::TimedOut { after: self.0 }),
        }
    }
}

/// A layer that answers on its own without calling `next` — a cache, a canned answer,
/// a maintenance-mode page. It knows nothing about clause 6, which is the point: it is
/// an ordinary thing to write and it never reaches `Conformity::finish`.
struct Cached;

impl Middleware for Cached {
    async fn call(
        &self,
        _request: &Message,
        _cx: &mut Context,
        _next: Next<'_>,
    ) -> Result<Message, EndpointError> {
        Ok(Message::text("cached"))
    }
}

/// A layer that edits the finished reply on the way out, and edits away exactly what
/// §6.2 put there.
struct StripTokens;

impl Middleware for StripTokens {
    async fn call(
        &self,
        request: &Message,
        cx: &mut Context,
        next: Next<'_>,
    ) -> Result<Message, EndpointError> {
        let mut reply = next.run(request, cx).await?;
        reply
            .submessages
            .retain(|submessage| !submessage.is_token());
        Ok(reply)
    }
}

/// A layer that refuses, to prove nothing below it runs.
struct AtCapacity;

impl Middleware for AtCapacity {
    async fn call(
        &self,
        _request: &Message,
        _cx: &mut Context,
        _next: Next<'_>,
    ) -> Result<Message, EndpointError> {
        Err(EndpointError::Unavailable {
            detail: "the endpoint is at capacity; retry later".to_string(),
            retry_after: None,
        })
    }
}

fn app<E: Endpoint>(endpoint: E) -> Router {
    Server::new(endpoint, Memory::default(), ServerConfig::new()).router()
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

impl Reply {
    fn text(&self) -> &str {
        self.body["content"].as_str().unwrap_or_default()
    }
}

async fn post(app: &Router, uri: &str, body: &str) -> Reply {
    post_with(app, uri, body, &[]).await
}

async fn post_with(app: &Router, uri: &str, body: &str, headers: &[(&str, &str)]) -> Reply {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let mut request = builder
        .body(Body::from(body.to_string()))
        .expect("request should build");
    // A server started with connect information always provides one; a test supplies
    // it directly, so `Context::peer` is what it would be on a socket.
    request
        .extensions_mut()
        .insert(ConnectInfo(PEER.parse::<SocketAddr>().unwrap()));

    let response = app
        .clone()
        .oneshot(request)
        .await
        .expect("router should answer");

    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should be readable");
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);

    Reply {
        status,
        headers,
        body,
    }
}

// ---------------------------------------------------------------------------
// ECMA-431 §6 — the mandatory surface
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_text_request_gets_a_text_answer_at_both_spellings_of_the_path() {
    for path in ["/nlip", "/nlip/"] {
        let reply = post(
            &app(Echo),
            path,
            r#"{"format":"text","subformat":"english","content":"hi"}"#,
        )
        .await;

        assert_eq!(reply.status, StatusCode::OK, "{path}");
        assert_eq!(reply.body["format"], "text");
        assert_eq!(reply.text(), "hi");
        assert_eq!(
            reply.headers[header::CONTENT_TYPE],
            "application/json",
            "JSON per ECMA-430 §5 ¶2 at ECMA-433 §6.1.5's media type — ECMA-431 §6 \
             names neither — whatever the body's Rust type was"
        );
    }
}

#[tokio::test]
async fn a_get_on_the_endpoint_is_not_a_message() {
    let response = app(Echo)
        .oneshot(Request::builder().uri("/nlip").body(Body::empty()).unwrap())
        .await
        .unwrap();

    // The path exists and the method does not. ECMA-431 §6 obliges POST and forbids
    // nothing, so the status is this binding's choice; the body is the crate's
    // principle, that a peer reading only the payload learns what happened.
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(response.headers()[header::ALLOW], "POST");
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).expect("the 405 carries a message");
    assert_eq!(body["messagetype"], "Error");
    assert_eq!(body["format"], "text");
    assert!(body["content"].as_str().unwrap().contains("POST"), "{body}");
}

#[tokio::test]
async fn the_405_body_and_its_allow_header_survive_a_merge() {
    // The fallback is attached to this router's own method routers, so a deployment
    // that merges its routes onto it keeps both. If axum ever stopped carrying it,
    // this is the test that would say so before a deployment found out.
    let app = app(Echo)
        .merge(Router::new().route("/health", axum::routing::get(|| async { "ok" })))
        .merge(Router::new().fallback(|| async { StatusCode::NOT_FOUND }));

    let response = app
        .clone()
        .oneshot(Request::builder().uri("/nlip").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(response.headers()[header::ALLOW], "POST");
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).expect("the 405 carries a message");
    assert_eq!(body["messagetype"], "Error");

    // And the deployment's own routes still answer.
    let health = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
}

#[tokio::test]
async fn the_router_carries_no_fallback_so_a_deployment_can_merge_one() {
    // Merging two routers that both set a fallback panics, so the binding sets none.
    let app = app(Echo).merge(
        Router::new().fallback(|| async { (StatusCode::NOT_FOUND, "the deployment's own") }),
    );
    let response = app
        .oneshot(Request::builder().uri("/nope").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// §6.2 token echo
// ---------------------------------------------------------------------------

#[tokio::test]
async fn token_submessages_come_back_unchanged() {
    let reply = post(
        &app(Echo),
        "/nlip",
        r#"{"format":"text","subformat":"english","content":"hi",
            "submessages":[
              {"format":"token","subformat":"conversation_9.2.3.5","content":"abc","label":"seq-1"},
              {"format":"token","subformat":"authorization","content":"tok-42"}
            ]}"#,
    )
    .await;

    assert_eq!(reply.status, StatusCode::OK);
    let tokens = reply.body["submessages"].as_array().unwrap();
    assert_eq!(tokens.len(), 2);
    assert_eq!(tokens[0]["subformat"], "conversation_9.2.3.5");
    assert_eq!(tokens[0]["content"], "abc");
    assert_eq!(tokens[0]["label"], "seq-1");
    assert_eq!(tokens[1]["subformat"], "authorization");
    assert_eq!(tokens[1]["content"], "tok-42");
}

#[tokio::test]
async fn a_request_that_is_itself_a_token_is_echoed_back() {
    // The shape the reference SDK's `create_token` emits: the whole message is the
    // token. §5 ¶2 calls the leading object "the first submessage", so §6.2 ¶2 applies
    // to it — and the echo lands in the reply's `submessages`, because the reply's own
    // first submessage is the endpoint's answer.
    let reply = post(
        &app(Echo),
        "/nlip",
        r#"{"format":"token","subformat":"conversation_9.2.3.5","content":"abc"}"#,
    )
    .await;

    assert_eq!(reply.status, StatusCode::OK);
    let echoed = &reply.body["submessages"][0];
    assert_eq!(echoed["format"], "token");
    assert_eq!(echoed["subformat"], "conversation_9.2.3.5");
    assert_eq!(echoed["content"], "abc");
}

#[tokio::test]
async fn tokens_are_echoed_even_when_the_request_is_refused() {
    // §6.2 speaks of "any NLIP message" carrying tokens, so a rejected request still
    // returns them — a peer that can decode the error keeps its state in sync.
    let reply = post(
        &app(Echo),
        "/nlip",
        r#"{"format":"text","subformat":"english","content":{"not":"a string"},
            "submessages":[{"format":"token","subformat":"conversation","content":"abc"}]}"#,
    )
    .await;

    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(reply.body["messagetype"], "Error");
    assert_eq!(reply.body["submessages"][0]["content"], "abc");
}

// ---------------------------------------------------------------------------
// §6.3 control
// ---------------------------------------------------------------------------

#[tokio::test]
async fn control_is_carried_in_whichever_spelling_the_peer_used() {
    let by_type = post(
        &app(Echo),
        "/nlip",
        r#"{"messagetype":"control","format":"text","subformat":"english","content":"?"}"#,
    )
    .await;
    assert_eq!(by_type.body["messagetype"], "control");
    assert_eq!(by_type.body["control"], true);

    let by_boolean = post(
        &app(Echo),
        "/nlip",
        r#"{"control":true,"format":"text","subformat":"english","content":"?"}"#,
    )
    .await;
    // Neither field is invented where the peer did not send it: §5.1.1's value and
    // §6.3's boolean are two spellings of one thing.
    assert_eq!(by_boolean.body["control"], true);
    assert!(
        by_boolean.body["messagetype"].is_null(),
        "{}",
        by_boolean.body
    );
}

#[tokio::test]
async fn a_failed_control_exchange_stays_a_control_exchange() {
    let app = app(Failing(EndpointError::Internal("x".to_string())));
    let reply = post(
        &app,
        "/nlip",
        r#"{"messagetype":"control","format":"text","subformat":"english","content":"?"}"#,
    )
    .await;

    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR);
    // §6.3 is satisfied by the boolean, which is the only field the clause names, and
    // `Error` survives as the one in-band marker a peer has for a failure: §5.1.1
    // gives `MessageType` one value, so the reply cannot spell both there.
    assert_eq!(reply.body["messagetype"], "Error");
    assert_eq!(reply.body["control"], true);
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_malformed_body_is_an_nlip_error_with_the_correlator_echoed() {
    let reply = post_with(
        &app(Echo),
        "/nlip",
        "{not json",
        &[(CORRELATOR_HEADER, "trace-me")],
    )
    .await;

    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(reply.body["messagetype"], "Error");
    assert_eq!(reply.body["format"], "text");
    assert_eq!(reply.headers[CORRELATOR_HEADER], "trace-me");
    assert_eq!(reply.headers[header::CONTENT_TYPE], "application/json");
}

#[tokio::test]
async fn an_oversized_body_is_an_nlip_error_rather_than_the_frameworks_own_words() {
    // The rejection a `RequestBodyLimitLayer` outside the router would answer in plain
    // text, answered in the protocol instead — which is the whole reason the limit is
    // a `ServerConfig` field and enforced here.
    let server = Server::new(
        Echo,
        Memory::default(),
        ServerConfig::new().with_max_request_body_size(64),
    );
    let body = format!(
        r#"{{"format":"text","subformat":"english","content":"{}"}}"#,
        "x".repeat(200)
    );
    let reply = post(&server.router(), "/nlip", &body).await;

    assert_eq!(reply.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(reply.body["messagetype"], "Error");
    assert!(reply.text().contains("64"), "{}", reply.text());
}

#[tokio::test]
async fn a_format_outside_the_five_never_becomes_a_message_at_all() {
    // `Format` is closed (§5.1.2), so this is refused by the decoder rather than by
    // clause 5 — `StatusMap::malformed`, and the error names what was allowed.
    let reply = post(
        &app(Echo),
        "/nlip",
        r#"{"format":"audio","subformat":"english","content":"x"}"#,
    )
    .await;

    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(reply.body["messagetype"], "Error");
    assert!(reply.text().contains("structured"), "{}", reply.text());
}

#[tokio::test]
async fn more_tokens_than_the_endpoint_will_echo_are_refused() {
    let tokens: Vec<Value> = (0..40)
        .map(|index| {
            serde_json::json!({"format":"token","subformat":"conversation","content":format!("t{index}")})
        })
        .collect();
    let request = serde_json::json!({
        "format": "text",
        "subformat": "english",
        "content": "hi",
        "submessages": tokens,
    });

    let reply = post(&app(Echo), "/nlip", &request.to_string()).await;

    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert!(reply.text().contains("token"), "{}", reply.text());
    // And it echoes none of them, so the refusal's text is true and the ceiling bounds
    // what it says it bounds. This is the engine's one deliberate departure from
    // §6.2 ¶2, taken under ECMA-434 §7.2.5 rather than under clause 6.
    assert!(reply.body["submessages"].is_null(), "{}", reply.body);
}

#[tokio::test]
async fn a_non_conforming_reply_is_withheld_rather_than_sent() {
    let reply = post(
        &app(NonConforming),
        "/nlip",
        r#"{"format":"text","subformat":"english","content":"hi"}"#,
    )
    .await;

    // The peer asked a valid question and got no answer to it, which is this endpoint
    // failing — `StatusMap::withheld`, like every other way it fails. A peer reading
    // only the status line is the second audience this crate's refusals are for.
    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(reply.body["messagetype"], "Error");
    assert!(reply.text().contains("withheld"), "{}", reply.text());
}

#[tokio::test]
async fn a_withheld_reply_to_a_control_request_is_still_readable_as_an_error() {
    // The two rulings meeting: the replacement keeps `MessageType: Error` and carries
    // control-ness by §6.3's boolean alone, and the status says the endpoint failed.
    // A client can tell this from a success by either half.
    let reply = post(
        &app(NonConforming),
        "/nlip",
        r#"{"control":true,"format":"text","subformat":"english","content":"?"}"#,
    )
    .await;

    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(reply.body["messagetype"], "Error");
    assert_eq!(reply.body["control"], true);
}

#[tokio::test]
async fn a_deployment_can_move_the_withheld_status_off_500() {
    let server = Server::new(
        NonConforming,
        Memory::default(),
        ServerConfig::new().with_status(StatusMap {
            withheld: StatusCode::BAD_GATEWAY,
            ..StatusMap::default()
        }),
    );
    let reply = post(
        &server.router(),
        "/nlip",
        r#"{"format":"text","subformat":"english","content":"hi"}"#,
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_GATEWAY);
}

// ---------------------------------------------------------------------------
// `EndpointError` → status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_failure_reaches_the_peer_at_the_status_the_map_gives_it() {
    let cases: Vec<(EndpointError, StatusCode, Option<&str>)> = vec![
        (
            EndpointError::Unsupported("no timetable endpoint".to_string()),
            StatusCode::NOT_IMPLEMENTED,
            None,
        ),
        (
            EndpointError::Unavailable {
                detail: "the model provider answered HTTP 429".to_string(),
                retry_after: Some(Duration::from_secs(12)),
            },
            StatusCode::SERVICE_UNAVAILABLE,
            Some("12"),
        ),
        (
            EndpointError::Unavailable {
                detail: "no idea when".to_string(),
                retry_after: None,
            },
            StatusCode::SERVICE_UNAVAILABLE,
            None,
        ),
        (
            EndpointError::TimedOut {
                after: Duration::from_secs(30),
            },
            StatusCode::GATEWAY_TIMEOUT,
            None,
        ),
        (
            EndpointError::Internal("connection string postgres://user:hunter2@db".to_string()),
            StatusCode::INTERNAL_SERVER_ERROR,
            None,
        ),
    ];

    for (error, status, retry_after) in cases {
        let reply = post(
            &app(Failing(error.clone())),
            "/nlip",
            r#"{"format":"text","subformat":"english","content":"hi"}"#,
        )
        .await;

        assert_eq!(reply.status, status, "{error}");
        assert_eq!(reply.body["messagetype"], "Error", "{error}");
        match retry_after {
            Some(seconds) => assert_eq!(reply.headers[header::RETRY_AFTER], seconds, "{error}"),
            None => assert!(
                !reply.headers.contains_key(header::RETRY_AFTER),
                "nothing to wait for, so the peer is not invited to: {error}"
            ),
        }
        // ECMA-434 §7.3.3: an internal detail can leak state, so the peer gets none.
        assert!(!reply.text().contains("hunter2"), "{error}");
    }
}

#[tokio::test]
async fn a_refusal_confirms_the_conversation_token_this_endpoint_issued() {
    // §6.2.1's confirmation is owed on every reply the binding builds, refusals
    // included: a client told to send back "whatever came back" would otherwise drop
    // the server's conversation token after one throttled turn.
    let server = Server::new(
        Failing(EndpointError::Unavailable {
            detail: "the model provider is down".to_string(),
            retry_after: None,
        }),
        Memory::in_conversation("srv-1"),
        ServerConfig::new()
            .with_conformity(nlipsy_core::ConformityConfig::new().with_conversation_token(true)),
    );

    let reply = post(
        &server.router(),
        "/nlip",
        r#"{"format":"text","subformat":"english","content":"hi"}"#,
    )
    .await;

    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(reply.body["messagetype"], "Error");
    assert_eq!(
        reply.body["submessages"][0]["content"], "srv-1",
        "{}",
        reply.body
    );
}

#[tokio::test]
async fn a_deployment_can_move_a_status_without_forking_the_pipeline() {
    let server = Server::new(
        Failing(EndpointError::Unsupported("x".to_string())),
        Memory::default(),
        ServerConfig::new().with_status(StatusMap {
            unsupported: StatusCode::BAD_REQUEST,
            ..StatusMap::default()
        }),
    );
    let reply = post(
        &server.router(),
        "/nlip",
        r#"{"format":"text","subformat":"english","content":"hi"}"#,
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// Layers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn what_a_layer_writes_into_the_context_is_what_clause_six_reads() {
    // The property the whole seam exists for: `finish` runs *under* the layers, so
    // §6.2.1's `mint` sees what they wrote. Here the tenant a layer put in the context
    // ends up inside the token the endpoint issues.
    let server = Server::builder(
        Echo,
        Memory::default(),
        ServerConfig::new()
            .with_conformity(nlipsy_core::ConformityConfig::new().with_conversation_token(true)),
    )
    .layer(WithTenant("acme"))
    .build();

    let reply = post(
        &server.router(),
        "/nlip",
        r#"{"format":"text","subformat":"english","content":"hi"}"#,
    )
    .await;

    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.body["submessages"][0]["content"], "conv-for-acme",
        "{}",
        reply.body
    );
}

#[tokio::test]
async fn a_layer_that_refuses_answers_in_the_protocol_and_the_endpoint_never_runs() {
    let server = Server::builder(Echo, Memory::default(), ServerConfig::new())
        .layer(AtCapacity)
        .build();

    let reply = post(
        &server.router(),
        "/nlip",
        r#"{"format":"text","subformat":"english","content":"hi",
            "submessages":[{"format":"token","subformat":"conversation","content":"abc"}]}"#,
    )
    .await;

    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(reply.body["messagetype"], "Error");
    assert!(reply.text().contains("capacity"), "{}", reply.text());
    // And the token is still echoed: the refusal went through the same one loop.
    assert_eq!(reply.body["submessages"][0]["content"], "abc");
}

#[tokio::test]
async fn a_layer_that_answers_without_the_endpoint_still_owes_clause_six() {
    // A layer that returns `Ok` without calling `Next::run` never reaches
    // `Conformity::finish`, so the binding closes clause 6 over whatever came back.
    let server = Server::builder(Echo, Memory::default(), ServerConfig::new())
        .layer(Cached)
        .build();

    let reply = post(
        &server.router(),
        "/nlip",
        r#"{"control":true,"format":"text","subformat":"english","content":"hi",
            "submessages":[{"format":"token","subformat":"conversation","content":"abc"}]}"#,
    )
    .await;

    assert_eq!(reply.status, StatusCode::OK);
    // The layer's own answer, so the endpoint really did not run.
    assert_eq!(reply.text(), "cached");
    // And the peer is owed these whoever answered.
    assert_eq!(reply.body["submessages"][0]["content"], "abc");
    assert_eq!(reply.body["control"], true);
}

#[tokio::test]
async fn a_layer_cannot_strip_the_echo_on_the_way_out() {
    // The other half: a layer's after-half holds the finished reply by value and can
    // take a token out of it. What clause 6 owes the peer is put back.
    let server = Server::builder(Echo, Memory::default(), ServerConfig::new())
        .layer(StripTokens)
        .build();

    let reply = post(
        &server.router(),
        "/nlip",
        r#"{"format":"text","subformat":"english","content":"hi",
            "submessages":[{"format":"token","subformat":"conversation","content":"abc"}]}"#,
    )
    .await;

    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.body["submessages"][0]["content"], "abc",
        "{}",
        reply.body
    );
}

#[tokio::test]
async fn an_endpoint_that_outlasts_a_deadline_layer_is_cut_off_with_504() {
    let server = Server::builder(
        Slow(Duration::from_secs(30)),
        Memory::default(),
        ServerConfig::new(),
    )
    .layer(Deadline(Duration::from_millis(20)))
    .build();

    let reply = post(
        &server.router(),
        "/nlip",
        r#"{"format":"text","subformat":"english","content":"hi",
            "submessages":[{"format":"token","subformat":"conversation","content":"conv-1"}]}"#,
    )
    .await;

    // 504, not 408: the peer sent its request promptly and it is this endpoint that
    // did not finish.
    assert_eq!(reply.status, StatusCode::GATEWAY_TIMEOUT);
    // And the reply is still a well-formed NLIP message carrying the peer's token,
    // which is only possible because the binding still has the request in hand.
    assert_eq!(reply.body["submessages"][0]["content"], "conv-1");
}

// ---------------------------------------------------------------------------
// The correlator
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_correlator_is_reused_when_sane_and_minted_when_not() {
    let app = app(Echo);
    let body = r#"{"format":"text","subformat":"english","content":"hi"}"#;

    let reused = post_with(&app, "/nlip", body, &[(CORRELATOR_HEADER, "trace-me")]).await;
    assert_eq!(reused.headers[CORRELATOR_HEADER], "trace-me");

    let minted = post(&app, "/nlip", body).await;
    assert!(!minted.headers[CORRELATOR_HEADER].is_empty());
    assert_ne!(minted.headers[CORRELATOR_HEADER], "trace-me");

    let absurd = post_with(
        &app,
        "/nlip",
        body,
        &[(CORRELATOR_HEADER, "x".repeat(4096).as_str())],
    )
    .await;
    assert_ne!(absurd.headers[CORRELATOR_HEADER], "x".repeat(4096));
}

#[tokio::test]
async fn with_no_correlator_header_none_is_read_and_none_is_echoed() {
    let server = Server::new(
        Echo,
        Memory::default(),
        ServerConfig::new().with_correlator_header(None),
    );
    let reply = post_with(
        &server.router(),
        "/nlip",
        r#"{"format":"text","subformat":"english","content":"hi"}"#,
        &[(CORRELATOR_HEADER, "trace-me")],
    )
    .await;

    assert_eq!(reply.status, StatusCode::OK);
    assert!(!reply.headers.contains_key(CORRELATOR_HEADER));
}

// ---------------------------------------------------------------------------
// `reject`, for a deployment's own layer
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reject_renders_an_nlip_error_a_layer_can_answer_with() {
    let config = ServerConfig::new();
    let mut response = reject(
        StatusCode::TOO_MANY_REQUESTS,
        "rate limit exceeded; retry later",
        Some(Duration::from_secs(7)),
    );
    config.echo_correlator(&mut response, "c-1");

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    assert_eq!(response.headers()[header::RETRY_AFTER], "7");
    assert_eq!(response.headers()[CORRELATOR_HEADER], "c-1");

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["messagetype"], "Error");
    assert_eq!(body["format"], "text");
    assert_eq!(body["content"], "rate limit exceeded; retry later");
}

#[tokio::test]
async fn a_zero_delay_is_floored_so_a_peer_does_not_spin() {
    let response = reject(
        StatusCode::SERVICE_UNAVAILABLE,
        "later",
        Some(Duration::from_millis(1)),
    );
    assert_eq!(response.headers()[header::RETRY_AFTER], "1");
}

// ---------------------------------------------------------------------------
// The upload leg (ECMA-431 §6.1) — outside clause 6
// ---------------------------------------------------------------------------

/// What a deployment's filesystem sink is, in ten lines: a name this endpoint chose
/// and never the peer's, because a name from the request is a path-traversal vector.
struct TempSink(tempfile::TempDir);

impl UploadSink for TempSink {
    async fn store(&self, upload: Upload<'_>) -> Result<Message, EndpointError> {
        let size = upload.body.len();
        let path = self.0.path().join("upload.bin");
        std::fs::write(&path, &upload.body)
            .map_err(|error| EndpointError::Internal(error.to_string()))?;
        Ok(Message::text(format!("upload received ({size} bytes)")))
    }
}

/// A sink that cannot write, to check a failure reaches the peer opaquely.
struct BrokenSink;

impl UploadSink for BrokenSink {
    async fn store(&self, _upload: Upload<'_>) -> Result<Message, EndpointError> {
        Err(EndpointError::Internal(
            "/var/secrets is read-only".to_string(),
        ))
    }
}

fn upload_app<S: UploadSink>(sink: S) -> Router {
    Server::new(Echo, Memory::default(), ServerConfig::new()).upload_router(sink)
}

#[tokio::test]
async fn the_discarding_sink_acknowledges_a_payload_and_keeps_nothing() {
    for path in ["/nlip/upload", "/nlip/upload/"] {
        let reply = post(&upload_app(DiscardSink), path, "binary-ish bytes").await;
        assert_eq!(reply.status, StatusCode::OK, "{path}");
        assert_eq!(reply.body["format"], "text");
        assert!(reply.text().contains("16 bytes"), "{}", reply.text());
        assert!(reply.text().contains("stores nothing"), "{}", reply.text());
        // Nothing of clause 6 applies here: there was no message to take tokens from.
        assert!(reply.body["submessages"].is_null(), "{}", reply.body);
    }
}

#[tokio::test]
async fn a_storing_sink_gets_the_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("upload.bin");
    // The router owns the sink and the sink owns the directory, so the router has to
    // outlive the assertion — dropping it would take the temporary directory with it.
    let app = upload_app(TempSink(dir));
    let reply = post(&app, "/nlip/upload", "payload").await;

    assert_eq!(reply.status, StatusCode::OK);
    assert!(reply.text().contains("7 bytes"), "{}", reply.text());
    assert_eq!(std::fs::read(&path).unwrap(), b"payload");
    drop(app);
}

#[tokio::test]
async fn a_sink_that_fails_answers_in_the_protocol_without_leaking_why() {
    let reply = post(&upload_app(BrokenSink), "/nlip/upload", "payload").await;

    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(reply.body["messagetype"], "Error");
    assert!(!reply.text().contains("/var/secrets"), "{}", reply.text());
}

#[tokio::test]
async fn the_upload_leg_is_not_served_beside_the_endpoint() {
    // It belongs on a listener of its own (§6.1: "any port different than the port
    // used by NLIP end point"), which is the property that makes an advertised
    // address conforming by construction.
    let app = app(Echo).merge(Router::new().fallback(|| async { StatusCode::NOT_FOUND }));
    let reply = post(&app, "/nlip/upload", "bytes").await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_body_limit_applies_to_the_upload_leg_too() {
    let server = Server::new(
        Echo,
        Memory::default(),
        ServerConfig::new().with_max_request_body_size(8),
    );
    let reply = post(
        &server.upload_router(DiscardSink),
        "/nlip/upload",
        "more than eight bytes",
    )
    .await;
    assert_eq!(reply.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(reply.body["messagetype"], "Error");
}

// ---------------------------------------------------------------------------
// The peer
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_request_with_no_connect_information_is_an_unknown_peer_not_a_failure() {
    // A nested router, or a test that forgot: neither is a reason to refuse.
    let response = app(Echo)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/nlip")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"format":"text","subformat":"english","content":"hi"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(Peer(None).label(), "unknown");
}

// ---------------------------------------------------------------------------
// Policy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_lenient_deployment_can_let_a_clause_five_error_through() {
    // `Rejection` exists so that a bridge relaying from an older implementation can
    // accept what a strict endpoint refuses, without forking the pipeline.
    use nlipsy_core::{Disposition, Policy};

    let lenient = Rejection::new(
        Policy::new(Disposition::Log, Disposition::Log),
        Policy::default(),
    );
    let server = Server::new(
        Echo,
        Memory::default(),
        ServerConfig::new().with_rejection(lenient),
    );

    // A `text` message whose content is not a string: it decodes, and clause 5 calls
    // it an error (§5.1.4).
    let off_spec = r#"{"format":"text","subformat":"english","content":{"not":"a string"}}"#;

    let reply = post(&server.router(), "/nlip", off_spec).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);

    // And the default still refuses the same message.
    let strict = post(&app(Echo), "/nlip", off_spec).await;
    assert_eq!(strict.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_token_this_endpoint_minted_is_not_echoed_back_to_the_peer() {
    // §6.2's exception, end to end: the peer returns what it was issued, and gets it
    // back once — as the conversation's identifier through `issued`, not as an echo.
    let server = Server::new(
        Echo,
        Memory::default(),
        ServerConfig::new()
            .with_conformity(nlipsy_core::ConformityConfig::new().with_conversation_token(true)),
    );
    let app = server.router();

    let first = post(
        &app,
        "/nlip",
        r#"{"format":"text","subformat":"english","content":"hi"}"#,
    )
    .await;
    let issued = first.body["submessages"][0]["content"]
        .as_str()
        .expect("a conversation token should have been issued")
        .to_string();

    let second = post(
        &app,
        "/nlip",
        &format!(
            r#"{{"format":"text","subformat":"english","content":"again",
                 "submessages":[{{"format":"token","subformat":"conversation","content":"{issued}"}}]}}"#
        ),
    )
    .await;

    let tokens = second.body["submessages"].as_array().unwrap();
    assert_eq!(tokens.len(), 1, "once, not twice: {}", second.body);
    assert_eq!(tokens[0]["content"], issued.as_str());
}

/// A `Submessage` is what the echo copies, and the test above asserts on the JSON;
/// this keeps the import honest by naming the type the assertions are about.
#[test]
fn a_token_submessage_is_what_is_echoed() {
    let token = Submessage::token("conversation", "abc");
    assert!(token.is_conversation_token());
}

// ---------------------------------------------------------------------------
// A principal, when a trusted proxy names one
// ---------------------------------------------------------------------------

/// Answers with the principal the exchange carried, or "nobody".
struct WhoAsked;

impl Endpoint for WhoAsked {
    fn name(&self) -> &str {
        "who-asked"
    }

    async fn handle(&self, _request: &Message, cx: &Context) -> Result<Message, EndpointError> {
        Ok(Message::text(
            cx.extensions
                .get::<Principal>()
                .map_or("nobody", Principal::as_str),
        ))
    }
}

fn trusting(header: &'static str) -> Router {
    Server::new(
        WhoAsked,
        Memory::default(),
        ServerConfig::new()
            .with_principal_header(Some(axum::http::HeaderName::from_static(header))),
    )
    .router()
}

const ASK: &str = r#"{"format":"text","subformat":"english","content":"who am I?"}"#;

#[tokio::test]
async fn the_principal_a_trusted_proxy_names_reaches_the_endpoint() {
    let reply = post_with(
        &trusting("x-nlip-principal"),
        "/nlip",
        ASK,
        &[("x-nlip-principal", "human:alice@example.com")],
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.text(), "human:alice@example.com");
}

#[tokio::test]
async fn without_a_named_header_no_header_is_trusted() {
    // The default: a caller that sends the header itself is not believed, because
    // nothing in front of this router was configured to vouch for it.
    let reply = post_with(
        &app(WhoAsked),
        "/nlip",
        ASK,
        &[("x-nlip-principal", "human:alice@example.com")],
    )
    .await;
    assert_eq!(reply.text(), "nobody");
}

#[tokio::test]
async fn a_principal_header_sent_twice_is_believed_neither_time() {
    // A proxy that strips the caller's copy writes exactly one; two are a copy that got
    // past it, and attributing the exchange to either would be a guess.
    let reply = post_with(
        &trusting("x-nlip-principal"),
        "/nlip",
        ASK,
        &[
            ("x-nlip-principal", "human:alice@example.com"),
            ("x-nlip-principal", "machine:mallory"),
        ],
    )
    .await;
    assert_eq!(reply.text(), "nobody");
}

/// A layer that reads the principal, as an authorising layer would.
struct OnlyHumans;

impl Middleware for OnlyHumans {
    async fn call(
        &self,
        request: &Message,
        cx: &mut Context,
        next: Next<'_>,
    ) -> Result<Message, EndpointError> {
        let human = cx
            .extensions
            .get::<Principal>()
            .is_some_and(|principal| principal.as_str().starts_with("human:"));
        if !human {
            return Err(EndpointError::Unsupported(
                "this endpoint answers people only".to_string(),
            ));
        }
        next.run(request, cx).await
    }
}

#[tokio::test]
async fn the_principal_is_in_the_context_before_the_first_layer_runs() {
    let app = Server::builder(
        WhoAsked,
        Memory::default(),
        ServerConfig::new().with_principal_header(Some(axum::http::HeaderName::from_static(
            "x-nlip-principal",
        ))),
    )
    .layer(OnlyHumans)
    .build()
    .router();

    let person = post_with(&app, "/nlip", ASK, &[("x-nlip-principal", "human:alice")]).await;
    assert_eq!(person.text(), "human:alice");
    let machine = post_with(&app, "/nlip", ASK, &[("x-nlip-principal", "machine:ci")]).await;
    assert_eq!(machine.status, StatusCode::NOT_IMPLEMENTED);
}
