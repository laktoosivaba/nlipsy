//! `export interface client` — an agent calling an end-point.
//!
//! Four free functions and no state. The URL, the wire and the recording are the host's
//! to keep and to pass, which is what makes every one of them free: `nlipsy_core::Client`
//! is built per call over whichever transport the function names, and `Client::send`
//! (`crates/nlipsy-core/src/transport.rs:576`) is the one `await` — encode, carry,
//! decode — that all four share.
//!
//! Which transport is the only thing that differs. `call` and `call-recording` go out
//! over `wasi:http/client@0.3.0` — [`crate::http::Outgoing`], which is the ECMA-431 §6
//! request this binding writes and nothing more; `call-through` goes out over the host's
//! `host-transport` import; `replay` answers from a recording and never leaves the
//! instance at all.

use nlipsy_core::{Client, Recorder, Replay, TransportError};

use crate::Component;
use crate::bindings::exports::nlipsy::engine::client::Guest;
use crate::bindings::nlipsy::engine::types as wt;
use crate::convert::{message_in, reply_out, transport_error_out, wire_in};
use crate::host::{self, HostSink, HostTransport};
use crate::http::Outgoing;

/// What a recording is called in the errors `replay` reports.
///
/// The host has the file name and this function does not — the contract passes the
/// recording's *text*, because the guest has no filesystem — so this is what
/// `ReplayError` and `TransportError` name instead. A caller that needs the path puts
/// it in its own message.
const RECORDING: &str = "the recording";

impl Guest for Component {
    async fn call(
        url: String,
        request: wt::Message,
        wire: wt::Wire,
        cx: wt::Context,
    ) -> Result<wt::Reply, wt::TransportError> {
        let client = Client::new(Outgoing::to(url)).on_wire(wire_in(wire));
        sent(
            client
                .send(&message_in(request), &host::context_in(cx))
                .await,
        )
    }

    async fn call_through(
        request: wt::Message,
        wire: wt::Wire,
        cx: wt::Context,
    ) -> Result<wt::Reply, wt::TransportError> {
        let client = Client::new(HostTransport).on_wire(wire_in(wire));
        sent(
            client
                .send(&message_in(request), &host::context_in(cx))
                .await,
        )
    }

    async fn call_recording(
        url: String,
        request: wt::Message,
        wire: wt::Wire,
        cx: wt::Context,
    ) -> Result<wt::Reply, wt::TransportError> {
        // `Recorder` is what decides *what* is recorded — the request as it went out, the
        // status, the reply body — and `HostSink` is a buffer under it, because
        // `RecordSink::record` is synchronous and `host-record-sink.write` is not. The
        // line crosses after the exchange has returned; a failure to write is a
        // `transport-error.malformed`, exactly as `Recorder` reports a failed `record`.
        //
        // The write happens whether the exchange succeeded or not, because a `Recorder`
        // that failed on the way out recorded nothing and one that succeeded recorded a
        // line that is owed to the sink either way — a recording that quietly stopped
        // growing would produce a fixture that passes for the wrong reason.
        let sink = HostSink::default();
        let client =
            Client::new(Recorder::new(Outgoing::to(url), sink.clone())).on_wire(wire_in(wire));
        let reply = client
            .send(&message_in(request), &host::context_in(cx))
            .await;
        if let Err(error) = host::flush(&sink).await {
            return Err(transport_error_out(&error));
        }
        sent(reply)
    }

    fn replay(
        recording: String,
        index: u32,
        request: wt::Message,
        wire: wt::Wire,
    ) -> Result<wt::Reply, String> {
        let wire = wire_in(wire);

        // Parsed twice, and deliberately. The first parse is over the **whole** text, so
        // that a malformed line is reported with the line number it actually has; the
        // second is over the tail from `index`, which is how a cursor the host keeps is
        // applied to a queue this guest does not hold. `Replay`'s queue is private and
        // there is no skip, which is right — a recording is answered in order — and the
        // guest has no cursor of its own to skip with.
        Replay::from_jsonl(&recording, RECORDING).map_err(|error| error.to_string())?;
        let tail: Vec<&str> = recording
            .lines()
            .filter(|line| !line.trim().is_empty())
            .skip(index as usize)
            .collect();
        let replay = Replay::from_jsonl(&tail.join("\n"), RECORDING)
            .map_err(|error| error.to_string())?
            .on_wire(wire);

        // `Replay::exchange` is synchronous work in an `async` block — "a recording that
        // needed an executor would not be usable from a synchronous test", which is half
        // of why recordings are worth keeping — so `Client::send` over one is ready on
        // its first poll and `now_or_never` is the whole executor it needs. The `None`
        // arm cannot be reached by the transport named here and is reported rather than
        // ignored.
        let client = Client::new(replay).on_wire(wire);
        let cx = nlipsy_core::Context::new();
        let sent = now_or_never(client.send(&message_in(request), &cx)).ok_or_else(|| {
            format!("{RECORDING}: replaying an exchange suspended, which a recording cannot do")
        })?;
        match sent {
            Ok(reply) => Ok(reply_out(&reply)),
            Err(error) => Err(error.to_string()),
        }
    }
}

/// One exchange's answer, engine to contract.
fn sent(sent: Result<nlipsy_core::Reply, TransportError>) -> Result<wt::Reply, wt::TransportError> {
    match sent {
        Ok(reply) => Ok(reply_out(&reply)),
        Err(error) => Err(transport_error_out(&error)),
    }
}

/// Poll a future once, for a path that cannot suspend.
///
/// `std::task::Waker::noop` and one poll: there is no runtime in this guest and none is
/// wanted on the synchronous `replay` export. A `None` answer means the future *did*
/// suspend, which is a fact worth reporting rather than a case to wait out.
fn now_or_never<F: std::future::Future>(future: F) -> Option<F::Output> {
    let mut future = std::pin::pin!(future);
    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);
    match future.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(value) => Some(value),
        std::task::Poll::Pending => None,
    }
}
