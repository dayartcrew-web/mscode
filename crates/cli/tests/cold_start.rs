//! Informational cold-start micro-benchmark.
//!
//! Runs `mscode version` 5 times, measures wall-clock per invocation, and
//! prints the median. Phase 1 does NOT enforce a threshold (Phase 8 will
//! gate at <100ms); this test only fails if the binary cannot be invoked at
//! all. The median is printed to stderr so `cargo test -- --nocapture`
//! surfaces it.

use std::process::Command;
use std::time::{Duration, Instant};

fn mscode_path() -> String {
    std::env::var("CARGO_BIN_EXE_mscode")
        .expect("CARGO_BIN_EXE_mscode must be set by the cargo test harness")
}

fn measure_one(bin: &str) -> Duration {
    let start = Instant::now();
    let status = Command::new(bin).arg("version").status().expect("spawn");
    assert!(
        status.success(),
        "mscode version did not exit cleanly during bench"
    );
    start.elapsed()
}

fn median(values: &mut [Duration]) -> Duration {
    values.sort();
    let len = values.len();
    if len % 2 == 1 {
        values[len / 2]
    } else {
        (values[len / 2 - 1] + values[len / 2]) / 2
    }
}

#[test]
fn cold_start_median_is_reported() {
    let bin = mscode_path();
    // Warm-up run to keep the file-cache primed — not measured.
    let _ = measure_one(&bin);

    let mut samples: Vec<Duration> = (0..5).map(|_| measure_one(&bin)).collect();
    let med = median(&mut samples);

    eprintln!(
        "mscode cold-start median = {:.2} ms (samples = {:?})",
        med.as_secs_f64() * 1000.0,
        samples
            .iter()
            .map(|d| d.as_secs_f64() * 1000.0)
            .collect::<Vec<_>>()
    );

    // Phase 1 does not gate; just sanity-check that it ran in under 10 seconds
    // (a binary that takes that long is obviously broken regardless of the
    // 100ms target).
    assert!(
        med < Duration::from_secs(10),
        "median cold start absurdly slow"
    );
}
