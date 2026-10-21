//! A transport that answers from a script.
//!
//! Behind a feature rather than `#[cfg(test)]`, because the crates that need it are
//! somewhere else: a federating endpoint's tests need a peer that answers without a
//! socket, and a terminal client's state machine needs a wire it can predict. A
//! release build of either compiles no part of this.

use std::future::Future;
use std::sync::Mutex;

use serde_json::Value;

use crate::context::Context;
use crate::send::MaybeSend;
use crate::transport::{Exchange, ExchangeDetail, Transport, TransportError};

/// A transport that answers from a script, and records what it was asked.
#[derive(Debug)]
pub struct Scripted {
    replies: Mutex<Vec<Result<Exchange, TransportError>>>,
    /// Every request body, in the order it was sent.
    pub sent: Mutex<Vec<Vec<u8>>>,
}

impl Scripted {
    /// Answer with `replies`, in the order given.
    pub fn new(replies: Vec<Result<Exchange, TransportError>>) -> Scripted {
        Scripted {
            // Reversed so `pop` yields them in the order given.
            replies: Mutex::new(replies.into_iter().rev().collect()),
            sent: Mutex::new(Vec::new()),
        }
    }

    /// Everything that was sent, decoded as JSON.
    ///
    /// # Panics
    ///
    /// When a request was not JSON. A test that sent something else wants to know.
    pub fn sent_json(&self) -> Vec<Value> {
        self.sent
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|body| serde_json::from_slice(body).expect("a request is JSON"))
            .collect()
    }
}

impl Transport for Scripted {
    fn exchange(
        &self,
        _cx: &Context,
        body: Vec<u8>,
    ) -> impl Future<Output = Result<Exchange, TransportError>> + MaybeSend {
        self.sent
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(body);
        let reply = self
            .replies
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop()
            .unwrap_or_else(|| {
                Err(TransportError::Malformed {
                    detail: "the script ran out of replies".to_string(),
                })
            });
        async move { reply }
    }
}

/// An exchange carrying an NLIP message over HTTP.
pub fn ok(status: u16, body: &str) -> Result<Exchange, TransportError> {
    Ok(Exchange {
        body: body.as_bytes().to_vec(),
        detail: ExchangeDetail::Http {
            status,
            retry_after: None,
        },
    })
}
