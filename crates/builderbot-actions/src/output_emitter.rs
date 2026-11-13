//! Batched delivery of one execution's output.
//!
//! A process writes in small pieces, and a progress bar redraws many times a
//! second. Handing every read to the listener costs it an event each, whether
//! or not anyone is watching. Instead the reader threads queue their reads for
//! one emitter thread per execution, which merges them and delivers a batch
//! once it is [`EMIT_INTERVAL`] old or [`EMIT_MAX_BYTES`] large.
//!
//! A batch reaches the execution's buffer and then its listener, in that
//! order, so the buffer never holds output the listener has yet to be told
//! about. A viewer that reads the buffer and then follows the events sees no
//! batch twice.

use std::convert::Infallible;
use std::sync::mpsc::{self, RecvTimeoutError, TrySendError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use tokio::runtime::Handle;

use crate::executor::ExecutionListener;
use crate::models::ExecutionEvent;
use crate::output_buffer::SharedOutputBuffer;

/// How long output waits to be merged with what follows it.
pub(crate) const EMIT_INTERVAL: Duration = Duration::from_millis(50);

/// A batch this large is delivered without waiting out the interval.
pub(crate) const EMIT_MAX_BYTES: usize = 64 * 1024;

/// How many reads may be queued for the emitter before a reader blocks. The
/// listener is called synchronously, so this is what stops a slow listener
/// from building an unbounded queue: the reader stops reading, and the
/// process blocks on its output, as it did when each read was delivered
/// directly.
const QUEUED_READS: usize = 64;

/// How often a flush that found the queue full tries again.
const FLUSH_RETRY_INTERVAL: Duration = Duration::from_millis(1);

enum Message {
    Output {
        stream: &'static str,
        text: String,
        timestamp: i64,
    },
    /// Deliver what is pending now, then drop the sender.
    Flush(mpsc::Sender<Infallible>),
}

/// Where the reader threads queue an execution's output. The emitter thread
/// ends once every clone has been dropped and the queue is empty.
#[derive(Clone)]
pub(crate) struct OutputSender(mpsc::SyncSender<Message>);

impl OutputSender {
    /// Queue one read of `stream`. Blocks while the emitter is behind.
    pub(crate) fn send(&self, stream: &'static str, text: String, timestamp: i64) {
        if text.is_empty() {
            return;
        }
        // Fails only if the emitter thread is gone, which leaves nowhere to
        // deliver to.
        let _ = self.0.send(Message::Output {
            stream,
            text,
            timestamp,
        });
    }

    /// Deliver everything queued so far without waiting out the interval.
    /// Returns whether the listener had it all by `deadline`.
    ///
    /// Unlike a read, the request does not wait indefinitely for room in the
    /// queue: a caller with a deadline must not be held up by a listener that
    /// has fallen behind.
    pub(crate) fn flush_by(&self, deadline: Instant) -> bool {
        let (delivered, wait) = mpsc::channel::<Infallible>();
        let mut request = Message::Flush(delivered);
        loop {
            match self.0.try_send(request) {
                Ok(()) => break,
                // The emitter thread has ended, with nothing left to deliver.
                Err(TrySendError::Disconnected(_)) => return true,
                Err(TrySendError::Full(unsent)) => {
                    if Instant::now() >= deadline {
                        return false;
                    }
                    request = unsent;
                    thread::sleep(FLUSH_RETRY_INTERVAL);
                }
            }
        }
        // Nothing is ever sent: the emitter drops its end once it is done.
        matches!(
            wait.recv_timeout(deadline.saturating_duration_since(Instant::now())),
            Err(RecvTimeoutError::Disconnected)
        )
    }
}

/// Consecutive output of one stream, waiting to be delivered.
struct Segment {
    stream: &'static str,
    text: String,
    /// When the first read in the segment arrived.
    timestamp: i64,
}

/// Start the emitter thread for one execution and return the queue that
/// feeds it.
pub(crate) fn spawn(
    execution_id: String,
    listener: Arc<dyn ExecutionListener>,
    buffer: SharedOutputBuffer,
    handle: Handle,
    interval: Duration,
    max_bytes: usize,
) -> OutputSender {
    let (sender, receiver) = mpsc::sync_channel(QUEUED_READS);

    thread::spawn(move || {
        let mut pending: Vec<Segment> = Vec::new();
        let mut pending_bytes = 0;
        // When the oldest pending read is due. Set while anything is pending.
        let mut due: Option<Instant> = None;

        loop {
            let message = match due {
                Some(due) => receiver.recv_timeout(due.saturating_duration_since(Instant::now())),
                None => receiver.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };

            let mut flush_requested = None;
            let mut disconnected = false;
            let deliver_now = match message {
                Ok(Message::Output {
                    stream,
                    text,
                    timestamp,
                }) => {
                    pending_bytes += text.len();
                    match pending.last_mut() {
                        Some(last) if last.stream == stream => last.text.push_str(&text),
                        _ => pending.push(Segment {
                            stream,
                            text,
                            timestamp,
                        }),
                    }
                    let due = *due.get_or_insert_with(|| Instant::now() + interval);
                    // The queue can stay non-empty past the due time, so the
                    // timeout alone would not bound how long a batch waits.
                    pending_bytes >= max_bytes || Instant::now() >= due
                }
                Ok(Message::Flush(delivered)) => {
                    flush_requested = Some(delivered);
                    true
                }
                Err(RecvTimeoutError::Timeout) => true,
                Err(RecvTimeoutError::Disconnected) => {
                    disconnected = true;
                    true
                }
            };

            if deliver_now {
                due = None;
                pending_bytes = 0;
                let batch = std::mem::take(&mut pending);
                if !batch.is_empty() {
                    {
                        let mut buffer = buffer.lock().unwrap();
                        for segment in &batch {
                            buffer.push(segment.stream, &segment.text, segment.timestamp);
                        }
                    }
                    for segment in batch {
                        handle.block_on(listener.on_event(ExecutionEvent::Output {
                            execution_id: execution_id.clone(),
                            chunk: segment.text,
                            stream: segment.stream.to_string(),
                            timestamp: segment.timestamp,
                        }));
                    }
                }
            }
            // Only now is whoever asked for the flush told that it is done.
            drop(flush_requested);

            if disconnected {
                break;
            }
        }
    });

    OutputSender(sender)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output_buffer::OutputBuffer;
    use async_trait::async_trait;

    /// Upper bound on any single wait. Generous, so a loaded machine does not
    /// fail a test; a healthy run takes milliseconds.
    const WAIT: Duration = Duration::from_secs(60);

    /// Forwards the stream and text of every output event to the test.
    struct Recorder {
        events: mpsc::Sender<(String, String)>,
        /// Held while an event is handled, so a test can stall the emitter.
        stall: Arc<std::sync::Mutex<()>>,
    }

    #[async_trait]
    impl ExecutionListener for Recorder {
        async fn on_event(&self, event: ExecutionEvent) {
            let _stalled = self.stall.lock().unwrap();
            if let ExecutionEvent::Output { chunk, stream, .. } = event {
                let _ = self.events.send((stream, chunk));
            }
        }
    }

    struct Fixture {
        output: OutputSender,
        events: mpsc::Receiver<(String, String)>,
        buffer: SharedOutputBuffer,
        stall: Arc<std::sync::Mutex<()>>,
    }

    fn emitter(interval: Duration, max_bytes: usize) -> Fixture {
        let (events_tx, events) = mpsc::channel();
        let stall = Arc::new(std::sync::Mutex::new(()));
        let buffer = OutputBuffer::shared(1024 * 1024);
        let output = spawn(
            "exec".to_string(),
            Arc::new(Recorder {
                events: events_tx,
                stall: stall.clone(),
            }),
            buffer.clone(),
            Handle::current(),
            interval,
            max_bytes,
        );
        Fixture {
            output,
            events,
            buffer,
            stall,
        }
    }

    fn flush(output: &OutputSender) {
        assert!(output.flush_by(Instant::now() + WAIT));
    }

    fn owned(events: &[(&str, &str)]) -> Vec<(String, String)> {
        events
            .iter()
            .map(|(stream, text)| (stream.to_string(), text.to_string()))
            .collect()
    }

    fn buffered(buffer: &SharedOutputBuffer) -> Vec<(String, String)> {
        let chunks = buffer.lock().unwrap().snapshot();
        chunks.into_iter().map(|c| (c.stream, c.chunk)).collect()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reads_are_merged_into_one_event_per_run_of_a_stream() {
        // The interval never elapses here: only the flush delivers.
        let Fixture {
            output,
            events,
            buffer,
            ..
        } = emitter(WAIT, usize::MAX);

        for (stream, text) in [
            ("stdout", "a"),
            ("stdout", "b"),
            ("stderr", "c"),
            ("stdout", "d"),
            ("stdout", ""),
            ("stdout", "e"),
        ] {
            output.send(stream, text.to_string(), 0);
        }
        assert!(events.try_recv().is_err());
        flush(&output);

        // The streams keep the order they were read in.
        let expected = owned(&[("stdout", "ab"), ("stderr", "c"), ("stdout", "de")]);
        assert_eq!(events.try_iter().collect::<Vec<_>>(), expected);
        assert_eq!(buffered(&buffer), expected);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_batch_is_delivered_once_the_interval_has_passed() {
        let Fixture { output, events, .. } = emitter(Duration::from_millis(20), usize::MAX);

        output.send("stdout", "one".to_string(), 0);
        // No flush and no further output: the interval alone delivers it.
        assert_eq!(
            events.recv_timeout(WAIT).unwrap(),
            ("stdout".to_string(), "one".to_string())
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_batch_that_reaches_the_size_limit_is_delivered_at_once() {
        let Fixture { output, events, .. } = emitter(WAIT, 10);

        output.send("stdout", "12345".to_string(), 0);
        output.send("stdout", "6789".to_string(), 0);
        output.send("stdout", "0".to_string(), 0);
        assert_eq!(events.recv_timeout(WAIT).unwrap().1, "1234567890");

        // The next batch starts empty.
        output.send("stdout", "rest".to_string(), 0);
        flush(&output);
        assert_eq!(events.try_iter().collect::<Vec<_>>().len(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_buffer_never_runs_ahead_of_the_listener() {
        let Fixture {
            output,
            events,
            buffer,
            ..
        } = emitter(WAIT, usize::MAX);

        output.send("stdout", "first".to_string(), 0);
        flush(&output);
        assert_eq!(events.try_iter().count(), 1);

        // Queued but not yet delivered: a viewer that reads the buffer now and
        // then follows the events must not get "second" from both.
        output.send("stdout", "second".to_string(), 0);
        assert_eq!(buffered(&buffer), owned(&[("stdout", "first")]));

        flush(&output);
        assert_eq!(buffered(&buffer), owned(&[("stdout", "firstsecond")]));
        assert_eq!(events.try_iter().count(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_flush_gives_up_at_its_deadline() {
        let Fixture {
            output,
            events,
            stall,
            ..
        } = emitter(WAIT, usize::MAX);
        let soon = || Instant::now() + Duration::from_millis(50);

        // The listener is busy with the batch being flushed.
        let stalled = stall.lock().unwrap();
        output.send("stdout", "one".to_string(), 0);
        assert!(!output.flush_by(soon()));

        // The queue fills up behind it, so a flush cannot even be requested.
        for _ in 0..QUEUED_READS {
            output.send("stdout", "x".to_string(), 0);
        }
        assert!(!output.flush_by(soon()));

        // Nothing was lost to either: it is all delivered once the listener
        // catches up.
        drop(stalled);
        flush(&output);
        let delivered: Vec<String> = events.try_iter().map(|(_, text)| text).collect();
        assert_eq!(delivered, vec!["one".to_string(), "x".repeat(QUEUED_READS)]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pending_output_is_delivered_when_the_last_sender_goes() {
        let Fixture { output, events, .. } = emitter(WAIT, usize::MAX);

        output.send("stderr", "tail".to_string(), 0);
        drop(output);

        assert_eq!(
            events.recv_timeout(WAIT).unwrap(),
            ("stderr".to_string(), "tail".to_string())
        );
        // The emitter thread has ended, and its listener with it.
        assert!(events.recv_timeout(WAIT).is_err());
    }
}
