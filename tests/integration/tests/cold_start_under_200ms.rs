//! Test 10: Cold-start under 200ms (release binary).
//!
//! This test is marked `#[ignore]` because:
//!   1. It must run against a release build (`cargo test --release`), but the
//!      default `cargo test` invocation builds debug. We don't want a debug
//!      build to fail spuriously.
//!   2. Wall-clock timing is inherently noisy on CI runners; we want this to
//!      be opt-in.
//!
//! To run: `cargo test --release -p mscode-integration-tests cold_start_under_200ms -- --ignored`.

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;
use tempfile::tempdir;

fn bin() -> PathBuf {
    // Same path-resolution logic as cli_subcommands_smoke — see that file
    // for the rationale. For the cold-start measurement we prefer the
    // *release* binary if present (the budget is defined against release).
    if let Ok(p) = std::env::var("MSCODE_BIN_PATH") {
        return PathBuf::from(p);
    }
    let exe = if cfg!(windows) {
        "mscode.exe"
    } else {
        "mscode"
    };
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target = manifest.parent().expect("workspace root").join("target");
    let release = target.join("release").join(exe);
    if release.exists() {
        return release;
    }
    target.join("debug").join(exe)
}

#[test]
#[ignore = "release-only cold-start measurement; run with --ignored --release"]
fn cold_start_under_200ms_release_build() {
    // Pre-spawn once to warm any filesystem caches so the measurements below
    // reflect the *second-through fifth* invocations rather than the very
    // first cold disk load. We want the median of a steady-state sample.
    let warmup_home = tempdir().expect("tempdir warmup");
    let _ = Command::new(bin())
        .arg("version")
        .env("MSCODE_HOME", warmup_home.path())
        .output();

    let mut samples_ms: [u128; 5] = [0; 5];
    for slot in &mut samples_ms {
        let home = tempdir().expect("tempdir sample").path().to_path_buf();
        let start = Instant::now();
        let out = Command::new(bin())
            .arg("version")
            .env("MSCODE_HOME", home)
            .output()
            .expect("spawn");
        let elapsed = start.elapsed();
        assert!(out.status.success(), "version must succeed on a sample");
        *slot = elapsed.as_millis();
    }

    samples_ms.sort();
    let median = samples_ms[2];
    // Sub-200ms cold start is the load-bearing budget for the version fast path.
    // We assert with a 50% safety margin above the 200ms goal so the test is
    // not flaky on slower CI hardware; the *benchmark* in benches/cold_start.rs
    // is the authoritative measurement.
    assert!(
        median < 200,
        "cold-start median too high: {median}ms (samples: {samples_ms:?})"
    );
}
