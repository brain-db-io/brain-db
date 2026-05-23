# 06.04 Retries and Streams

> **TL;DR.** Retry policy and idempotency at the SDK layer, plus the streaming-response model: per-operation retry decisions with exponential backoff, idempotency-key generation, and an async-iterator surface for SUBSCRIBE / large RECALL / any future streaming opcodes.

## Retries

Retry policy and idempotency in the SDK.

## 1. The retry decision

For each operation, the SDK decides:

- Is this error retryable?
- Has the retry budget been exhausted?
- Should it wait before retrying?

```rust
fn should_retry(err: &BrainError, attempt: u32, config: &RetryConfig) -> bool {
    err.is_retryable() && attempt < config.max_attempts
}

fn retry_delay(attempt: u32, config: &RetryConfig) -> Duration {
    let base = config.initial_delay;
    let factor = config.backoff_factor.pow(attempt - 1);
    let with_jitter = base * factor * jitter();
    with_jitter.min(config.max_delay)
}
```

## 2. Retryable errors

Per [05. Operations](../05_operations/00_purpose.md) §error model:

| Error code | Retryable |
|---|---|
| InvalidRequest | No |
| NotFound | No |
| Unauthorized | No |
| QuotaExceeded | No |
| Conflict (idempotency mismatch) | No |
| Overloaded | Yes |
| Timeout | Yes |
| NetworkError | Yes |
| InternalError | Yes (carefully) |
| EmbedderUnavailable | Yes |

Brain's responses include the `retryable` flag explicitly. The SDK respects it.

## 3. Idempotency

State-mutating operations require idempotency:

- ENCODE
- FORGET
- LINK / UNLINK
- TXN_COMMIT

Each of these requires a `RequestId`. If not provided, the SDK generates one:

```rust
let request_id = RequestId::generate();    // UUIDv7
client.encode("text").request_id(request_id).send().await?;
```

The auto-generated RequestId is stable for the lifetime of the operation — retries use the same RequestId. Brain deduplicates.

## 4. Read operations and retries

Read operations (RECALL, PLAN, REASON, ADMIN_STATS) are idempotent by nature:

- Brain doesn't change state when serving them.
- A retry just re-runs the read.
- No RequestId needed.

Retries on reads are simpler — just resend.

## 5. The retry loop

```rust
async fn execute_with_retry(
    op: impl Fn() -> impl Future<Output = Result<R, BrainError>>,
    config: &RetryConfig,
) -> Result<R, BrainError> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        match op().await {
            Ok(r) => return Ok(r),
            Err(e) if !should_retry(&e, attempt, config) => return Err(e),
            Err(e) => {
                let delay = retry_delay(attempt, config);
                tracing::warn!("Retry attempt {}, error: {}", attempt, e);
                tokio::time::sleep(delay).await;
            }
        }
    }
}
```

The SDK wraps each operation with this retry logic.

## 6. Exponential backoff with jitter

Default config:

```
max_attempts = 3
initial_delay = 100ms
backoff_factor = 2.0
max_delay = 30s
jitter = 0.1 (±10%)
```

So delays: 100ms, 200ms, 400ms (with jitter).

Jitter prevents synchronized retries from many clients ("thundering herd").

## 7. The retry budget

Beyond `max_attempts`, the SDK gives up. The error is returned to the caller.

The caller can:
- Implement application-level retry on top.
- Treat the failure as terminal.

The SDK's retries are first-line defense; not unlimited.

## 8. Per-operation retry config

Different operations may want different retry configs:

```rust
client.encode("text")
    .retry_config(RetryConfig::aggressive())    // More retries for important data
    .send().await?;

client.recall("cue")
    .retry_config(RetryConfig::fast_fail())      // Don't retry; user can re-cue
    .send().await?;
```

Defaults are conservative; per-op overrides for special cases.

## 9. The "retry-after" header

If Brain's Overloaded response includes a "retry after" duration, the SDK respects it:

```rust
match err {
    Error::Overloaded { retry_after: Some(d) } => sleep(d).await,
    _ => sleep(retry_delay(attempt, config)).await,
}
```

This lets Brain signal "I'll be ready in 5 seconds; back off until then".

## 10. The "retry exhausted" error

When all retries fail, the SDK returns the last error with retry context:

```rust
Err(BrainError::RetryExhausted {
    last_error: Box::new(actual_err),
    attempts: 3,
    total_duration: Duration::from_millis(700),
})
```

The caller knows that retries were attempted. They can choose to retry further or treat as failure.

