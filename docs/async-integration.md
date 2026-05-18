# Async Integration 🌐

`edifact-rs` is intentionally **synchronous** — it exposes `std::io::Read`-based
APIs. This is the right design for EDIFACT:

- **Parsing is CPU-bound** after the first read; async would add scheduling overhead
  without benefit.
- **Imposing an async runtime** on every downstream user is a heavy design tax.
- **Integration is easy**: the patterns below bridge to any async runtime cleanly.

This guide covers three canonical patterns for Tokio. The same patterns apply to
other runtimes (`async-std`, `smol`) with minor API substitutions.

---

## Dependencies

```toml
[dependencies]
edifact-rs = "0.1"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "fs", "sync"] }
```

---

## Pattern A — Read into memory, parse synchronously

**Best for**: payloads that fit comfortably in memory (typical EDIFACT messages are
well under 1 MB).

```rust,no_run
use edifact_rs::{from_bytes, Segment};

async fn process_edi_file(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Async I/O: read the file without blocking the runtime
    let bytes = tokio::fs::read(path).await?;

    // Synchronous parsing on the async thread — safe because it's fast
    let segments: Vec<_> = from_bytes(&bytes).collect::<Result<_, _>>()?;

    for seg in &segments {
        println!("{}", seg.tag);
    }
    Ok(())
}
```

This is the simplest pattern. Because `from_bytes` is O(n) in CPU (no blocking I/O),
it is safe to call from an async context without `spawn_blocking`.

---

## Pattern B — `spawn_blocking` for large files or blocking sources

**Best for**: large files (multi-MB interchanges), blocking file systems, or
reader-based streaming.

```rust,no_run
use edifact_rs::{OwnedSegment, message_windows_from_reader};

async fn process_large_interchange(path: String)
    -> Result<Vec<Vec<OwnedSegment>>, Box<dyn std::error::Error>>
{
    let windows = tokio::task::spawn_blocking(move || {
        let f = std::fs::File::open(&path)?;
        message_windows_from_reader(f)
            .collect::<Result<Vec<_>, _>>()
    })
    .await??;  // propagates both JoinError and EdifactError

    Ok(windows)
}
```

`spawn_blocking` moves the work onto Tokio's dedicated blocking thread pool, keeping
the async worker threads free.

---

## Pattern C — Channel bridge (backpressure-safe streaming)

**Best for**: very large interchanges where you want to process each `UNH..UNT`
window on the async side as soon as it is parsed, without buffering all windows.

```rust,no_run
use edifact_rs::{EdifactError, OwnedSegment, message_windows_from_reader};
use tokio::sync::mpsc;

async fn stream_interchange(
    bytes: Vec<u8>,
) -> Result<Vec<Vec<OwnedSegment>>, EdifactError> {
    // Channel with backpressure (capacity = 8 windows in flight)
    let (tx, mut rx) = mpsc::channel::<Result<Vec<OwnedSegment>, EdifactError>>(8);

    // Producer: blocking thread
    tokio::task::spawn_blocking(move || {
        let cursor = std::io::Cursor::new(bytes);
        for window in message_windows_from_reader(cursor) {
            if tx.blocking_send(window).is_err() {
                break; // consumer dropped; stop parsing early
            }
        }
    });

    // Consumer: async side — processes windows as they arrive
    let mut all: Vec<Vec<OwnedSegment>> = Vec::new();
    while let Some(result) = rx.recv().await {
        all.push(result?);
    }
    Ok(all)
}
```

The channel capacity controls backpressure: if the consumer is slow, the producer
blocks until there is space in the channel buffer.

---

## Pattern D — Typed message streaming with channel

Combine Pattern C with `deserialize_messages_from_reader` for typed output:

```rust,no_run
use edifact_rs::{EdifactError, EdifactDeserialize, deserialize_messages_from_reader};
use tokio::sync::mpsc;

# #[derive(Debug, EdifactDeserialize)]
# #[edifact(segment = "BGM")]
# struct Bgm { #[edifact(element = 0)] doc_code: String }
# #[derive(Debug, EdifactDeserialize)]
# struct OrderMessage { bgm: Option<Bgm> }
async fn stream_typed_messages(
    bytes: Vec<u8>,
) -> Result<Vec<OrderMessage>, EdifactError> {
    let (tx, mut rx) = mpsc::channel::<Result<OrderMessage, EdifactError>>(8);

    tokio::task::spawn_blocking(move || {
        let cursor = std::io::Cursor::new(bytes);
        for msg in deserialize_messages_from_reader::<OrderMessage, _>(cursor) {
            if tx.blocking_send(msg).is_err() {
                break;
            }
        }
    });

    let mut messages = Vec::new();
    while let Some(result) = rx.recv().await {
        messages.push(result?);
    }
    Ok(messages)
}
```

---

## Pattern E — Process bytes from `AsyncRead`

When your async input is an `AsyncRead` (e.g. `tokio::net::TcpStream`), buffer it
into memory before parsing, or use `spawn_blocking` with a synchronous wrapper:

```rust,no_run
use tokio::io::{AsyncReadExt};
use edifact_rs::from_bytes;

async fn parse_from_async_reader<R: AsyncReadExt + Unpin>(
    mut reader: R,
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await?;

    // Now parse synchronously
    let segments: Vec<_> = from_bytes(&buf).collect::<Result<_, _>>()?;
    Ok(segments.len())
}
```

> **Why not `AsyncBufRead`?** `edifact-rs` exposes `std::io::BufRead` internally.
> The simplest bridge is `read_to_end` into a `Vec<u8>`. For byte-by-byte bridging
> see the `async-compat` crate or write an `std::io::Read` adapter.

---

## Choosing a pattern

| Payload size | Source | Recommended pattern |
|---|---|---|
| < 1 MB | Any | Pattern A — buffer, then `from_bytes` |
| 1 MB – 100 MB | File | Pattern B — `spawn_blocking` + `from_reader_iter` |
| > 100 MB | File | Pattern B or C — streaming with channel |
| Any | `TcpStream` / HTTP | Pattern A (small) or E + B (large) |
| Any, with typed structs | Reader | Pattern D — typed channel streaming |

---

## Error propagation

`spawn_blocking` returns a `JoinError` if the worker thread panics. Map it to your
own error type:

```rust,no_run
use edifact_rs::EdifactError;

async fn safe_parse(bytes: Vec<u8>) -> Result<usize, EdifactError> {
    tokio::task::spawn_blocking(move || {
        edifact_rs::from_bytes(&bytes).count()
    })
    .await
    .map_err(|join_err| EdifactError::ValidationFailed {
        error_count: 1,
        first_message: format!("parse thread panicked: {join_err}"),
    })
}
```

---

## Complete example

```bash
cargo run -p edifact-rs --example cookbook_async_tokio_integration
```

See [`cookbook_async_tokio_integration.rs`](../crates/edifact-rs/examples/cookbook_async_tokio_integration.rs)
for a runnable example demonstrating Patterns A, B, and C.

---

## Next steps

- [Streaming](streaming.md) — synchronous streaming APIs
- [Performance](performance.md) — memory budgets and benchmarking
