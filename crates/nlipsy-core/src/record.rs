//! Recorded exchanges: the wire as it really was, kept so tests need no network.
//!
//! A test that builds its request with this crate's own encoder and asserts against a
//! reply this crate's own decoder produced proves only that the two agree — and they
//! would agree just as readily if both misread the same clause. A recording is
//! evidence from outside that circle: bytes that a live endpoint actually sent.
//!
//! Two consequences shape the format:
//!
//! * **The body is kept as text, not as a decoded [`Message`].** Storing the parsed
//!   form would re-encode it through the same code under test on the way back out,
//!   which is exactly the circularity the recording exists to break.
//! * **The request is kept too, and matched on replay.** A recording of replies alone
//!   would not notice the request changing shape underneath it — a client that started
//!   omitting the conversation token would still replay happily.
//!
//! The format is one JSON object per line, so a recording reads in a diff the way it
//! reads in a test.
//!
//! # Where the bytes go
//!
//! Nothing here touches a file. [`Recorder`] writes to a [`RecordSink`], which a
//! deployment implements over a file, a buffer or a test double, because file I/O is a
//! runtime concern and this crate has to keep compiling for `wasm32`. [`Replay`] is
//! given records that somebody else read.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::Mutex;

use nlipsy_protocol::content::Wire;
use nlipsy_protocol::message::Message;
use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeMap};
use serde_json::Value;
use thiserror::Error;

use crate::context::Context;
use crate::send::MaybeSend;
use crate::transport::{Exchange, ExchangeDetail, Transport, TransportError};

/// One recorded exchange.
///
/// The body is text. Everything an ECMA-431 endpoint produces is JSON, so a non-UTF-8
/// body would mean something other than the endpoint answered — a proxy, a load
/// balancer — and recording its lossy rendering is preferable to refusing to record at
/// all. [`Recorded::lossy`] says so rather than pretending the bytes are exact.
///
/// The fields are ECMA-431's, and the serialised spelling is a compatibility promise:
/// recordings on disk are evidence taken from live endpoints, and a field renamed here
/// invalidates them. A frame-based binding with no status will need its own record
/// type rather than a fourth optional field on this one.
///
/// Deliberately *not* `#[non_exhaustive]`, unlike most types in this workspace. The
/// field list is the file format, so adding a field is a format change whether or not
/// the type admits one — and whoever reads a recording out of wherever they keep it has
/// to be able to build these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    /// The request body, exactly as it was sent.
    pub request: String,
    /// The HTTP status the endpoint answered with.
    pub status: u16,
    /// `Retry-After`, in seconds, when the endpoint named one.
    pub retry_after_secs: Option<u64>,
    /// The reply body.
    pub body: String,
    /// Set when `body` is a lossy rendering rather than the bytes themselves.
    pub lossy: bool,
}

impl Recorded {
    /// A record of `exchange`, answering the request in `request`.
    pub fn of(request: &[u8], exchange: &Exchange) -> Recorded {
        let (status, retry_after_secs) = match &exchange.detail {
            ExchangeDetail::Http {
                status,
                retry_after,
            } => (*status, retry_after.map(|delay| delay.as_secs())),
        };
        Recorded {
            request: String::from_utf8_lossy(request).into_owned(),
            status,
            retry_after_secs,
            lossy: std::str::from_utf8(&exchange.body).is_err(),
            body: String::from_utf8_lossy(&exchange.body).into_owned(),
        }
    }

    /// The exchange this record stands for. The reply bytes go back verbatim.
    pub fn to_exchange(&self) -> Exchange {
        Exchange {
            body: self.body.clone().into_bytes(),
            detail: ExchangeDetail::Http {
                status: self.status,
                retry_after: self.retry_after_secs.map(std::time::Duration::from_secs),
            },
        }
    }

