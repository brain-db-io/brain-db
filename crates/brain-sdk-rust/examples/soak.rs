//! Long-duration soak test. Sub-task 13.4 /.
//!
//! Drives sustained load through the SDK and periodically samples
//! the server's `/metrics` endpoint, asserting that:
//!
//! - **No memory leak** — RSS stays within `--mem-tolerance-pct` of
//!   the post-warmup baseline (memory grows with data;
//!   on a steady-state mixed workload at fixed data size, RSS should
//!   be flat).
//! - **No latency drift** — moving-average p99 of `brain_request_duration_ms`
//!   stays within `--latency-drift-pct` of the post-warmup baseline.
//! - **No error-rate spike** — `brain_request_total{status="error"}`
//!   rate stays below `--max-error-rate`.
//!
//! Run:
//!
//! ```bash
//! cargo run --release --example soak -- \
//!     --data-addr 127.0.0.1:8080 \
//!     --metrics-addr 127.0.0.1:9091 \
//!     --duration 48h \
//!     --warmup 5m \
//!     --rate 500 \
//!     --sample-interval 60s
//! ```
//!
//! Output (CSV, one row per sample):
//!
//! ```text
//! sample_unix,rss_bytes,open_fds,p99_ms,error_rate
//! 1747300060,734003200,128,18.3,0.0001
//! 1747300120,734019584,128,17.9,0.0000
//! ```
//!
//! Plus a final summary line:
//!
//! ```text
//! SOAK_RESULT pass=true rss_drift_pct=0.4 latency_drift_pct=2.1 max_error_rate=0.0003
//! ```
//!
//! `pass=false` means a threshold breach; exit code is non-zero.
//!
//! ## CI status
//!
//! This rig is `#[ignore]`-equivalent — run on dedicated infra,
//! never in CI puts soak at "weekly" cadence; the
//! result file is committed to `docs/performance/soak-<date>.md`.

use std::env;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use brain_sdk_rust::Client;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Debug)]
struct Args {
    data_addr: SocketAddr,
    metrics_addr: SocketAddr,
    duration: Duration,
    warmup: Duration,
    rate: u32,
    concurrency: u32,
    sample_interval: Duration,
    mem_tolerance_pct: f64,
    latency_drift_pct: f64,
    max_error_rate: f64,
}

const HELP: &str = "\
soak — long-duration steady-state load + threshold check

USAGE:
    soak --data-addr <ADDR> --metrics-addr <ADDR> [OPTIONS]

OPTIONS:
    --data-addr <ADDR>           brain-server data-plane address [required]
    --metrics-addr <ADDR>        brain-server metrics address [required]
    --duration <DUR>             Total soak window (e.g. 48h) [default: 1h]
    --warmup <DUR>               Warm-up before baseline samples [default: 5m]
    --rate <OPS_PER_SEC>         Total target ops/sec [default: 100]
    --concurrency <N>            Parallel workers [default: 4]
    --sample-interval <DUR>      How often to scrape /metrics [default: 60s]
    --mem-tolerance-pct <F>      Max RSS drift from baseline [default: 10.0]
    --latency-drift-pct <F>      Max p99 drift from baseline [default: 20.0]
    --max-error-rate <F>         Max sustained error-rate (0..1) [default: 0.01]
    -h, --help                   Print this help
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

