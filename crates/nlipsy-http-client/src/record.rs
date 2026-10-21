//! Recorded exchanges on disk: the file half of `nlipsy-core`'s record and replay.
//!
//! The format, the matching and the [`Recorded`] type live in `nlipsy-core`, because
//! they are what makes a recording evidence and none of it needs a filesystem. What is
//! here is the I/O shell: a [`FileSink`] the recorder appends to, and a [`Replay`] that
//! reads one back. It is here rather than there for the reason the whole boundary is
//! drawn: the core has to keep compiling for `wasm32`, where there are no paths, and a
//! browser recording into its own storage implements the same trait over that instead.
//!
//! Why recordings are kept at all, from the module this grew out of: a test that builds
//! its request with the client's own encoder and asserts against a reply the same
//! decoder produced proves only that the two agree — and they would agree just as
//! readily if both misread the same clause. A recording is evidence from outside that
//! circle: bytes that a live end-point actually sent.

use std::path::{Path, PathBuf};

use nlipsy_core::{
    Context, Exchange, RecordError, RecordSink, Recorded, ReplayError, Transport, TransportError,
};

/// Appends every recorded exchange to a file, one JSON object per line.
#[derive(Debug)]
pub struct FileSink {
    path: PathBuf,
}

impl FileSink {
    /// Start recording to `path`, replacing whatever was there.
    pub fn create(path: impl Into<PathBuf>) -> Result<FileSink, ReplayError> {
        let path = path.into();
        // `filter` rather than the `let … && …` chain the server's copy of this used:
        // let chains are stable from 1.88 and this workspace's floor is 1.85, which is
        // checked (`cargo +1.85 check --workspace --all-targets --all-features`).
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty());
        if let Some(parent) = parent {
            std::fs::create_dir_all(parent).map_err(|source| ReplayError::Write {
                path: path.display().to_string(),
                source,
            })?;
        }
        // Created empty rather than appended to: a recording that silently grew across
        // runs would replay exchanges from an earlier one first.
        std::fs::write(&path, "").map_err(|source| ReplayError::Write {
            path: path.display().to_string(),
            source,
        })?;
        Ok(FileSink { path })
    }

    /// Where the recording is being written.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl RecordSink for FileSink {
    fn record(&self, exchange: &Recorded) -> Result<(), RecordError> {
        // A failure to record is reported, not swallowed: a recording that quietly
        // stopped growing would produce a fixture that passes for the wrong reason.
        let line = exchange.to_json_line();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|source| {
                RecordError::new(format!("cannot open {}: {source}", self.path.display()))
            })?;
        use std::io::Write;
        writeln!(file, "{line}").map_err(|source| {
            RecordError::new(format!("cannot write {}: {source}", self.path.display()))
        })
    }
}

/// A recording read from a file, replayed through `nlipsy-core`'s matcher.
///
/// A newtype rather than a re-export: reading the file is the one part of replay that
/// is I/O, and the core has none by design. Everything else — the queue, the request
/// matching, the verbatim reply bytes — is the core's.
#[derive(Debug)]
pub struct Replay(nlipsy_core::Replay);

impl Replay {
    /// Read a recording from `path`.
    pub fn load(path: impl AsRef<Path>) -> Result<Replay, ReplayError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| ReplayError::Read {
            path: path.display().to_string(),
            source,
        })?;
        Ok(Replay(nlipsy_core::Replay::from_jsonl(
            &text,
            path.display().to_string(),
        )?))
    }

    /// Replay records a caller already has.
    pub fn from_records(records: Vec<Recorded>, source: impl Into<String>) -> Replay {
        Replay(nlipsy_core::Replay::from_records(records, source))
    }

    /// How many exchanges are still unanswered.
    pub fn remaining(&self) -> usize {
        self.0.remaining()
    }
}

impl Transport for Replay {
    async fn exchange(&self, cx: &Context, body: Vec<u8>) -> Result<Exchange, TransportError> {
        self.0.exchange(cx, body).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nlipsy_core::{Client, ExchangeDetail};
    use nlipsy_protocol::message::Message;

    fn record(request: &str, status: u16, body: &str) -> Recorded {
        Recorded {
            request: request.to_string(),
            status,
            retry_after_secs: None,
            body: body.to_string(),
            lossy: false,
        }
    }

    #[tokio::test]
    async fn a_recording_round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recording.jsonl");
        let sink = FileSink::create(&path).unwrap();

        sink.record(&record("{\"a\":1}", 200, "{\"b\":2}")).unwrap();
        sink.record(&Recorded {
            request: "{\"c\":3}".to_string(),
            status: 429,
            retry_after_secs: Some(7),
            body: "slow down".to_string(),
            lossy: false,
        })
        .unwrap();

        let loaded = Replay::load(&path).unwrap();
        assert_eq!(loaded.remaining(), 2);
        // And it reads as one exchange per line, which is what makes a recording
        // reviewable in a diff.
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(text.contains("\"retry_after_secs\":7"), "{text}");
    }

    #[tokio::test]
    async fn a_recorded_exchange_replays_through_the_real_client() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recording.jsonl");
        let request = Message::text("What is Ecma?");

        let sink = FileSink::create(&path).unwrap();
        sink.record(&record(
            &request.to_json_string(),
            200,
            r#"{"format":"text","subformat":"english","content":"A standards body."}"#,
        ))
        .unwrap();

        let client = Client::new(Replay::load(&path).unwrap());
        let reply = client.send(&request, &Context::new()).await.unwrap();
        assert_eq!(reply.text().as_deref(), Some("A standards body."));
        assert!(matches!(
            reply.detail,
            ExchangeDetail::Http { status: 200, .. }
        ));
    }

    #[test]
    fn a_recording_is_truncated_rather_than_appended_to() {
        // A recording that silently grew across runs would replay exchanges from an
        // earlier one first, which is a fixture that passes for the wrong reason.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recording.jsonl");

        FileSink::create(&path)
            .unwrap()
            .record(&record("{}", 200, "old"))
            .unwrap();
        let _fresh = FileSink::create(&path).unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
    }
}