    /// The record as one line of a recording, in the field order used on disk.
    ///
    /// Here rather than in the sink so that a sink needs no JSON codec of its own:
    /// the spelling of a record is this crate's promise, and a second encoder of it
    /// would be a second thing to keep in step.
    ///
    /// # Panics
    ///
    /// Never: every field is a string, a number or a boolean, and JSON has all three.
    pub fn to_json_line(&self) -> String {
        serde_json::to_string(&self.to_value())
            .expect("a record is strings, numbers and booleans, which JSON can hold")
    }

    /// The record as a JSON value, in the field order recordings on disk use.
    pub fn to_value(&self) -> Value {
        let mut map = serde_json::Map::new();
        map.insert("request".to_string(), Value::from(self.request.clone()));
        map.insert("status".to_string(), Value::from(self.status));
        if let Some(seconds) = self.retry_after_secs {
            map.insert("retry_after_secs".to_string(), Value::from(seconds));
        }
        map.insert("body".to_string(), Value::from(self.body.clone()));
        if self.lossy {
            map.insert("lossy".to_string(), Value::from(true));
        }
        Value::Object(map)
    }

    /// Read a record back from a JSON value.
    ///
    /// The two optional fields default, as the recordings that omit them expect; an
    /// unknown field is refused rather than ignored, because a recording this crate
    /// cannot fully read is a fixture that would pass for the wrong reason.
    pub fn from_value(value: &Value) -> Result<Recorded, String> {
        let object = value
            .as_object()
            .ok_or_else(|| "a recorded exchange is a JSON object".to_string())?;
        for key in object.keys() {
            if !matches!(
                key.as_str(),
                "request" | "status" | "retry_after_secs" | "body" | "lossy"
            ) {
                return Err(format!("unknown field `{key}` in a recorded exchange"));
            }
        }

        let text = |field: &str| -> Result<String, String> {
            object
                .get(field)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("`{field}` must be a string"))
        };

        let status = object
            .get("status")
            .and_then(Value::as_u64)
            .and_then(|status| u16::try_from(status).ok())
            .ok_or_else(|| "`status` must be a status code".to_string())?;

        let retry_after_secs = match object.get("retry_after_secs") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_u64()
                    .ok_or_else(|| "`retry_after_secs` must be a whole number".to_string())?,
            ),
        };

        let lossy = match object.get("lossy") {
            None => false,
            Some(value) => value
                .as_bool()
                .ok_or_else(|| "`lossy` must be a boolean".to_string())?,
        };

        Ok(Recorded {
            request: text("request")?,
            status,
            retry_after_secs,
            body: text("body")?,
            lossy,
        })
    }
}

// Written by hand rather than derived, as `nlipsy-protocol` writes its own: the field
// order of a recording is part of its format, a derive would not let the two optional
// fields be skipped and defaulted in the same breath, and `serde_derive` is a
// proc-macro that a consumer wanting only an endpoint should not have to compile.
impl Serialize for Recorded {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let fields = 3 + usize::from(self.retry_after_secs.is_some()) + usize::from(self.lossy);
        let mut map = serializer.serialize_map(Some(fields))?;
        map.serialize_entry("request", &self.request)?;
        map.serialize_entry("status", &self.status)?;
        if let Some(seconds) = self.retry_after_secs {
            map.serialize_entry("retry_after_secs", &seconds)?;
        }
        map.serialize_entry("body", &self.body)?;
        if self.lossy {
            map.serialize_entry("lossy", &true)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Recorded {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Recorded, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Recorded::from_value(&value).map_err(serde::de::Error::custom)
    }
}

/// Why a recording could not be read or written.
///
/// `Read` and `Write` name a path because a recording is a file wherever one exists —
/// the sink that produces them lives outside this crate, but the vocabulary is one
/// type so that a caller matches in one place.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ReplayError {
    /// The recording could not be read.
    #[error("cannot read the recording at {path}: {source}")]
    Read {
        /// Where it was.
        path: String,
        /// What the reader said.
        #[source]
        source: std::io::Error,
    },