## 11. The "no retry" option

For applications that prefer manual retry control:

```rust
let client = Client::builder()
    .retry_config(RetryConfig::none())    // No automatic retries
    .build();
```

Or per-operation:

```rust
client.encode("text").no_retry().send().await?;
```

The application is responsible for retries. Useful for systems that already have retry logic.

## 12. Retries and tracing

Each retry is logged / traced:

```
encode op started [request_id=abc]
  attempt 1: NetworkError after 50ms
  attempt 2: succeeded after 120ms
encode op completed [duration=170ms, attempts=2]
```

This visibility helps debugging.

## 13. Retries and timeouts

There are two timeouts:

- **Per-attempt timeout** (default 30s): each individual request can take up to this long.
- **Total timeout** (default 60s): retries plus delays must finish within this.

If total exceeds, the SDK gives up regardless of attempts.

```rust
client.encode("text")
    .per_attempt_timeout(Duration::from_secs(10))
    .total_timeout(Duration::from_secs(60))
    .send().await?;
```

## 14. The "first attempt's error is the user's error" rule

If the first attempt fails with InvalidRequest (non-retryable), the SDK doesn't retry — the error is returned. The user fixes the request.

If it fails with Overloaded, retries kick in. The user shouldn't see Overloaded unless retries fail.

The SDK handles transient errors transparently; only persistent errors surface to the user.

## 15. Retries and side effects

For idempotent operations, retries are safe.

For non-idempotent operations (rare in Brain — Brain enforces idempotency for state-mutating operations), retries may cause duplication.

The SDK's RequestId mechanism makes all state-mutating operations idempotent. So retries are always safe for Brain operations.

## 16. The "fail fast" mode

For low-latency applications, retries add latency. An option:

```rust
let client = Client::builder()
    .fail_fast(true)
    .build();
```

In fail-fast mode:
- Single attempt.
- No retries.
- Immediate failure on any error.

The application implements its own retry strategy at a higher layer.

## 17. The retry history

For debugging, the SDK can record per-request retry history:

```rust
let result = client.encode("text").send().await;
let history = client.last_request_history();    // e.g., ["NetworkError", "Success"]
```

Useful in tests and during development.

## 18. The retries-and-timeout interaction

If the per-attempt timeout fires during a retry attempt:

- The attempt is canceled.
- The error is `Timeout`.
- If retryable (yes for Timeout), retry continues until exhausted.

If the total timeout fires:

- The whole operation is canceled.
- The error is `Timeout` (with "total" context).
- No more retries.

The two timeouts interact: per-attempt for individual calls, total for the whole operation.

---

## Streaming Responses

How the SDK handles streaming responses — primarily SUBSCRIBE, but also large RECALL or any future streaming opcodes.

## 1. The streaming model

A streaming response is a sequence of frames on the same stream ID, each containing a response chunk. The SDK exposes this as an async iterator:

```rust
let mut stream = client.subscribe()
    .agent("agent-id")
    .start()
    .await?;

while let Some(event) = stream.next().await? {
    process_event(event);
}
```

The user gets events one at a time. The SDK handles framing, buffering, and flow control.

## 2. The stream's lifecycle

```
1. Client sends SUBSCRIBE frame with stream_id=N.
2. Server begins streaming events on stream_id=N.
3. Each event arrives as a frame; the SDK puts it in the stream's buffer.
4. Each `next()` call returns an event from the buffer.
5. When the user is done, `stream.close()` (or drop) signals end.
6. Server finishes, sends final frame, closes stream.
```

