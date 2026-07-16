//! Cold-start benchmarks for the `mscode` binary.
//!
//! Three scenarios with hard budgets enforced by Phase 7's local-first
//! constraints:
//!
//! 1. `version` - synchronous fast path, no async, no SQLite.
//!    Budget: < 100ms release median.
//! 2. `chat --help` - clap parse + help formatting; a proxy for the parser
//!    fast path that doesn't touch the LLM or TUI. Budget: < 150ms release median.
//! 3. `new` (first SQLite op) - opens the SQLite pool, writes one session row,
//!    exits. Budget: < 200ms release median.
//!
//! These benchmarks spawn the mscode binary as a subprocess. The measured
//! latency therefore reflects the *true* end-to-end cold start the user
//! experiences (binary load, runtime init, command dispatch) rather than a
//! function-level micro-benchmark. They are the authoritative measurement of
//! the budgets the integration test (`cold_start_under_200ms_release_build`)
//! enforces.
//!
//! ## Running
//!
//! ```text
//! cargo bench -p mscode-cli
//! ```
//!
//! On CI, run with `--bench` to skip the long criterion warmup phase:
//!
//! ```text
//! cargo bench -p mscode-cli -- --quick
//! ```

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use tempfile::tempdir;

/// Resolve the mscode binary built by this package. We are inside
/// `crates/cli`, so `env!("CARGO_BIN_EXE_mscode")` is set by cargo.
fn bin() -> PathBuf {
    // Within the same package as the binary target, CARGO_BIN_EXE_<name> is
    // available at compile time.
    PathBuf::from(env!("CARGO_BIN_EXE_mscode"))
}

/// Benchmark `mscode version` cold-start. No async runtime, no SQLite.
fn bench_version(c: &mut Criterion) {
    let mut group = c.benchmark_group("cold_start/version");
    group.sample_size(20);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(5));
    group.throughput(Throughput::Elements(1));

    group.bench_function(BenchmarkId::new("version", "release"), |b| {
        b.iter(|| {
            let out = Command::new(bin()).arg("version").output().expect("spawn");
            assert!(out.status.success(), "version must succeed");
        })
    });
    group.finish();
}

/// Benchmark the clap parse + help formatting path. This is a proxy for the
/// parser dispatch overhead without touching the TUI or LLM.
fn bench_chat_help(c: &mut Criterion) {
    let mut group = c.benchmark_group("cold_start/chat_help");
    group.sample_size(20);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(5));
    group.throughput(Throughput::Elements(1));

    group.bench_function(BenchmarkId::new("chat_help", "release"), |b| {
        b.iter(|| {
            // `chat --help` exits 0 from clap without entering the TUI code path.
            let out = Command::new(bin())
                .args(["chat", "--help"])
                .output()
                .expect("spawn");
            // clap exits 0 on --help.
            assert!(
                out.status.success() || out.status.code() == Some(2),
                "chat --help must terminate"
            );
        })
    });
    group.finish();
}

/// Benchmark the first-IO path: open the SQLite pool, write one row, exit.
/// This is the load-bearing budget — first-message latency in `chat` is
/// gated by the same pool-open + initial-write path.
fn bench_first_io(c: &mut Criterion) {
    let mut group = c.benchmark_group("cold_start/first_io_new");
    group.sample_size(20);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(8));
    group.throughput(Throughput::Elements(1));

    group.bench_function(BenchmarkId::new("new", "release"), |b| {
        b.iter_with_large_drop(|| {
            // Fresh tempdir per iteration so each sample is a true cold start
            // (no warm SQLite cache from a prior iteration).
            let home = tempdir().expect("tempdir");
            let out = Command::new(bin())
                .arg("new")
                .env("MSCODE_HOME", home.path())
                .output()
                .expect("spawn");
            // Force-drop the tempdir to release the file lock before the next
            // iteration begins.
            drop(home);
            out
        })
    });
    group.finish();
}

criterion_group!(benches, bench_version, bench_chat_help, bench_first_io);
criterion_main!(benches);