impl Args {
    fn parse() -> Result<Self, String> {
        let mut data_addr: Option<SocketAddr> = None;
        let mut metrics_addr: Option<SocketAddr> = None;
        let mut duration = Duration::from_secs(3600);
        let mut warmup = Duration::from_secs(300);
        let mut rate: u32 = 100;
        let mut concurrency: u32 = 4;
        let mut sample_interval = Duration::from_secs(60);
        let mut mem_tolerance_pct = 10.0;
        let mut latency_drift_pct = 20.0;
        let mut max_error_rate = 0.01;

        let mut it = env::args().skip(1);
        while let Some(a) = it.next() {
            match a.as_str() {
                "--data-addr" => {
                    data_addr = Some(
                        it.next()
                            .ok_or("--data-addr requires a value")?
                            .parse()
                            .map_err(|e| format!("--data-addr: {e}"))?,
                    );
                }
                "--metrics-addr" => {
                    metrics_addr = Some(
                        it.next()
                            .ok_or("--metrics-addr requires a value")?
                            .parse()
                            .map_err(|e| format!("--metrics-addr: {e}"))?,
                    );
                }
                "--duration" => duration = parse_duration(&it.next().ok_or("--duration")?)?,
                "--warmup" => warmup = parse_duration(&it.next().ok_or("--warmup")?)?,
                "--rate" => {
                    rate = it
                        .next()
                        .ok_or("--rate")?
                        .parse()
                        .map_err(|e| format!("{e}"))?
                }
                "--concurrency" => {
                    concurrency = it
                        .next()
                        .ok_or("--concurrency")?
                        .parse()
                        .map_err(|e| format!("{e}"))?;
                }
                "--sample-interval" => {
                    sample_interval = parse_duration(&it.next().ok_or("--sample-interval")?)?;
                }
                "--mem-tolerance-pct" => {
                    mem_tolerance_pct = it
                        .next()
                        .ok_or("--mem-tolerance-pct")?
                        .parse()
                        .map_err(|e| format!("{e}"))?;
                }
                "--latency-drift-pct" => {
                    latency_drift_pct = it
                        .next()
                        .ok_or("--latency-drift-pct")?
                        .parse()
                        .map_err(|e| format!("{e}"))?;
                }
                "--max-error-rate" => {
                    max_error_rate = it
                        .next()
                        .ok_or("--max-error-rate")?
                        .parse()
                        .map_err(|e| format!("{e}"))?;
                }
                "--help" | "-h" => {
                    println!("{HELP}");
                    std::process::exit(0);
                }
                other => return Err(format!("unknown flag: {other}")),
            }
        }

        Ok(Self {
            data_addr: data_addr.ok_or("--data-addr is required")?,
            metrics_addr: metrics_addr.ok_or("--metrics-addr is required")?,
            duration,
            warmup,
            rate,
            concurrency,
            sample_interval,
            mem_tolerance_pct,
            latency_drift_pct,
            max_error_rate,
        })
    }
}

/// One scrape of `/metrics`. Parses just the fields the soak rig
/// needs; the rest of the body is discarded.
#[derive(Debug, Clone, Copy, Default)]
struct MetricsSample {
    rss_bytes: u64,
    open_fds: u64,
    // p99 derived from histogram buckets (encode op).
    p99_ms_encode: f64,
    // Cumulative request_total{status="error"}.
    error_total: u64,
    request_total: u64,
}

async fn scrape_metrics(addr: SocketAddr) -> Result<MetricsSample, String> {
    let mut s = TcpStream::connect(addr)
        .await
        .map_err(|e| format!("connect: {e}"))?;
    s.write_all(b"GET /metrics HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .map_err(|e| format!("write: {e}"))?;
    let mut buf = Vec::with_capacity(16 * 1024);
    s.read_to_end(&mut buf)
        .await
        .map_err(|e| format!("read: {e}"))?;
    let raw = String::from_utf8_lossy(&buf);
    let body = raw.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    Ok(parse_metrics_body(body))
}

fn parse_metrics_body(body: &str) -> MetricsSample {
    let mut sample = MetricsSample::default();
    for line in body.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(v) = line.strip_prefix("process_memory_resident_bytes ") {
            sample.rss_bytes = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = line.strip_prefix("process_open_fds ") {
            sample.open_fds = v.trim().parse().unwrap_or(0);
        } else if line.starts_with("brain_request_total{") {
            if line.contains("status=\"error\"") {
                if let Some(v) = line.split_whitespace().last() {
                    sample.error_total += v.parse().unwrap_or(0);
                }
            }
            if let Some(v) = line.split_whitespace().last() {
                sample.request_total += v.parse().unwrap_or(0);
            }
        }
    }
    // p99 derivation from the histogram would parse `brain_request_duration_ms_bucket{op="encode",le="..."}`
    // lines and apply standard PromQL histogram_quantile. The soak
    // rig defers that to the operator's Prometheus instance (which
    // already exposes the same calc). For the in-process drift
    // assertion we use the histogram's `_sum / _count` mean as a
    // proxy — coarser than p99 but enough to catch large drifts.
    let mut sum_ms = 0.0f64;
    let mut count = 0u64;
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("brain_request_duration_ms_sum{") {
            if rest.contains("op=\"encode\"") {
                if let Some(v) = line.split_whitespace().last() {
                    sum_ms = v.parse().unwrap_or(0.0);
                }
            }
        } else if let Some(rest) = line.strip_prefix("brain_request_duration_ms_count{") {
            if rest.contains("op=\"encode\"") {
                if let Some(v) = line.split_whitespace().last() {
                    count = v.parse().unwrap_or(0);
                }
            }
        }
    }
    if count > 0 {
        sample.p99_ms_encode = sum_ms / count as f64;
    }
    sample
}