    /// The recording could not be written.
    #[error("cannot write the recording at {path}: {source}")]
    Write {
        /// Where it was going.
        path: String,
        /// What the writer said.
        #[source]
        source: std::io::Error,
    },

    /// One line of the recording is not a recorded exchange.
    #[error("{path}: line {line} is not a recorded exchange: {source}")]
    Line {
        /// Where the recording was.
        path: String,
        /// Which line, counting from one.
        line: usize,
        /// What the decoder said.
        #[source]
        source: serde_json::Error,
    },
}

/// Why one exchange could not be recorded.
///
/// A string rather than a boxed error: a sink may be a file, a socket or a browser's
/// storage, and the one thing a caller does with this is put it in a message. A
/// failure to record is reported and never swallowed — a recording that quietly
/// stopped growing would produce a fixture that passes for the wrong reason.
#[derive(Debug, Clone, Error)]
#[error("the exchange could not be recorded: {detail}")]
pub struct RecordError {
    detail: String,
}

impl RecordError {
    /// A failure, described.
    pub fn new(detail: impl Into<String>) -> RecordError {
        RecordError {
            detail: detail.into(),
        }
    }

    /// What the sink said.
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

/// Where a [`Recorder`] puts what it saw.
///
/// A trait rather than a path, because appending to a file is I/O and this crate has
/// none. `&self` rather than `&mut self` for the same reason a transport takes `&self`:
/// a recorder is shared behind the client that holds it.
pub trait RecordSink: MaybeSend + Sync {
    /// Append one exchange.
    fn record(&self, exchange: &Recorded) -> Result<(), RecordError>;
}

/// Wraps a transport and hands everything that passes through it to a sink.
#[derive(Debug, Clone)]
pub struct Recorder<T, S> {
    inner: T,
    sink: S,
}

impl<T, S> Recorder<T, S> {
    /// Record everything `inner` carries into `sink`.
    pub fn new(inner: T, sink: S) -> Recorder<T, S> {
        Recorder { inner, sink }
    }

    /// The transport underneath.
    pub fn inner(&self) -> &T {
        &self.inner
    }

    /// The sink being written to.
    pub fn sink(&self) -> &S {
        &self.sink
    }
}

impl<T: Transport, S: RecordSink> Transport for Recorder<T, S> {
    async fn exchange(&self, cx: &Context, body: Vec<u8>) -> Result<Exchange, TransportError> {
        let exchange = self.inner.exchange(cx, body.clone()).await?;
        let record = Recorded::of(&body, &exchange);
        self.sink
            .record(&record)
            .map_err(|error| TransportError::Malformed {
                detail: error.to_string(),
            })?;
        Ok(exchange)
    }
}

/// Answers from a recording, in order, and refuses a request the recording does not
/// contain.
///
/// Refusing rather than skipping is the point. If the client's encoder changed, the
/// next recorded exchange belongs to a request that is no longer being sent, and
/// quietly returning the next reply anyway would let the change pass.
///
/// The match is on the *decoded* request, not on the bytes. Two encoders can spell one
/// message differently — CBOR (ECMA-432 §7.1) especially — and a recording is evidence
/// about the message, not about one encoder's key order. A request that does not
/// decode at all is compared verbatim instead, because two bodies that are not
/// messages can still be the same bytes.
#[derive(Debug)]
pub struct Replay {
    queue: Mutex<VecDeque<Recorded>>,
    source: String,
    wire: Wire,
}

impl Replay {
    /// Replay `records`, naming `source` in any failure.
    pub fn from_records(records: Vec<Recorded>, source: impl Into<String>) -> Replay {
        Replay {
            queue: Mutex::new(records.into()),
            source: source.into(),
            wire: Wire::Json,
        }
    }

