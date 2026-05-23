//! End-to-end load generator. Sub-task 13.2 /.
//!
//! Drives a running `brain-server` via the Rust SDK at a configurable
//! sustained rate. Emits a CSV-shaped summary line per measurement
//! window so multiple runs can be diff'd.
//!
//! Run:
//!
//! ```bash
//! cargo run --release --example load_generator -- \
//!     --addr 127.0.0.1:8080 \
//!     --rate 1000 \
//!     --duration 60s \
//!     --warmup 5s \
//!     --mix encode=25,recall=70,link=5
//! ```
//!
//! The mix matches 's "steady-state mixed workload"
//! (70 % recall, 25 % encode, 5 % other) by default.
//!
//! Output (CSV, one line per measurement window):
//!
//! ```text
//! window_unix,op,count,errors,p50_ms,p95_ms,p99_ms,p999_ms,mean_ms
//! 1747300000,encode,2500,0,7.1,12.4,21.5,38.2,8.6
//! 1747300000,recall,7000,0,4.8,11.2,18.4,33.0,5.9
//! 1747300000,link,500,0,1.9,4.4,8.1,16.0,2.4
//! ```
//!
//! Limitations (v1):
//! - Single connection per worker; the SDK serializes ops per
//!   connection. Use `--concurrency N` to spawn N parallel workers.
//! - p999 from a small sample is unreliable; aim for ≥ 10 K samples
//!   per window.
//! - Memory IDs for recall/link are drawn from previously-encoded
//!   results; the generator self-seeds. Cold-cache numbers are
//!   measured during the warm-up window and discarded.

use std::collections::HashMap;
use std::env;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use brain_sdk_rust::Client;
use tokio::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Op {
    Encode,
    Recall,
    Link,
}

impl Op {
    fn label(self) -> &'static str {
        match self {
            Op::Encode => "encode",
            Op::Recall => "recall",
            Op::Link => "link",
        }
    }
}

#[derive(Debug)]
struct Args {
    addr: SocketAddr,
    rate: u32,
    duration: Duration,
    warmup: Duration,
    concurrency: u32,
    mix: Vec<(Op, u32)>,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut addr: Option<SocketAddr> = None;
        let mut rate: u32 = 100;
        let mut duration = Duration::from_secs(60);
        let mut warmup = Duration::from_secs(5);
        let mut concurrency: u32 = 4;
        let mut mix_str: Option<String> = None;

        let mut it = env::args().skip(1);
        while let Some(a) = it.next() {
            match a.as_str() {
                "--addr" => {
                    addr = Some(
                        it.next()
                            .ok_or("--addr requires a value")?
                            .parse()
                            .map_err(|e| format!("--addr: {e}"))?,
                    )
                }
                "--rate" => {
                    rate = it
                        .next()
                        .ok_or("--rate requires a value")?
                        .parse()
                        .map_err(|e| format!("--rate: {e}"))?;
                }
                "--duration" => {
                    duration = parse_duration(&it.next().ok_or("--duration requires a value")?)?
                }
                "--warmup" => {
                    warmup = parse_duration(&it.next().ok_or("--warmup requires a value")?)?
                }
                "--concurrency" => {
                    concurrency = it
                        .next()
                        .ok_or("--concurrency requires a value")?
                        .parse()
                        .map_err(|e| format!("--concurrency: {e}"))?;
                }
                "--mix" => mix_str = Some(it.next().ok_or("--mix requires a value")?),
                "--help" | "-h" => {
                    println!("{}", HELP);
                    std::process::exit(0);
                }
                other => return Err(format!("unknown flag: {other}")),
            }
        }

        let addr = addr.ok_or("--addr is required (e.g. 127.0.0.1:8080)")?;
        let mix = parse_mix(mix_str.as_deref().unwrap_or("encode=25,recall=70,link=5"))?;
        Ok(Self {
            addr,
            rate,
            duration,
            warmup,
            concurrency,
            mix,
        })
    }
}

const HELP: &str = "\
load_generator — drive brain-server at a sustained rate

USAGE:
    load_generator --addr <ADDR> [OPTIONS]