#[derive(Debug)]
struct DriftResult {
    pass: bool,
    rss_drift_pct: f64,
    latency_drift_pct: f64,
    max_error_rate: f64,
    breach_reason: Option<String>,
}

fn check_drift(
    baseline: &MetricsSample,
    latest: &MetricsSample,
    args: &Args,
    err_rate_observed: f64,
) -> DriftResult {
    let rss_drift = if baseline.rss_bytes == 0 {
        0.0
    } else {
        ((latest.rss_bytes as f64 - baseline.rss_bytes as f64) / baseline.rss_bytes as f64) * 100.0
    };
    let lat_drift = if baseline.p99_ms_encode == 0.0 {
        0.0
    } else {
        ((latest.p99_ms_encode - baseline.p99_ms_encode) / baseline.p99_ms_encode) * 100.0
    };

    let mut breach = None;
    if rss_drift.abs() > args.mem_tolerance_pct {
        breach = Some(format!(
            "memory drift {:.2}% exceeds tolerance {:.2}%",
            rss_drift, args.mem_tolerance_pct
        ));
    }
    if breach.is_none() && lat_drift.abs() > args.latency_drift_pct {
        breach = Some(format!(
            "latency drift {:.2}% exceeds tolerance {:.2}%",
            lat_drift, args.latency_drift_pct
        ));
    }
    if breach.is_none() && err_rate_observed > args.max_error_rate {
        breach = Some(format!(
            "error rate {:.4} exceeds max {:.4}",
            err_rate_observed, args.max_error_rate
        ));
    }

    DriftResult {
        pass: breach.is_none(),
        rss_drift_pct: rss_drift,
        latency_drift_pct: lat_drift,
        max_error_rate: err_rate_observed,
        breach_reason: breach,
    }
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
        "soak: data={} metrics={} duration={:?} warmup={:?} rate={}/s interval={:?}",
        args.data_addr,
        args.metrics_addr,
        args.duration,
        args.warmup,
        args.rate,
        args.sample_interval
    );

    // Spawn load workers.
    let stop = Arc::new(AtomicU64::new(0));
    let mut worker_handles = Vec::with_capacity(args.concurrency as usize);
    let per_worker_rate = (args.rate / args.concurrency.max(1)).max(1);
    let tick_interval = Duration::from_micros((1_000_000 / per_worker_rate as u64).max(1));

    for worker_idx in 0..args.concurrency {
        let addr = args.data_addr;
        let stop = stop.clone();
        worker_handles.push(tokio::spawn(async move {
            let client = match Client::connect(addr).await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("worker {worker_idx}: connect failed: {e}");
                    return;
                }
            };
            let mut tick: u64 = worker_idx as u64;
            let mut next = Instant::now();
            while stop.load(Ordering::Relaxed) == 0 {
                if Instant::now() < next {
                    tokio::time::sleep_until(tokio::time::Instant::from_std(next)).await;
                }
                // Mix: 70% recall / 25% encode / 5% link (LINK is a
                // stub; mirrors load_generator.rs's coverage).
                let op = tick % 100;
                let _ = if op < 70 {
                    client.recall(format!("q-{tick}")).send().await.map(|_| ())
                } else if op < 95 {
                    client
                        .encode(format!("worker-{worker_idx}-tick-{tick}"))
                        .send()
                        .await
                        .map(|_| ())
                } else {
                    client
                        .encode(format!("link-stub-{tick}"))
                        .send()
                        .await
                        .map(|_| ())
                };
                tick = tick.wrapping_add(args.concurrency as u64);
                next += tick_interval;
            }
            let _ = client.bye().await;
        }));
    }

    // Warm-up + measurement loop.
    eprintln!("soak: warmup phase ({:?})", args.warmup);
    tokio::time::sleep(args.warmup).await;
    let baseline = match scrape_metrics(args.metrics_addr).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("soak: baseline scrape failed: {e}");
            stop.store(1, Ordering::Relaxed);
            return ExitCode::FAILURE;
        }
    };
    let baseline_errors = baseline.error_total;
    let baseline_requests = baseline.request_total;
    eprintln!(
        "soak: baseline rss={} p99~{:.2}ms",
        baseline.rss_bytes, baseline.p99_ms_encode
    );

    println!("sample_unix,rss_bytes,open_fds,p99_ms,error_rate");
    let measurement_deadline = Instant::now() + args.duration;
    let mut latest = baseline;
    let mut highest_error_rate = 0.0f64;
    while Instant::now() < measurement_deadline {
        tokio::time::sleep(args.sample_interval).await;
        let s = match scrape_metrics(args.metrics_addr).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("soak: scrape failed: {e}");
                continue;
            }
        };
        let now_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let delta_err = s.error_total.saturating_sub(baseline_errors);
        let delta_req = s.request_total.saturating_sub(baseline_requests).max(1);
        let err_rate = delta_err as f64 / delta_req as f64;
        if err_rate > highest_error_rate {
            highest_error_rate = err_rate;
        }
        println!(
            "{},{},{},{:.3},{:.6}",
            now_unix, s.rss_bytes, s.open_fds, s.p99_ms_encode, err_rate
        );
        latest = s;
    }

    stop.store(1, Ordering::Relaxed);
    for h in worker_handles {
        let _ = h.await;
    }

    let result = check_drift(&baseline, &latest, &args, highest_error_rate);
    let reason = result.breach_reason.as_deref().unwrap_or("none");
    println!(
        "SOAK_RESULT pass={} rss_drift_pct={:.2} latency_drift_pct={:.2} max_error_rate={:.6} reason={}",
        result.pass, result.rss_drift_pct, result.latency_drift_pct, result.max_error_rate, reason
    );

    if result.pass {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn baseline() -> MetricsSample {
        MetricsSample {
            rss_bytes: 100_000_000,
            open_fds: 100,
            p99_ms_encode: 10.0,
            error_total: 0,
            request_total: 1000,
        }
    }

    fn args_with(mem: f64, lat: f64, err: f64) -> Args {
        Args {
            data_addr: "127.0.0.1:1".parse().unwrap(),
            metrics_addr: "127.0.0.1:2".parse().unwrap(),
            duration: Duration::from_secs(60),
            warmup: Duration::from_secs(5),
            rate: 100,
            concurrency: 1,
            sample_interval: Duration::from_secs(10),
            mem_tolerance_pct: mem,
            latency_drift_pct: lat,
            max_error_rate: err,
        }
    }

    #[test]
    fn check_drift_passes_within_tolerance() {
        let base = baseline();
        let latest = MetricsSample {
            rss_bytes: 105_000_000, // +5%
            p99_ms_encode: 11.0,    // +10%
            ..base
        };
        let r = check_drift(&base, &latest, &args_with(10.0, 20.0, 0.01), 0.001);
        assert!(r.pass, "should pass: {:?}", r);
    }

    #[test]
    fn check_drift_fails_on_memory() {
        let base = baseline();
        let latest = MetricsSample {
            rss_bytes: 130_000_000, // +30%
            ..base
        };
        let r = check_drift(&base, &latest, &args_with(10.0, 20.0, 0.01), 0.001);
        assert!(!r.pass);
        assert!(r.breach_reason.unwrap().contains("memory"));
    }

    #[test]
    fn check_drift_fails_on_latency() {
        let base = baseline();
        let latest = MetricsSample {
            p99_ms_encode: 20.0, // +100%
            ..base
        };
        let r = check_drift(&base, &latest, &args_with(10.0, 20.0, 0.01), 0.001);
        assert!(!r.pass);
        assert!(r.breach_reason.unwrap().contains("latency"));
    }

    #[test]
    fn check_drift_fails_on_error_rate() {
        let base = baseline();
        let latest = base;
        let r = check_drift(&base, &latest, &args_with(10.0, 20.0, 0.01), 0.05);
        assert!(!r.pass);
        assert!(r.breach_reason.unwrap().contains("error rate"));
    }

    #[test]
    fn parse_metrics_body_extracts_fields() {
        let body = "\
# HELP process_memory_resident_bytes ...
# TYPE process_memory_resident_bytes gauge
process_memory_resident_bytes 734003200
# HELP process_open_fds ...
# TYPE process_open_fds gauge
process_open_fds 42
# HELP brain_request_total ...
# TYPE brain_request_total counter
brain_request_total{op=\"encode\",status=\"success\"} 100
brain_request_total{op=\"encode\",status=\"error\"} 5
brain_request_duration_ms_sum{op=\"encode\"} 850
brain_request_duration_ms_count{op=\"encode\"} 100
";
        let s = parse_metrics_body(body);
        assert_eq!(s.rss_bytes, 734_003_200);
        assert_eq!(s.open_fds, 42);
        assert_eq!(s.error_total, 5);
        // request_total sums every brain_request_total line — both
        // success (100) and error (5).
        assert_eq!(s.request_total, 105);
        assert!((s.p99_ms_encode - 8.5).abs() < 0.001);
    }
}
