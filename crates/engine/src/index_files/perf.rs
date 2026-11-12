//! Measurements of the `index.md` refresh, run by hand in a release build:
//!
//! ```text
//! cargo test --release -p crystalline-engine perf_index_ -- --ignored --nocapture
//! CRYSTALLINE_PERF_ENGRAMS=20000 cargo test --release -p crystalline-engine perf_index_ -- --ignored --nocapture
//! ```
//!
//! The fixture is generated from a seed, so two runs measure the same domain.
//! Only the generator's determinism runs in the normal suite.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crystalline_core::ReflectionMode;

/// A small deterministic generator (splitmix64), so the fixture needs no
/// dependency and one seed always builds the same domain.
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    pub(crate) fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `0..n`; `n` must not be zero.
    pub(crate) fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const WORDS: &[&str] = &[
    "deploy",
    "rollback",
    "cluster",
    "review",
    "standard",
    "runbook",
    "metric",
    "alert",
    "budget",
    "release",
    "schema",
    "migration",
    "queue",
    "worker",
    "cache",
    "latency",
    "owner",
    "policy",
    "network",
    "storage",
    "backup",
    "restore",
    "incident",
    "pager",
    "service",
    "client",
    "token",
    "session",
    "folder",
    "engram",
];

const MANIFEST: &str = "---\ntype: manifest\ntitle: perf\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# perf\n\n## Scope\n\n- A generated domain for measurements\n\n## When to Use\n\n- Never, it is a fixture\n";

/// The target size of one generated engram, about what a real one weighs.
const ENGRAM_BYTES: usize = 2900;

fn words(rng: &mut Rng, n: usize) -> String {
    (0..n)
        .map(|_| WORDS[rng.below(WORDS.len())])
        .collect::<Vec<_>>()
        .join(" ")
}

/// One generated engram's text: frontmatter with a title and a description,
/// then a body of about [`ENGRAM_BYTES`] in all.
pub(crate) fn perf_engram(rng: &mut Rng, i: usize) -> String {
    let mut out = format!(
        "---\ntype: engram\ntitle: Note {i} on {}\ndescription: {}\ntags:\n  - {}\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Note {i}\n\n",
        words(rng, 2),
        words(rng, 8),
        WORDS[rng.below(WORDS.len())],
    );
    while out.len() < ENGRAM_BYTES {
        out.push_str(&words(rng, 12));
        out.push_str(".\n\n");
    }
    out
}

/// Fill `root` with a MANIFEST and `engrams` generated engrams, spread over
/// `folders` flat folders plus one chain `deep/l1/.../l<depth>`. Returns the
/// domain-relative paths of the engrams, in the order they were made.
pub(crate) fn perf_fixture(
    root: &Path,
    engrams: usize,
    folders: usize,
    depth: usize,
    seed: u64,
) -> Vec<String> {
    let mut rng = Rng::new(seed);
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("MANIFEST.md"), MANIFEST).unwrap();
    let mut places: Vec<String> = (0..folders).map(|f| format!("topic-{f:02}")).collect();
    let mut deep = String::from("deep");
    for level in 1..=depth {
        deep.push_str(&format!("/l{level}"));
        places.push(deep.clone());
    }
    let mut made = Vec::with_capacity(engrams);
    for i in 0..engrams {
        let folder = &places[rng.below(places.len())];
        let rel = format!("{folder}/note-{i:05}.md");
        let path = root.join(&rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, perf_engram(&mut rng, i)).unwrap();
        made.push(rel);
    }
    made
}

/// Give the engram at `rel` a new title the way the engine writes a file:
/// a sibling temp file renamed over it.
pub(crate) fn retitle(root: &Path, rel: &str, round: usize) {
    let path = root.join(rel);
    let text = std::fs::read_to_string(&path).unwrap();
    let (head, rest) = text.split_once("\ntitle: ").unwrap();
    let (_, tail) = rest.split_once('\n').unwrap();
    let text = format!("{head}\ntitle: Retitled in round {round}\n{tail}");
    let tmp = path.with_file_name(".perf-retitle.tmp");
    std::fs::write(&tmp, text).unwrap();
    std::fs::rename(&tmp, &path).unwrap();
}

