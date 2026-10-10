//! Bounded cancellable response-body streaming (issue 749 P2c).
//!
//! Split from [`super::http`]: one chunk at a time, never a buffered whole
//! response, with the stream bound, the deadline, and the sticky cancel all
//! enforced per poll.

use futures_util::{Stream, StreamExt};
use tokio::sync::watch;

pub(crate) struct BodyCursor {
    pub(crate) stream:
        std::pin::Pin<Box<dyn Stream<Item = Result<Vec<u8>, std::io::Error>> + Send>>,
    pub(crate) received: u64,
    pub(crate) cancel: watch::Receiver<bool>,
    pub(crate) deadline: tokio::time::Instant,
    pub(crate) max_bytes: u64,
}

/// Poll one body chunk: cancel first, then deadline, then the network.
pub(crate) async fn next_body_chunk(
    cursor: BodyCursor,
) -> Option<(Result<Vec<u8>, std::io::Error>, BodyCursor)> {
    let BodyCursor {
        mut stream,
        received,
        mut cancel,
        deadline,
        max_bytes,
    } = cursor;
    if *cancel.borrow_and_update() {
        return Some((
            Err(cancel_io()),
            BodyCursor {
                stream,
                received,
                cancel,
                deadline,
                max_bytes,
            },
        ));
    }
    enum Step {
        Cancelled,
        TimedOut,
        Chunk(Option<Result<Vec<u8>, std::io::Error>>),
    }
    let step = tokio::select! {
        biased;
        _ = cancel.wait_for(|cancelled| *cancelled) => Step::Cancelled,
        _ = tokio::time::sleep_until(deadline) => Step::TimedOut,
        next = stream.next() => Step::Chunk(next),
    };
    let mut cursor = BodyCursor {
        stream,
        received,
        cancel,
        deadline,
        max_bytes,
    };
    match step {
        Step::Cancelled => Some((Err(cancel_io()), cursor)),
        Step::TimedOut => Some((Err(timeout_io()), cursor)),
        Step::Chunk(next) => match next {
            Some(Ok(chunk)) => {
                cursor.received += chunk.len() as u64;
                if cursor.received > cursor.max_bytes {
                    Some((Err(overflow_io()), cursor))
                } else {
                    Some((Ok(chunk), cursor))
                }
            }
            Some(Err(error)) => Some((Err(error), cursor)),
            None => None,
        },
    }
}

pub(crate) fn cancel_io() -> std::io::Error {
    std::io::Error::other("relay stream cancelled")
}

pub(crate) fn timeout_io() -> std::io::Error {
    std::io::Error::other("relay stream timed out")
}

pub(crate) fn overflow_io() -> std::io::Error {
    std::io::Error::other("relay response exceeded the stream bound")
}

pub(crate) fn unavailable_io() -> std::io::Error {
    std::io::Error::other("relay stream unavailable")
}
