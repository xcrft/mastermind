use mmcg::indexer::Indexer;
use mmcg::store::Store;
use serde::Serialize;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

#[derive(Serialize)]
struct RunMetrics {
    elapsed_ms: u128,
    peak_rss_kib: Option<u64>,
    files_indexed: u32,
    files_unchanged: u32,
    files_failed: u32,
}

#[derive(Serialize)]
struct BenchmarkReport {
    schema_version: u32,
    files: usize,
    symbols_per_file: usize,
    changed_files: usize,
    parse_batch_size: usize,
    cold: RunMetrics,
    warm: RunMetrics,
    incremental: RunMetrics,
}

fn main() {
    let files = env_usize("MMCG_BENCH_FILES", 1_000);
    let symbols_per_file = env_usize("MMCG_BENCH_SYMBOLS_PER_FILE", 20);
    let changed_files = env_usize("MMCG_BENCH_CHANGED_FILES", files.div_ceil(10)).min(files);
    let temp = tempfile::tempdir().expect("create benchmark workspace");
    let source_root = temp.path().join("src");
    std::fs::create_dir_all(&source_root).expect("create source directory");
    write_fixture(&source_root, files, symbols_per_file);

    let db_path = temp.path().join(".mastermind/bench.db");
    let mut store = Store::open(&db_path).expect("open benchmark index");
    let indexer = Indexer::new(temp.path());

    let cold = measure_run(|| indexer.index_all(&mut store, false).expect("cold index"));
    let warm = measure_run(|| indexer.index_all(&mut store, false).expect("warm index"));

    for index in 0..changed_files {
        let path = source_root.join(format!("file_{index:05}.rs"));
        let mut body = fixture_body(index, symbols_per_file);
        writeln!(body, "pub fn changed_{index}() -> usize {{ {index} }}").unwrap();
        std::fs::write(&path, body).expect("update benchmark source");
        std::fs::File::options()
            .write(true)
            .open(&path)
            .expect("open changed source")
            .set_modified(SystemTime::now() + Duration::from_secs(2))
            .expect("advance source mtime");
    }
    let incremental = measure_run(|| {
        indexer
            .index_all(&mut store, false)
            .expect("incremental index")
    });

    let report = BenchmarkReport {
        schema_version: 1,
        files,
        symbols_per_file,
        changed_files,
        parse_batch_size: mmcg::indexer::PARSE_BATCH_SIZE,
        cold,
        warm,
        incremental,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("serialize benchmark report")
    );
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn write_fixture(root: &Path, files: usize, symbols_per_file: usize) {
    for index in 0..files {
        std::fs::write(
            root.join(format!("file_{index:05}.rs")),
            fixture_body(index, symbols_per_file),
        )
        .expect("write benchmark source");
    }
}

fn fixture_body(file_index: usize, symbols_per_file: usize) -> String {
    let mut body = String::new();
    for symbol_index in 0..symbols_per_file {
        writeln!(
            body,
            "pub fn symbol_{file_index}_{symbol_index}() -> usize {{ {symbol_index} }}"
        )
        .unwrap();
    }
    body
}

fn measure_run<F>(run: F) -> RunMetrics
where
    F: FnOnce() -> mmcg::indexer::IndexStats,
{
    let running = Arc::new(AtomicBool::new(true));
    let peak = Arc::new(AtomicU64::new(current_rss_kib().unwrap_or(0)));
    let sampler_running = Arc::clone(&running);
    let sampler_peak = Arc::clone(&peak);
    let sampler = std::thread::spawn(move || {
        while sampler_running.load(Ordering::Relaxed) {
            if let Some(rss) = current_rss_kib() {
                sampler_peak.fetch_max(rss, Ordering::Relaxed);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    });

    let started = Instant::now();
    let stats = run();
    let elapsed = started.elapsed();
    running.store(false, Ordering::Relaxed);
    sampler.join().expect("join RSS sampler");
    if let Some(rss) = current_rss_kib() {
        peak.fetch_max(rss, Ordering::Relaxed);
    }
    let peak_rss = peak.load(Ordering::Relaxed);

    RunMetrics {
        elapsed_ms: elapsed.as_millis(),
        peak_rss_kib: (peak_rss > 0).then_some(peak_rss),
        files_indexed: stats.files_indexed,
        files_unchanged: stats.files_unchanged,
        files_failed: stats.files_failed,
    }
}

#[cfg(target_os = "linux")]
fn current_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

#[cfg(target_os = "macos")]
fn current_rss_kib() -> Option<u64> {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    String::from_utf8(output.stdout).ok()?.trim().parse().ok()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn current_rss_kib() -> Option<u64> {
    None
}