    /// Replay one JSON object per line.
    ///
    /// The text is whatever a caller read — from a file, from an embedded fixture,
    /// from a browser's storage. `source` names it in any failure.
    pub fn from_jsonl(text: &str, source: impl Into<String>) -> Result<Replay, ReplayError> {
        let source = source.into();
        let mut records = Vec::new();
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let record = serde_json::from_str(line).map_err(|source_error| ReplayError::Line {
                path: source.clone(),
                line: index + 1,
                source: source_error,
            })?;
            records.push(record);
        }
        Ok(Replay::from_records(records, source))
    }

    /// Compare requests as messages on this wire instead of on [`Wire::Json`].
    pub fn on_wire(mut self, wire: Wire) -> Replay {
        self.wire = wire;
        self
    }

    /// How many exchanges are still unanswered.
    pub fn remaining(&self) -> usize {
        self.queue.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).len()
    }
}

/// `true` when two request bodies are the same message — or, when neither is a
/// message, the same bytes.
fn same_request(recorded: &str, sent: &[u8], wire: Wire) -> bool {
    let decode = |bytes: &[u8]| -> Option<Message> {
        match wire {
            Wire::Json => Message::from_json_bytes(bytes).ok(),
            #[cfg(feature = "cbor")]
            Wire::Cbor => Message::from_cbor_slice(bytes).ok(),
            #[cfg(not(feature = "cbor"))]
            Wire::Cbor => None,
        }
    };
    match (decode(recorded.as_bytes()), decode(sent)) {
        (Some(recorded), Some(sent)) => recorded == sent,
        _ => recorded.as_bytes() == sent,
    }
}

impl Transport for Replay {
    fn exchange(
        &self,
        _cx: &Context,
        body: Vec<u8>,
    ) -> impl Future<Output = Result<Exchange, TransportError>> + MaybeSend {
        // Synchronous work in an `async` block: there is nothing to wait for, and a
        // recording that needed an executor would not be usable from a synchronous
        // test — which is half of why recordings are worth keeping.
        let result = self.answer(&body);
        async move { result }
    }
}

impl Replay {
    fn answer(&self, body: &[u8]) -> Result<Exchange, TransportError> {
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let Some(next) = queue.front() else {
            return Err(TransportError::Malformed {
                detail: format!("{}: the recording has no more exchanges", self.source),
            });
        };

        if !same_request(&next.request, body, self.wire) {
            let sent = String::from_utf8_lossy(body);
            return Err(TransportError::Malformed {
                detail: format!(
                    "{}: the next recorded exchange is for a different request.\n\
                     recorded: {}\n\
                     sent:     {sent}",
                    self.source, next.request
                ),
            });
        }

        let record = queue.pop_front().expect("just checked");
        Ok(record.to_exchange())
    }
}

#[cfg(all(test, feature = "testing"))]
mod tests {
    use super::*;
    use crate::transport::Client;
    use nlipsy_protocol::message::Submessage;

    fn record(request: &str, status: u16, body: &str) -> Recorded {
        Recorded {
            request: request.to_string(),
            status,
            retry_after_secs: None,
            body: body.to_string(),
            lossy: false,
        }
    }

