use std::{
    io::{BufRead, BufReader, Read},
    ops::ControlFlow,
    sync::mpsc::Sender,
    thread,
    time::Duration,
};

use anyhow::{bail, Result};
use log::{debug, info, warn};
use serde::Deserialize;

/// Wait before the first reconnect; doubles after each failure up to `MAX_BACKOFF`.
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// Two of ntfy's 45 s keepalives without a line means a dead (half-open) connection.
const STALE_TIMEOUT: Duration = Duration::from_secs(90);
/// ntfy messages are at most 4 KiB, so a longer line is not a valid event.
const MAX_LINE_BYTES: u64 = 64 * 1024;

#[derive(Deserialize)]
struct Event {
    event: String,
    time: u64,
}

/// Streams `<url>/json` on a background thread and sends a signal for each
/// message. The thread ends when the receiver is dropped.
pub fn spawn_listener(url: String, signal_tx: Sender<()>) {
    thread::spawn(move || listen(&url, &signal_tx));
}

fn listen(url: &str, signal_tx: &Sender<()>) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_per_read(Some(STALE_TIMEOUT))
        .build()
        .into();
    let stream_url = format!("{}/json", url.trim_end_matches('/'));
    // `since` is inclusive, so a resume may replay one message: one extra pull at most
    let mut since = None;
    let mut backoff = INITIAL_BACKOFF;

    loop {
        match stream(&agent, &stream_url, &mut since, &mut backoff, signal_tx) {
            Ok(ControlFlow::Break(())) => return,
            Ok(ControlFlow::Continue(())) => {
                warn!("ntfy stream closed, reconnecting in {backoff:?}")
            }
            Err(e) => warn!("ntfy stream failed, reconnecting in {backoff:?}: {e:#}"),
        }
        thread::sleep(backoff);
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

fn stream(
    agent: &ureq::Agent,
    stream_url: &str,
    since: &mut Option<u64>,
    backoff: &mut Duration,
    signal_tx: &Sender<()>,
) -> Result<ControlFlow<()>> {
    let mut request = agent.get(stream_url);
    if let Some(time) = since {
        request = request.query("since", time.to_string());
    }
    let reader = BufReader::new(request.call()?.into_body().into_reader());
    read_events(reader, since, backoff, signal_tx)
}

/// Returns `Continue` when the stream ends and `Break` when the receiver is gone.
fn read_events(
    mut reader: impl BufRead,
    since: &mut Option<u64>,
    backoff: &mut Duration,
    signal_tx: &Sender<()>,
) -> Result<ControlFlow<()>> {
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.by_ref().take(MAX_LINE_BYTES).read_line(&mut line)?;
        if read == 0 {
            return Ok(ControlFlow::Continue(()));
        }
        if !line.ends_with('\n') {
            if read as u64 == MAX_LINE_BYTES {
                bail!("ntfy event is longer than {MAX_LINE_BYTES} bytes");
            }
            return Ok(ControlFlow::Continue(()));
        }

        let event: Event = serde_json::from_str(&line)?;
        *since = Some(event.time);
        match event.event.as_str() {
            "open" => {
                info!("Listening for ntfy push signals");
                *backoff = INITIAL_BACKOFF;
            }
            "message" => {
                debug!("ntfy message received");
                if signal_tx.send(()).is_err() {
                    return Ok(ControlFlow::Break(()));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{io::Cursor, sync::mpsc};

    use testresult::TestResult;

    use super::*;

    const STREAM: &str = concat!(
        r#"{"id":"a","time":100,"event":"open","topic":"t"}"#,
        "\n",
        r#"{"id":"b","time":101,"event":"keepalive","topic":"t"}"#,
        "\n",
        r#"{"id":"c","time":102,"event":"message","topic":"t","message":"push refs/heads/main"}"#,
        "\n",
        r#"{"id":"d","time":103,"event":"keepalive","topic":"t"}"#,
        "\n",
    );

    #[test]
    fn test_read_events_signals_only_messages() -> TestResult {
        let (tx, rx) = mpsc::channel();
        let mut since = None;
        let mut backoff = MAX_BACKOFF;

        let flow = read_events(Cursor::new(STREAM), &mut since, &mut backoff, &tx)?;

        assert_eq!(flow, ControlFlow::Continue(()));
        assert_eq!(rx.try_iter().count(), 1);
        assert_eq!(since, Some(103));
        assert_eq!(backoff, INITIAL_BACKOFF, "open resets the backoff");
        Ok(())
    }

    #[test]
    fn test_read_events_stops_without_receiver() -> TestResult {
        let (tx, rx) = mpsc::channel();
        drop(rx);
        let mut since = None;
        let mut backoff = INITIAL_BACKOFF;

        let flow = read_events(Cursor::new(STREAM), &mut since, &mut backoff, &tx)?;

        assert_eq!(flow, ControlFlow::Break(()));
        assert_eq!(since, Some(102));
        Ok(())
    }

    #[test]
    fn test_read_events_rejects_bad_input() {
        let (tx, _rx) = mpsc::channel();
        let mut since = None;
        let mut backoff = INITIAL_BACKOFF;

        let not_json = Cursor::new("not json\n");
        assert!(read_events(not_json, &mut since, &mut backoff, &tx).is_err());

        let too_long = Cursor::new("x".repeat(MAX_LINE_BYTES as usize + 1));
        let err = read_events(too_long, &mut since, &mut backoff, &tx).unwrap_err();
        assert!(err.to_string().contains("longer than"), "{err}");
    }

    #[test]
    fn test_read_events_partial_line_reconnects() -> TestResult {
        let (tx, rx) = mpsc::channel();
        let mut since = None;
        let mut backoff = INITIAL_BACKOFF;

        let partial = Cursor::new(r#"{"id":"c","time":102,"event":"mess"#);
        let flow = read_events(partial, &mut since, &mut backoff, &tx)?;

        assert_eq!(flow, ControlFlow::Continue(()));
        assert_eq!(rx.try_iter().count(), 0);
        assert_eq!(since, None);
        Ok(())
    }
}