pub(crate) fn engram_count() -> usize {
    std::env::var("CRYSTALLINE_PERF_ENGRAMS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000)
}

pub(crate) fn rounds() -> usize {
    std::env::var("CRYSTALLINE_PERF_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20)
}

/// The resident set size of this process in kilobytes, from `ps`, where it
/// can be read.
pub(crate) fn rss_kb() -> Option<u64> {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// p50, p90 and the maximum of a set of durations, in milliseconds.
pub(crate) fn summary(label: &str, mut times: Vec<Duration>) -> f64 {
    times.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let at = |q: f64| times[((times.len() as f64 - 1.0) * q).round() as usize];
    let p50 = ms(at(0.5));
    println!(
        "{label}: p50 {p50:.2} ms, p90 {:.2} ms, max {:.2} ms over {} rounds",
        ms(at(0.9)),
        ms(*times.last().unwrap()),
        times.len()
    );
    p50
}

#[test]
fn perf_fixture_is_deterministic_for_a_seed() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let c = tempfile::tempdir().unwrap();
    let made_a = perf_fixture(a.path(), 40, 5, 3, 7);
    let made_b = perf_fixture(b.path(), 40, 5, 3, 7);
    let made_c = perf_fixture(c.path(), 40, 5, 3, 8);
    assert_eq!(made_a, made_b);
    for rel in &made_a {
        let text = std::fs::read_to_string(a.path().join(rel)).unwrap();
        assert_eq!(text, std::fs::read_to_string(b.path().join(rel)).unwrap());
        assert!(text.len() >= ENGRAM_BYTES);
        crystalline_core::parse_engram(&text).unwrap();
    }
    assert!(made_a.iter().any(|rel| rel.starts_with("deep/l1/l2/l3/")));
    let texts = |dir: &Path, made: &[String]| -> Vec<String> {
        made.iter()
            .map(|rel| std::fs::read_to_string(dir.join(rel)).unwrap())
            .collect()
    };
    assert_ne!(
        (made_a.clone(), texts(a.path(), &made_a)),
        (made_c.clone(), texts(c.path(), &made_c)),
        "another seed builds another domain"
    );
}

/// One refresh pass alone, after one title change, as the write path runs
/// it.
#[test]
#[ignore = "a measurement, run by hand in a release build"]
fn perf_index_refresh() {
    let engrams = engram_count();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("perf");
    let made = perf_fixture(&root, engrams, 50, 8, 42);
    println!("perf_index_refresh: {engrams} engrams");

    let rss_before = rss_kb();
    let started = Instant::now();
    let first = super::refresh(&root, ReflectionMode::Off);
    println!(
        "rss before the first pass: {rss_before:?} kB, after: {:?} kB",
        rss_kb()
    );
    println!(
        "first full pass: {:.2} ms, {} written, {} files read, {} bytes read",
        started.elapsed().as_secs_f64() * 1000.0,
        first.written,
        first.read,
        first.bytes_read
    );

    let mut times = Vec::new();
    let mut reads = 0;
    let mut bytes = 0;
    for round in 0..rounds() {
        retitle(&root, &made[(round * 7919) % made.len()], round);
        let started = Instant::now();
        let report = super::refresh(&root, ReflectionMode::Off);
        times.push(started.elapsed());
        assert_eq!(report.written, 1, "one folder index changes per round");
        reads = report.read;
        bytes = report.bytes_read;
    }
    summary("full pass after one write", times);
    println!("full pass: {reads} files read, {bytes} bytes read per pass");

    // The same with the memo, the touched path forced as the engine does.
    let mut memo = super::ListingMemo::default();
    let rss_before = rss_kb();
    let started = Instant::now();
    let seeding = super::refresh_with(&root, ReflectionMode::Off, &mut memo, &[]);
    println!(
        "memo seeding pass: {:.2} ms, {} files read; memo {} entries, about {} bytes; rss {rss_before:?} kB before, {:?} kB after",
        started.elapsed().as_secs_f64() * 1000.0,
        seeding.read,
        memo.len(),
        memo.approx_bytes(),
        rss_kb()
    );
    let mut times = Vec::new();
    let mut worst_reads = 0;
    let mut bytes = 0;
    for round in 0..rounds() {
        let rel = &made[(round * 7919 + 13) % made.len()];
        retitle(&root, rel, 1000 + round);
        let started = Instant::now();
        let report = super::refresh_with(
            &root,
            ReflectionMode::Off,
            &mut memo,
            std::slice::from_ref(rel),
        );
        times.push(started.elapsed());
        assert_eq!(report.written, 1, "one folder index changes per round");
        worst_reads = worst_reads.max(report.read);
        bytes = report.bytes_read;
    }
    summary("memo pass after one write", times);
    println!("memo pass: at most {worst_reads} concept files read, {bytes} bytes read per pass");
}

fn write_params(i: usize) -> crate::params::WriteParams {
    crate::params::WriteParams {
        path: None,
        base64: None,
        domain: "perf".to_string(),
        title: format!("Perf write {i}"),
        content: Some(format!("A measured write, number {i}.")),
        folder: Some(format!("topic-{:02}", i % 50)),
        engram_type: None,
        tags: Vec::new(),
        status: None,
        metadata: None,
        overwrite: true,
        share_link: None,
        model: None,
        expected_checksum: None,
    }
}

/// `write_engram` end to end with the generated listings on and off; the
/// difference is what the listings cost on a write receipt. Then a burst of
/// writes sent at once with the listings on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a measurement, run by hand in a release build"]
async fn perf_index_write_engram() {
    use crate::engine::Engine;
    use crystalline_core::config::{DomainEntry, GlobalConfig, IndexConfig};
    use crystalline_index::TursoStore;

    let engrams = engram_count();
    println!("perf_index_write_engram: {engrams} engrams");
    let mut p50s = Vec::new();
    for files in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("perf");
        perf_fixture(&root, engrams, 50, 8, 42);
        let mut config = GlobalConfig::default();
        config
            .domains
            .insert("perf".to_string(), DomainEntry::file(root.clone()));
        config.index = Some(IndexConfig { files: Some(files) });
        let store = TursoStore::open_in_memory().await.unwrap();
        let engine = Arc::new(Engine::new(
            Arc::new(tokio::sync::Mutex::new(store)),
            config,
            None,
            None,
        ));
        let started = Instant::now();
        engine.sync(None).await.unwrap();
        println!(
            "index.files {files}: first sync {:.0} ms",
            started.elapsed().as_secs_f64() * 1000.0
        );
        if files {
            let started = Instant::now();
            engine.warm_index_files().await;
            println!(
                "index.files {files}: warm-up {:.2} ms",
                started.elapsed().as_secs_f64() * 1000.0
            );
        }
        // One write first, so the domain is warm before anything is timed.
        engine.write_engram(&write_params(1_000_000)).await.unwrap();
        let mut times = Vec::new();
        for i in 0..rounds() {
            let started = Instant::now();
            engine.write_engram(&write_params(i)).await.unwrap();
            times.push(started.elapsed());
        }
        p50s.push(summary(
            &format!("write_engram, index.files {files}"),
            times,
        ));

        if files {
            let started = Instant::now();
            let burst: Vec<_> = (0..20)
                .map(|i| {
                    let engine = engine.clone();
                    tokio::spawn(async move {
                        engine
                            .write_engram(&write_params(2_000_000 + i))
                            .await
                            .unwrap();
                    })
                })
                .collect();
            for task in burst {
                task.await.unwrap();
            }
            println!(
                "burst of 20 writes sent at once, index.files on: {:.0} ms in all",
                started.elapsed().as_secs_f64() * 1000.0
            );
        }
    }
    println!(
        "index cost on the write receipt (p50 on minus p50 off): {:.2} ms",
        p50s[1] - p50s[0]
    );
}