The stream is async; clients can pause without losing events (the server respects the client's window).

## 3. Backpressure

The SDK supports backpressure:

- The SDK maintains a buffer of received events (default size 1000).
- If the buffer fills, the SDK stops reading from the connection (TCP backpressure).
- Brain observes the slow client via TCP window; pauses sending.

For applications processing slowly, this prevents memory blowup.

## 4. The flow-control window

Per the wire protocol's stream multiplexing, each stream has a window:

```rust
struct StreamWindow {
    available: u32,    // Frames Brain can send before waiting for ack
    consumed: u32,
}
```

The SDK acks frames after the user reads them; the window updates.

Users don't see this directly, but the mechanism enforces backpressure end-to-end.

## 5. Cancellation

To cancel a stream:

```rust
stream.close().await?;
// Or in async iterators with drop:
drop(stream);
```

The SDK sends a `CANCEL_STREAM` frame. Brain emits an `EOS` ack and stops sending on this stream.

If the user simply drops the stream object (in Rust), the destructor sends the close. In Python, an async context manager pattern is used:

```python
async with client.subscribe(...) as stream:
    async for event in stream:
        process(event)
# Auto-closes on exit
```

## 6. Filters

SUBSCRIBE accepts filters:

- Per agent.
- Per context.
- Per event kind.
- Per memory kind.

These are sent in the initial SUBSCRIBE frame; Brain filters server-side.

```rust
client.subscribe()
    .agent("agent-id")
    .contexts(["important"])
    .events([EventKind::MemoryCreated])
    .start()
    .await?;
```

## 7. The "starting position"

A SUBSCRIBE can start:

- From now (default): only events after subscription.
- From a specific LSN: replay events starting from there.
- From the beginning: all events ever.

```rust
client.subscribe()
    .agent("agent-id")
    .start_from(StartPosition::Lsn(12345))
    .start()
    .await?;
```

For from-LSN subscribes, Brain must have the WAL records still on disk (per [15.03 WAL Retention](../15_background_workers/03_substrate_sweepers.md)). If too old, the SDK gets a "LSN not available" error.

## 8. Reconnection during streaming

If the connection drops mid-stream:

- In-buffer events are still available.
- New events are not received until reconnect.
- The SDK can reconnect and resume:
  - Brain's stream state may not survive.
  - The SDK re-subscribes from the last received LSN.

This is opt-in:

```rust
let mut stream = client.subscribe()
    .agent("agent-id")
    .resume_on_disconnect(true)
    .start()
    .await?;
```

Without resume, a disconnect ends the stream and the user reconnects manually.

## 9. The ordering guarantee

Within a stream, events arrive in WAL order. Different agents' events on the same stream may interleave but each is sequential.

Brain doesn't reorder events.

## 10. Large RECALL responses

For RECALL with very large K (1000+) and `include_text`, the response may exceed the wire protocol's frame size limit (~16 MiB).

The SDK handles this transparently:

- Brain streams the response across multiple frames.
- The SDK assembles them.
- The user sees a single response object.

For extremely large responses, the SDK could expose them as streams instead of single objects:

```rust
let mut results = client.recall("cue").k(10000).stream().await?;
while let Some(r) = results.next().await? {
    process_result(r);
}
```

`stream()` mode delivers results as they arrive; the user processes them incrementally.

## 11. The stream is iterable

Each language's idiomatic iteration:

```rust
// Rust
while let Some(item) = stream.next().await? {
    process(item);
}
```

```python
# Python
async for item in stream:
    process(item)
```

```typescript
// TypeScript
for await (const item of stream) {
    process(item);
}
```

```go
// Go
for {
    item, err := stream.Recv()
    if err == io.EOF { break }
    if err != nil { return err }
    process(item)
}
```

## 12. The error stream

Errors during streaming are delivered as errors, not as events:

```rust
while let Some(item) = stream.next().await {
    match item {
        Ok(event) => process(event),
        Err(e) => {
            log_error(e);
            break;
        }
    }
}
```

A non-recoverable error ends the stream. A recoverable error (transient) might be auto-retried by the SDK if `resume_on_disconnect` is enabled.

## 13. The keep-alive on streams

For long-lived subscriptions, Brain sends keep-alive frames:

- Empty event frames every 30 seconds.
- The SDK ignores them (they're for liveness only).

If keep-alives stop arriving (default timeout 90 seconds), the SDK considers the connection dead.

## 14. The "stream close acks"

When the SDK closes a stream:

- Sends `CANCEL_STREAM` frame.
- Awaits Brain's `EOS` ack.
- If ack doesn't arrive within timeout, log a warning but proceed.

## 15. The metrics for streams

The SDK exposes:

- Active streams count.
- Events received per stream.
- Buffer size (current).
- Lag (time between event creation and reception).

For long-lived streams, monitoring these helps detect issues.

## 16. The "fan-out" stream

For multi-shard subscriptions (a multi-shard agent), the SDK fans out:

- Subscribes to each shard.
- Merges events from all subscribers.
- Presents as a single stream.

Ordering across shards is best-effort (timestamp-based); strict per-shard ordering is preserved.

## 17. The "single-use" stream

A stream is single-use:

- Once closed, can't be resumed.
- For reconnection, create a new stream.

This simplifies the SDK's state management.

---

*Continue to [`05_idiomatic_languages.md`](05_idiomatic_languages.md) for language-specific idioms.*