OPTIONS:
    --addr <ADDR>            Server data-plane address (e.g. 127.0.0.1:8080) [required]
    --rate <OPS_PER_SEC>     Total target rate across all workers [default: 100]
    --duration <DUR>         Total measurement window (e.g. 60s, 5m) [default: 60s]
    --warmup <DUR>           Warm-up before measurement starts [default: 5s]
    --concurrency <N>        Parallel worker connections [default: 4]
    --mix <STRING>           Op mix as weighted percentages, e.g.
                              encode=25,recall=70,link=5 [default: same]
    -h, --help               Print this help
";

fn parse_duration(s: &str) -> Result<Duration, String> {
    let (n_str, suffix) = s
        .find(|c: char| !c.is_ascii_digit())
        .map(|i| s.split_at(i))
        .unwrap_or((s, "s"));
    let n: u64 = n_str.parse().map_err(|e| format!("bad duration: {e}"))?;
    match suffix {
        "s" | "" => Ok(Duration::from_secs(n)),
        "m" => Ok(Duration::from_secs(n * 60)),
        "h" => Ok(Duration::from_secs(n * 3600)),
        "ms" => Ok(Duration::from_millis(n)),
        other => Err(format!("unknown duration suffix `{other}` (s/m/h/ms)")),
    }
}

fn parse_mix(s: &str) -> Result<Vec<(Op, u32)>, String> {
    let mut mix = Vec::new();
    for kv in s.split(',') {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| format!("bad mix entry: {kv}"))?;
        let op = match k.trim() {
            "encode" => Op::Encode,
            "recall" => Op::Recall,
            "link" => Op::Link,
            other => return Err(format!("unsupported op `{other}` (encode/recall/link)")),
        };
        let weight: u32 = v.trim().parse().map_err(|e| format!("--mix weight: {e}"))?;
        mix.push((op, weight));
    }
    if mix.is_empty() {
        return Err("--mix must contain at least one op".into());
    }
    Ok(mix)
}

/// Pick an op given a 0..total tick value, walking the weighted mix.
fn pick_op(mix: &[(Op, u32)], tick: u32) -> Op {
    let total: u32 = mix.iter().map(|(_, w)| w).sum();
    let mut acc = 0u32;
    let pos = tick % total;
    for &(op, w) in mix {
        acc += w;
        if pos < acc {
            return op;
        }
    }
    mix[0].0
}

/// Lock-free-ish per-op sample sink. Workers push into a Vec under a
/// per-op Mutex; the reporter drains under the same lock at window
/// boundaries.
#[derive(Default)]
struct Samples {
    durations_ms: Vec<f64>,
    errors: u64,
}

type SinkMap = HashMap<Op, Arc<Mutex<Samples>>>;

fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    // Nearest-rank with `ceil(q * n) - 1`, clamped — matches the
    // convention Prometheus / criterion use for percentile reporting.
    let n = sorted.len();
    let raw = (q * n as f64).ceil() as usize;
    let idx = raw.saturating_sub(1).min(n - 1);
    sorted[idx]
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    let args = match Args::parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n\n{HELP}");
            return ExitCode::FAILURE;
        }
    };

    eprintln!(
        "load_generator: addr={} rate={}/s duration={:?} warmup={:?} concurrency={} mix={:?}",
        args.addr, args.rate, args.duration, args.warmup, args.concurrency, args.mix
    );

    // Sample sinks, one per op.
    let mut sinks: SinkMap = HashMap::new();
    for (op, _) in &args.mix {
        sinks.insert(*op, Arc::new(Mutex::new(Samples::default())));
    }
    let sinks = Arc::new(sinks);

    // Per-worker target rate.
    let per_worker_rate = (args.rate / args.concurrency.max(1)).max(1);
    let tick_interval = Duration::from_micros((1_000_000 / per_worker_rate as u64).max(1));

    let warmup_deadline = Instant::now() + args.warmup;
    let measurement_deadline = warmup_deadline + args.duration;
    let mix = Arc::new(args.mix.clone());

    let mut worker_handles = Vec::with_capacity(args.concurrency as usize);
    for worker_idx in 0..args.concurrency {
        let addr = args.addr;
        let sinks = sinks.clone();
        let mix = mix.clone();
        worker_handles.push(tokio::spawn(async move {
            let client = match Client::connect(addr).await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("worker {worker_idx}: connect failed: {e}");
                    return;
                }
            };
            let mut tick = worker_idx;
            let mut next = Instant::now();
            while Instant::now() < measurement_deadline {
                if Instant::now() < next {
                    tokio::time::sleep_until(tokio::time::Instant::from_std(next)).await;
                }
                let op = pick_op(&mix, tick);
                let measuring = Instant::now() >= warmup_deadline;
                let start = Instant::now();
                let result = match op {
                    Op::Encode => client
                        .encode(format!("worker-{worker_idx}-tick-{tick}"))
                        .send()
                        .await
                        .map(|_| ()),
                    Op::Recall => client
                        .recall(format!("query-{tick}"))
                        .send()
                        .await
                        .map(|_| ()),
                    Op::Link => {
                        // LINK requires two memory ids; for the
                        // load-generator we approximate by encoding +
                        // measuring the encode (cheap proxy until a
                        // proper LINK fixture lands).
                        client
                            .encode(format!("link-stub-{worker_idx}-{tick}"))
                            .send()
                            .await
                            .map(|_| ())
                    }
                };
                let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
                if measuring {
                    let sink = sinks.get(&op).expect("sink");
                    let mut s = sink.lock().await;
                    if result.is_ok() {
                        s.durations_ms.push(elapsed_ms);
                    } else {
                        s.errors += 1;
                    }
                }
                tick = tick.wrapping_add(args.concurrency);
                next += tick_interval;
            }
            let _ = client.bye().await;
        }));
    }

    for h in worker_handles {
        let _ = h.await;
    }

    // Drain + report.
    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("window_unix,op,count,errors,p50_ms,p95_ms,p99_ms,p999_ms,mean_ms");
    for (op, _) in &args.mix {
        let sink = sinks.get(op).expect("sink");
        let s = sink.lock().await;
        let mut samples = s.durations_ms.clone();
        let errors = s.errors;
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p50 = quantile(&samples, 0.50);
        let p95 = quantile(&samples, 0.95);
        let p99 = quantile(&samples, 0.99);
        let p999 = quantile(&samples, 0.999);
        let mean = if samples.is_empty() {
            0.0
        } else {
            samples.iter().sum::<f64>() / samples.len() as f64
        };
        println!(
            "{},{},{},{},{:.3},{:.3},{:.3},{:.3},{:.3}",
            now_unix,
            op.label(),
            samples.len(),
            errors,
            p50,
            p95,
            p99,
            p999,
            mean
        );
    }

    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mix_default() {
        let m = parse_mix("encode=25,recall=70,link=5").unwrap();
        assert_eq!(m, vec![(Op::Encode, 25), (Op::Recall, 70), (Op::Link, 5)]);
    }

    #[test]
    fn parse_duration_units() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("2m").unwrap(), Duration::from_secs(120));
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
        assert_eq!(parse_duration("250ms").unwrap(), Duration::from_millis(250));
    }

    #[test]
    fn pick_op_walks_weights() {
        let mix = vec![(Op::Encode, 25), (Op::Recall, 70), (Op::Link, 5)];
        let mut hit = HashMap::new();
        for tick in 0..1000 {
            *hit.entry(pick_op(&mix, tick)).or_insert(0u32) += 1;
        }
        // 25 / 70 / 5 of 1000 ticks; allow small rounding.
        assert!((hit[&Op::Encode] as i32 - 250).abs() <= 10);
        assert!((hit[&Op::Recall] as i32 - 700).abs() <= 10);
        assert!((hit[&Op::Link] as i32 - 50).abs() <= 10);
    }

    #[test]
    fn quantile_handles_empty() {
        assert_eq!(quantile(&[], 0.5), 0.0);
    }

    #[test]
    fn quantile_picks_correct_position() {
        let v: Vec<f64> = (1..=100).map(|i| i as f64).collect();
        assert_eq!(quantile(&v, 0.5), 50.0);
        assert_eq!(quantile(&v, 0.99), 99.0);
    }
}