    fn jsonl(records: &[Recorded]) -> String {
        records
            .iter()
            .map(|record| serde_json::to_string(record).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn a_recording_replays_through_the_real_client() {
        // The whole path on both sides: the client encodes, the recording answers, the
        // client decodes. The exchange is the canonical one from TR/113 §11.1.
        let request = Message::text("What is Ecma?")
            .with_submessage(Submessage::token("conversation", "c1"));
        let sent = request.to_json_string();

        let recorded = record(
            &sent,
            200,
            r#"{"format":"text","subformat":"english","content":"Ecma is a standards body."}"#,
        );
        let client = Client::new(Replay::from_records(vec![recorded], "test"));

        let reply = client.send(&request, &Context::new()).await.unwrap();
        assert_eq!(reply.text().as_deref(), Some("Ecma is a standards body."));
    }

    #[tokio::test]
    async fn a_request_the_recording_does_not_contain_is_refused() {
        // The property that makes a recording worth keeping: if the request changes
        // shape, the test fails instead of quietly replaying an answer to a question
        // that is no longer being asked.
        let recorded = record(
            r#"{"format":"text","subformat":"english","content":"recorded"}"#,
            200,
            r#"{"format":"text","subformat":"english","content":"answer"}"#,
        );
        let client = Client::new(Replay::from_records(vec![recorded], "test"));

        let different = Message::text("something else");
        let error = client
            .send(&different, &Context::new())
            .await
            .unwrap_err();

        let message = error.to_string();
        assert!(message.contains("recorded"), "{message}");
        assert!(message.contains("something else"), "{message}");
    }

    #[tokio::test]
    async fn the_same_message_spelled_differently_still_replays() {
        // Matching on the decoded message rather than on the bytes: the recording is
        // evidence about the message, not about one encoder's key order. Without this
        // a CBOR binding could never replay a recording at all, since two encoders
        // produce different bytes for the same message.
        let reordered = r#"{"content":"hi","subformat":"english","format":"text"}"#;
        let recorded = record(
            reordered,
            200,
            r#"{"format":"text","subformat":"english","content":"answer"}"#,
        );
        let client = Client::new(Replay::from_records(vec![recorded], "test"));

        let reply = client
            .send(&Message::text("hi"), &Context::new())
            .await
            .unwrap();
        assert_eq!(reply.text().as_deref(), Some("answer"));
    }

    #[tokio::test]
    async fn running_out_of_recorded_exchanges_is_an_error_not_a_hang() {
        let client = Client::new(Replay::from_records(Vec::new(), "test"));
        let error = client
            .send(&Message::text("anything"), &Context::new())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("no more exchanges"));
    }

    #[tokio::test]
    async fn exchanges_are_answered_in_the_order_they_were_recorded() {
        let first = Message::text("one");
        let second = Message::text("two");
        let replay = Replay::from_records(
            vec![
                record(
                    &first.to_json_string(),
                    200,
                    r#"{"format":"text","subformat":"english","content":"first"}"#,
                ),
                record(
                    &second.to_json_string(),
                    200,
                    r#"{"format":"text","subformat":"english","content":"second"}"#,
                ),
            ],
            "test",
        );
        assert_eq!(replay.remaining(), 2);
        let client = Client::new(replay);

        let cx = Context::new();
        assert_eq!(
            client.send(&first, &cx).await.unwrap().text().as_deref(),
            Some("first")
        );
        assert_eq!(
            client.send(&second, &cx).await.unwrap().text().as_deref(),
            Some("second")
        );
    }

    #[test]
    fn a_recording_round_trips_through_its_jsonl_form() {
        let records = vec![
            record("{\"a\":1}", 200, "{\"b\":2}"),
            Recorded {
                request: "{\"c\":3}".to_string(),
                status: 429,
                retry_after_secs: Some(7),
                body: "slow down".to_string(),
                lossy: false,
            },
        ];

        let text = jsonl(&records);
        // One exchange per line, which is what makes a recording reviewable in a diff.
        assert_eq!(text.lines().count(), 2);

        let loaded = Replay::from_jsonl(&text, "test").unwrap();
        assert_eq!(loaded.remaining(), 2);
    }

    #[test]
    fn a_recorded_429_keeps_its_retry_after() {
        let recorded = Recorded {
            request: "{}".to_string(),
            status: 429,
            retry_after_secs: Some(7),
            body: "slow down".to_string(),
            lossy: false,
        };

        let line = serde_json::to_string(&recorded).unwrap();
        // The delay survives the round trip, since it is the whole reason a peer pauses
        // rather than retrying blindly.
        assert!(line.contains("\"retry_after_secs\":7"), "{line}");
        assert_eq!(
            serde_json::from_str::<Recorded>(&line).unwrap(),
            recorded
        );
        assert_eq!(recorded.to_exchange().detail.retry_after(), Some(std::time::Duration::from_secs(7)));
    }

    #[test]
    fn the_recorded_field_order_is_the_one_on_disk() {
        // Recordings are evidence taken from live endpoints and are read in diffs.
        // A derive would spell them in declaration order; this is written by hand, so
        // the order is asserted rather than assumed.
        let line = serde_json::to_string(&record("req", 200, "body")).unwrap();
        assert_eq!(line, r#"{"request":"req","status":200,"body":"body"}"#);

        let lossy = Recorded {
            request: "req".to_string(),
            status: 429,
            retry_after_secs: Some(1),
            body: "…".to_string(),
            lossy: true,
        };
        assert_eq!(
            serde_json::to_string(&lossy).unwrap(),
            r#"{"request":"req","status":429,"retry_after_secs":1,"body":"…","lossy":true}"#
        );
    }

    #[test]
    fn a_line_that_is_not_a_recorded_exchange_names_its_number() {
        let text = format!("{}\nnot json\n", serde_json::to_string(&record("a", 200, "b")).unwrap());
        let error = Replay::from_jsonl(&text, "fixture.jsonl").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("line 2"), "{message}");
        assert!(message.contains("fixture.jsonl"), "{message}");
    }

    #[test]
    fn a_field_the_format_does_not_have_is_refused_rather_than_ignored() {
        // A recording this crate cannot fully read is a fixture that would pass for
        // the wrong reason.
        let error = Replay::from_jsonl(
            r#"{"request":"a","status":200,"body":"b","frame_kind":"binary"}"#,
            "fixture.jsonl",
        )
        .unwrap_err();
        assert!(error.to_string().contains("frame_kind"), "{error}");
    }

    #[tokio::test]
    async fn a_recorder_hands_every_exchange_to_its_sink() {
        use crate::testing::{Scripted, ok};

        #[derive(Default)]
        struct Collect(Mutex<Vec<Recorded>>);

        impl RecordSink for Collect {
            fn record(&self, exchange: &Recorded) -> Result<(), RecordError> {
                self.0.lock().unwrap().push(exchange.clone());
                Ok(())
            }
        }

        let scripted = Scripted::new(vec![ok(
            200,
            r#"{"format":"text","subformat":"english","content":"answer"}"#,
        )]);
        let recorder = Recorder::new(scripted, Collect::default());

        // Driven as a transport rather than through a `Client`, so the sink can be
        // read back afterwards — which is what `Recorder::sink` is for.
        let exchange = recorder
            .exchange(&Context::new(), Message::text("hi").to_json_vec())
            .await
            .unwrap();
        assert_eq!(exchange.detail.status(), Some(200));

        let seen = recorder.sink().0.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert!(seen[0].request.contains("hi"), "{}", seen[0].request);
        assert_eq!(seen[0].status, 200);
        assert!(seen[0].body.contains("answer"));
        assert!(!seen[0].lossy);
    }

    #[tokio::test]
    async fn a_sink_that_fails_fails_the_exchange() {
        use crate::testing::{Scripted, ok};

        struct Refuses;

        impl RecordSink for Refuses {
            fn record(&self, _exchange: &Recorded) -> Result<(), RecordError> {
                Err(RecordError::new("the disk is full"))
            }
        }

        // A failure to record is reported, not swallowed: a recording that quietly
        // stopped growing would produce a fixture that passes for the wrong reason.
        let recorder = Recorder::new(
            Scripted::new(vec![ok(200, r#"{"format":"text","subformat":"english","content":"x"}"#)]),
            Refuses,
        );
        let error = Client::new(recorder)
            .send(&Message::text("hi"), &Context::new())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("the disk is full"), "{error}");
    }
}
