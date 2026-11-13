//! The store concurrency measurements (read connections, F3), as ignored tests.
//!
//! Nothing here asserts a number: each scenario prints p50, p99 and max per
//! operation, and the numbers go into the release notes and the research
//! notes by hand. Run them in release mode:
//!
//! ```text
//! cargo test --release -p crystalline-service --test perf_store perf_store_ -- --ignored --nocapture
//! CRYSTALLINE_PERF_ENGRAMS=20000 CRYSTALLINE_PERF_SEARCHERS=8 cargo test --release ...
//! CRYSTALLINE_READ_CONNECTIONS=0 cargo test --release ...   # the baseline
//! ```
//!
//! Settings, all optional: `CRYSTALLINE_PERF_ENGRAMS` (default 2000),
//! `CRYSTALLINE_PERF_SEARCHERS` (the highest searcher count, default 8; the
//! runs use 1, 4 and 8 up to it), `CRYSTALLINE_PERF_QUERIES` (searches per
//! searcher, default 20), `CRYSTALLINE_PERF_SEED` (default 1). The Postgres
//! leg runs when `CRYSTALLINE_TEST_POSTGRES_URL` is set.
//!
//! The fixture is a file domain of N engrams in 50 folders, each with three
//! paragraphs of about 1.1 KB of pseudo words, so the chunker cuts three chunks
//! per engram. It is synced and embedded through the engine like a real
//! index, with a provider that turns a text into a fixed 384-dimension vector,
//! so a full reindex keeps every vector and the searches stay hybrid.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crystalline_core::config::{
    DomainEntry, EmbeddingsConfig, GlobalConfig, ResponseFormat, ServiceConfig,
};
use crystalline_index::{EmbeddingProvider, Store};
use crystalline_service::params::{ReadParams, SearchParams, WriteParams};
use crystalline_service::{Engine, Scope, SimilarProbe};
use tokio::sync::Mutex;

const DIMS: usize = 384;
const MODEL: &str = "perf-model";
const DOMAIN: &str = "perf";
const FOLDERS: usize = 50;

// --- the fixture --------------------------------------------------------------

/// A small seeded generator (xorshift64*), so a seed always gives the same
/// fixture without a dependency.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// The fixture's words: `count` pseudo words of three to nine letters.
fn vocabulary(seed: u64, count: usize) -> Vec<String> {
    let mut rng = Rng::new(seed ^ 0xA5A5);
    (0..count)
        .map(|_| {
            let len = 3 + rng.below(7);
            (0..len)
                .map(|_| (b'a' + rng.below(26) as u8) as char)
                .collect()
        })
        .collect()
}

/// A paragraph of about `bytes` bytes of words from `words`.
fn paragraph(rng: &mut Rng, words: &[String], bytes: usize) -> String {
    let mut out = String::with_capacity(bytes + 16);
    while out.len() < bytes {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&words[rng.below(words.len())]);
    }
    out.push('.');
    out
}

/// The path and text of engram `i` of the fixture.
fn engram_file(i: usize, rng: &mut Rng, words: &[String]) -> (String, String) {
    let title = format!(
        "Note {i} {} {}",
        words[rng.below(words.len())],
        words[rng.below(words.len())]
    );
    let tag = format!("tag{}", rng.below(20));
    let body = (0..3)
        .map(|_| paragraph(rng, words, 1100))
        .collect::<Vec<_>>()
        .join("\n\n");
    let text = format!(
        "---\ntitle: {title}\ntype: engram\nstatus: stable\npermalink: note-{i}\ntags:\n  - {tag}\n---\n\n{body}\n"
    );
    (format!("f{:02}/note-{i}.md", i % FOLDERS), text)
}

/// Every file of a fixture of `n` engrams for `seed`, in order.
fn fixture_files(seed: u64, n: usize) -> Vec<(String, String)> {
    let words = vocabulary(seed, 4000);
    let mut rng = Rng::new(seed);
    (0..n).map(|i| engram_file(i, &mut rng, &words)).collect()
}

/// Write the fixture domain under `root` and return its folder.
fn write_fixture(root: &Path, seed: u64, n: usize) -> PathBuf {
    let dir = root.join(DOMAIN);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: perf\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# perf\n\n## Scope\n\n- Measurements\n\n## When to Use\n\n- Never\n",
    )
    .unwrap();
    for (path, text) in fixture_files(seed, n) {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    dir
}

/// A provider that turns a text into a fixed unit vector: the same text gives
/// the same vector, and no model is loaded.
struct PerfEmbedder;

impl PerfEmbedder {
    fn embed_one(text: &str) -> Vec<f32> {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in text.as_bytes() {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        let mut rng = Rng::new(h);
        let v: Vec<f32> = (0..DIMS)
            .map(|_| (rng.next() >> 40) as f32 / (1u64 << 24) as f32 - 0.5)
            .collect();
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.into_iter().map(|x| x / norm).collect()
    }
}

#[async_trait::async_trait]
impl EmbeddingProvider for PerfEmbedder {
    async fn embed(&self, texts: &[String]) -> crystalline_index::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| PerfEmbedder::embed_one(t)).collect())
    }

    fn model_id(&self) -> &str {
        MODEL
    }

    fn dims(&self) -> usize {
        DIMS
    }

    fn max_input_tokens(&self) -> usize {
        512
    }
}

// --- settings and reports -------------------------------------------------------

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

struct Settings {
    engrams: usize,
    searchers: Vec<usize>,
    queries: usize,
    seed: u64,
}

impl Settings {
    fn from_env() -> Settings {
        let top = env_usize("CRYSTALLINE_PERF_SEARCHERS", 8).max(1);
        let mut searchers: Vec<usize> = [1, 4, 8].into_iter().filter(|&n| n <= top).collect();
        if !searchers.contains(&top) {
            searchers.push(top);
        }
        Settings {
            engrams: env_usize("CRYSTALLINE_PERF_ENGRAMS", 2000),
            searchers,
            queries: env_usize("CRYSTALLINE_PERF_QUERIES", 20),
            seed: env_usize("CRYSTALLINE_PERF_SEED", 1) as u64,
        }
    }
}

/// p50, p99 and max of `samples`, in milliseconds.
fn report(label: &str, samples: &mut [Duration]) {
    if samples.is_empty() {
        println!("{label:<52} n=0");
        return;
    }
    samples.sort();
    let at = |q: f64| {
        let i = ((samples.len() as f64 - 1.0) * q).round() as usize;
        samples[i].as_secs_f64() * 1000.0
    };
    println!(
        "{label:<52} n={:<4} p50={:>8.1} ms  p99={:>8.1} ms  max={:>8.1} ms",
        samples.len(),
        at(0.5),
        at(0.99),
        samples.last().unwrap().as_secs_f64() * 1000.0
    );
}

/// The resident set size of this process in MiB, from `ps`.
fn rss_mib() -> Option<f64> {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    let kib: f64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    Some(kib / 1024.0)
}

// --- the engine -------------------------------------------------------------------

struct Rig {
    _tmp: tempfile::TempDir,
    engine: Arc<Engine>,
    db: Option<PathBuf>,
    words: Vec<String>,
    engrams: usize,
}

async fn rig(settings: &Settings, store: Arc<Mutex<dyn Store>>, db: Option<PathBuf>) -> Rig {
    let tmp = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let dir = write_fixture(tmp.path(), settings.seed, settings.engrams);
    let mut cfg = GlobalConfig {
        embeddings: Some(EmbeddingsConfig {
            provider: "remote".to_string(),
            model: MODEL.to_string(),
            endpoint: None,
            api_key_env: None,
        }),
        ..GlobalConfig::default()
    };
    cfg.domains
        .insert(DOMAIN.to_string(), DomainEntry::file(dir));
    cfg.service = Some(ServiceConfig {
        response_format: Some(ResponseFormat::Json),
        ..ServiceConfig::default()
    });
    let config_path = tmp.path().join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let shared = store.lock().await.shares_database();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut engine = Engine::new(
        store,
        cfg,
        Some(Arc::new(PerfEmbedder) as Arc<dyn EmbeddingProvider>),
        Some(config_path),
    )
    .with_embed_channel(tx);
    if shared {
        engine = engine.with_instance_id("this-machine".to_string());
    }
    let engine = Arc::new(engine);
    engine.sync(None).await.unwrap();
    let synced = started.elapsed();
    while engine.embed_pending().await.unwrap() > 0 {}
    println!(
        "fixture: {} engrams, written and synced in {:.1} s, embedded in {:.1} s",
        settings.engrams,
        synced.as_secs_f64(),
        (started.elapsed() - synced).as_secs_f64()
    );
    // The embed worker the writes of the write loop nudge, as in the daemon.
    tokio::spawn(crystalline_service::engine::run_embed_worker(
        engine.clone(),
        rx,
    ));
    Rig {
        _tmp: tmp,
        engine,
        db,
        words: vocabulary(settings.seed, 4000),
        engrams: settings.engrams,
    }
}

impl Rig {
    async fn search(&self, rng: &mut Rng) -> Duration {
        let query = format!(
            "{} {}",
            self.words[rng.below(self.words.len())],
            self.words[rng.below(self.words.len())]
        );
        let p = SearchParams {
            query: Some(query),
            domains: vec![DOMAIN.to_string()],
            search_type: Some("hybrid".to_string()),
            ..SearchParams::default()
        };
        let started = Instant::now();
        let out = self
            .engine
            .search_engrams(&p, &Scope::Unrestricted)
            .await
            .unwrap();
        let took = started.elapsed();
        assert_eq!(out["mode"], "hybrid", "the fixture searches stay hybrid");
        took
    }

    async fn read(&self, rng: &mut Rng) -> Duration {
        let p = ReadParams {
            identifier: format!("note-{}", rng.below(self.engrams)),
            domain: Some(DOMAIN.to_string()),
            ..ReadParams::default()
        };
        let started = Instant::now();
        self.engine
            .read_engram(&p, &Scope::Unrestricted)
            .await
            .unwrap();
        started.elapsed()
    }

    /// `searchers` tasks, each running `per` searches (or reads), all at once.
    async fn load(self: &Arc<Self>, searchers: usize, per: usize, reads: bool) -> Vec<Duration> {
        let tasks: Vec<_> = (0..searchers)
            .map(|s| {
                let rig = self.clone();
                tokio::spawn(async move {
                    let mut rng = Rng::new(1000 + s as u64);
                    let mut out = Vec::with_capacity(per);
                    for _ in 0..per {
                        out.push(if reads {
                            rig.read(&mut rng).await
                        } else {
                            rig.search(&mut rng).await
                        });
                    }
                    out
                })
            })
            .collect();
        let mut all = Vec::new();
        for t in tasks {
            all.extend(t.await.unwrap());
        }
        all
    }

    /// One `write_engram` every 200 ms with its `similar` probe, as the MCP
    /// write tool runs it, until `stop`. Returns the write and the probe
    /// latencies.
    fn write_loop(
        self: &Arc<Self>,
        stop: Arc<AtomicBool>,
        tag: &'static str,
    ) -> tokio::task::JoinHandle<(Vec<Duration>, Vec<Duration>)> {
        let rig = self.clone();
        tokio::spawn(async move {
            let mut rng = Rng::new(77);
            let (mut writes, mut probes) = (Vec::new(), Vec::new());
            let mut i = 0usize;
            while !stop.load(Ordering::Relaxed) {
                let body = paragraph(&mut rng, &rig.words, 1100);
                let title = format!("Written {tag} {i}");
                let p = WriteParams {
                    path: None,
                    base64: None,
                    domain: DOMAIN.to_string(),
                    title: title.clone(),
                    content: Some(body.clone()),
                    folder: Some("writes".to_string()),
                    engram_type: None,
                    tags: vec!["w".to_string()],
                    status: None,
                    metadata: None,
                    overwrite: true,
                    share_link: None,
                    model: None,
                    expected_checksum: None,
                };
                let started = Instant::now();
                let mut receipt = rig.engine.write_engram(&p).await.unwrap();
                let wrote = started.elapsed();
                rig.engine
                    .attach_similar(
                        &mut receipt,
                        SimilarProbe::Write {
                            title: &title,
                            description: None,
                            body: &body,
                        },
                        &Scope::Unrestricted,
                    )
                    .await;
                writes.push(wrote);
                probes.push(started.elapsed() - wrote);
                i += 1;
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            (writes, probes)
        })
    }
}

/// A legacy MCP session on the engine and a task that pings it every 20 ms
/// until `stop`, returning the round trips.
async fn ping_loop(
    engine: Arc<Engine>,
    stop: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<Vec<Duration>> {
    use rmcp::model::{ClientRequest, PingRequest};
    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    let server = tokio::spawn(async move {
        rmcp::serve_server(crystalline_service::mcp::McpServer::new(engine), server_io).await
    });
    let client = rmcp::serve_client((), client_io).await.unwrap();
    let server = server.await.unwrap().unwrap();
    tokio::spawn(async move {
        let _server = server;
        let mut out = Vec::new();
        while !stop.load(Ordering::Relaxed) {
            let started = Instant::now();
            client
                .peer()
                .send_request(ClientRequest::PingRequest(PingRequest::default()))
                .await
                .unwrap();
            out.push(started.elapsed());
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        out
    })
}

fn banner(backend: &str, settings: &Settings) {
    println!(
        "\n== perf_store {backend}: engrams={} queries/searcher={} searchers={:?} seed={} CRYSTALLINE_READ_CONNECTIONS={} workers={}",
        settings.engrams,
        settings.queries,
        settings.searchers,
        settings.seed,
        std::env::var("CRYSTALLINE_READ_CONNECTIONS").unwrap_or_else(|_| "(unset)".to_string()),
        tokio::runtime::Handle::current().metrics().num_workers(),
    );
}

/// Scenario 1: hybrid search on an idle store.
async fn scenario_idle(rig: &Arc<Rig>, settings: &Settings) {
    for &k in &settings.searchers {
        let mut s = rig.load(k, settings.queries, false).await;
        report(&format!("S1 search idle, {k} searchers"), &mut s);
    }
}

/// Scenario 2 (search beside a write loop), with the ping of scenario 7 and the
/// write latencies of scenario 5 at the highest searcher count.
async fn scenario_writes(rig: &Arc<Rig>, settings: &Settings, ping: bool) {
    for &k in &settings.searchers {
        let stop = Arc::new(AtomicBool::new(false));
        let writer = rig.write_loop(stop.clone(), "s2");
        let pinger = if ping {
            Some(ping_loop(rig.engine.clone(), stop.clone()).await)
        } else {
            None
        };
        tokio::time::sleep(Duration::from_millis(250)).await;
        let mut s = rig.load(k, settings.queries, false).await;
        stop.store(true, Ordering::Relaxed);
        let (mut writes, mut probes) = writer.await.unwrap();
        report(&format!("S2 search beside writes, {k} searchers"), &mut s);
        report(&format!("S5 write beside {k} searchers"), &mut writes);
        report(
            &format!("S5 similar probe beside {k} searchers"),
            &mut probes,
        );
        if let Some(pinger) = pinger {
            let mut pings = pinger.await.unwrap();
            report(&format!("S7 ping beside writes, {k} searchers"), &mut pings);
        }
    }
}

/// Scenario 5 alone: the write loop with no searcher, for 25 writes.
async fn scenario_writes_alone(rig: &Arc<Rig>) {
    let stop = Arc::new(AtomicBool::new(false));
    let writer = rig.write_loop(stop.clone(), "s5");
    tokio::time::sleep(Duration::from_millis(25 * 200)).await;
    stop.store(true, Ordering::Relaxed);
    let (mut writes, mut probes) = writer.await.unwrap();
    report("S5 write alone", &mut writes);
    report("S5 similar probe alone", &mut probes);
}

/// Scenario 3: search while full reindexes run back to back, then scenario 6.
async fn scenario_reindex(rig: &Arc<Rig>, settings: &Settings) {
    let stop = Arc::new(AtomicBool::new(false));
    let reindexer = {
        let rig = rig.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            let mut out = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                let started = Instant::now();
                rig.engine.reindex(true).await.unwrap();
                out.push(started.elapsed());
            }
            out
        })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    for &k in &settings.searchers {
        let mut s = rig.load(k, settings.queries, false).await;
        report(
            &format!("S3 search during full reindex, {k} searchers"),
            &mut s,
        );
    }
    stop.store(true, Ordering::Relaxed);
    let mut reindexes = reindexer.await.unwrap();
    report("S3 full reindex", &mut reindexes);
    // Scenario 6.
    let wal = rig.db.as_ref().and_then(|db| {
        let mut name = db.as_os_str().to_os_string();
        name.push("-wal");
        std::fs::metadata(PathBuf::from(name)).ok().map(|m| m.len())
    });
    println!(
        "S6 after the reindex runs: rss={} MiB wal={} MiB",
        rss_mib().map_or("?".to_string(), |r| format!("{r:.0}")),
        wal.map_or("-".to_string(), |w| format!(
            "{:.1}",
            w as f64 / (1 << 20) as f64
        ))
    );
}

/// Scenario 4: `read_engram` of random engrams beside the write loop.
async fn scenario_reads(rig: &Arc<Rig>, settings: &Settings) {
    for &k in &settings.searchers {
        let stop = Arc::new(AtomicBool::new(false));
        let writer = rig.write_loop(stop.clone(), "s4");
        tokio::time::sleep(Duration::from_millis(250)).await;
        let mut s = rig.load(k, settings.queries * 5, true).await;
        stop.store(true, Ordering::Relaxed);
        writer.await.unwrap();
        report(
            &format!("S4 read_engram beside writes, {k} readers"),
            &mut s,
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement: run it in release mode with --ignored --nocapture"]
async fn perf_store_turso() {
    let settings = Settings::from_env();
    banner("turso", &settings);
    let dbdir = tempfile::tempdir().unwrap();
    let db = dbdir.path().join("index.db");
    let store = crystalline_index::TursoStore::open(&db).await.unwrap();
    let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(store));
    let rig = Arc::new(rig(&settings, store, Some(db)).await);
    println!(
        "S6 after the fixture: rss={} MiB",
        rss_mib().map_or("?".to_string(), |r| format!("{r:.0}"))
    );
    scenario_idle(&rig, &settings).await;
    scenario_writes_alone(&rig).await;
    scenario_writes(&rig, &settings, true).await;
    scenario_reads(&rig, &settings).await;
    scenario_reindex(&rig, &settings).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement: run it in release mode with --ignored --nocapture"]
async fn perf_store_postgres() {
    let Some(url) = std::env::var("CRYSTALLINE_TEST_POSTGRES_URL")
        .ok()
        .filter(|u| !u.is_empty())
    else {
        eprintln!("note: CRYSTALLINE_TEST_POSTGRES_URL is unset; no postgres measurements");
        return;
    };
    let settings = Settings::from_env();
    banner("postgres", &settings);
    let schema = crystalline_index::ScratchSchema::new(&url, "perf_store");
    let store = crystalline_index::PostgresStore::open_in_schema(&url, schema.name())
        .await
        .unwrap();
    let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(store));
    let rig = Arc::new(rig(&settings, store, None).await);
    scenario_idle(&rig, &settings).await;
    scenario_writes_alone(&rig).await;
    scenario_writes(&rig, &settings, false).await;
    drop(rig);
    drop(schema);
}

#[test]
fn the_fixture_is_the_same_for_a_seed_and_differs_between_seeds() {
    let a = fixture_files(7, 30);
    let b = fixture_files(7, 30);
    let c = fixture_files(8, 30);
    assert_eq!(a, b);
    assert_ne!(a, c);
    assert_eq!(a[3].0, "f03/note-3.md");
    assert!(a[3].1.contains("permalink: note-3"));
    // Three paragraphs of about 1.1 KB: three chunks under the default budget.
    let body = a[3].1.split("---\n\n").nth(1).unwrap();
    assert_eq!(body.split("\n\n").count(), 3);
    assert!(body.len() > 3000 && body.len() < 3600, "{}", body.len());
    assert_eq!(PerfEmbedder::embed_one("x"), PerfEmbedder::embed_one("x"));
    assert_eq!(PerfEmbedder::embed_one("x").len(), DIMS);
}

// --- memory -----------------------------------------------------------------------

/// The WAL file's size in MiB.
fn wal_mib(db: &Path) -> f64 {
    let mut name = db.as_os_str().to_os_string();
    name.push("-wal");
    std::fs::metadata(PathBuf::from(name))
        .map(|m| m.len() as f64 / (1 << 20) as f64)
        .unwrap_or(0.0)
}

/// How many threads this process has, from `ps -M`.
fn threads() -> usize {
    std::process::Command::new("ps")
        .args(["-M", "-p", &std::process::id().to_string()])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .count()
                .saturating_sub(1)
        })
        .unwrap_or(0)
}

/// This process's physical footprint in MiB: the memory it dirtied and
/// still holds, without the clean pages the allocator gave back but the
/// kernel has not taken yet, which RSS still counts. macOS only.
#[cfg(target_os = "macos")]
fn footprint_mib() -> Option<f64> {
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
    // SAFETY: `proc_pid_rusage` writes one `rusage_info_v4` for the flavor
    // asked, into a buffer of exactly that size.
    let rc = unsafe {
        libc::proc_pid_rusage(
            std::process::id() as libc::c_int,
            libc::RUSAGE_INFO_V4,
            info.as_mut_ptr().cast(),
        )
    };
    // SAFETY: zeroed, then filled by the call on success.
    (rc == 0).then(|| unsafe { info.assume_init() }.ri_phys_footprint as f64 / (1 << 20) as f64)
}

#[cfg(not(target_os = "macos"))]
fn footprint_mib() -> Option<f64> {
    None
}

/// Memory over repeated full reindexes beside searchers, with the store's
/// read connections set by `CRYSTALLINE_PERF_READERS` (default 4, 0 for
/// none). Per round: the peak RSS while it ran, then RSS, the physical
/// footprint, the WAL and the threads once it settled; then once more after
/// `CRYSTALLINE_PERF_IDLE_SECS` (default 30) of idle. Fails when the WAL is
/// left over a few MiB after a round, when the footprint grows from round to
/// round, or when the threads are not bounded.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement: run it in release mode with --ignored --nocapture"]
async fn perf_store_memory() {
    let settings = Settings::from_env();
    let readers = env_usize("CRYSTALLINE_PERF_READERS", 4);
    let rounds = env_usize("CRYSTALLINE_PERF_ROUNDS", 3).max(1);
    let idle = env_usize("CRYSTALLINE_PERF_IDLE_SECS", 30);
    let searchers = *settings.searchers.last().unwrap();
    banner("turso memory", &settings);
    println!("readers={readers} rounds={rounds} searchers={searchers}");
    let dbdir = tempfile::tempdir().unwrap();
    let db = dbdir.path().join("index.db");
    let store = crystalline_index::TursoStore::open_with_readers(&db, readers)
        .await
        .unwrap();
    let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(store));
    let rig = Arc::new(rig(&settings, store, Some(db.clone())).await);
    let state = |label: &str| {
        let (rss, footprint, wal, threads) = (
            rss_mib().unwrap_or(0.0),
            footprint_mib().unwrap_or(0.0),
            wal_mib(&db),
            threads(),
        );
        println!(
            "{label:<24} rss={rss:>6.0} MiB  footprint={footprint:>6.0} MiB  wal={wal:>7.1} MiB  threads={threads}"
        );
        (footprint, wal, threads)
    };
    state("after the fixture");
    let mut footprints = Vec::new();
    for round in 1..=rounds {
        let stop = Arc::new(AtomicBool::new(false));
        let peak = {
            let stop = stop.clone();
            tokio::spawn(async move {
                let mut peak = 0f64;
                while !stop.load(Ordering::Relaxed) {
                    peak = peak.max(rss_mib().unwrap_or(0.0));
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                peak
            })
        };
        let reindex = {
            let rig = rig.clone();
            tokio::spawn(async move { rig.engine.reindex(true).await.unwrap() })
        };
        let searches = rig.load(searchers, settings.queries, false).await;
        reindex.await.unwrap();
        stop.store(true, Ordering::Relaxed);
        let peak = peak.await.unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        println!(
            "round {round}: {} searches, peak rss={peak:.0} MiB",
            searches.len()
        );
        let (footprint, wal, _) = state(&format!("after round {round}"));
        assert!(wal < 8.0, "round {round} left the WAL at {wal:.1} MiB");
        footprints.push(footprint);
    }
    tokio::time::sleep(Duration::from_secs(idle as u64)).await;
    let (_, _, threads) = state(&format!("after {idle} s idle"));
    if std::env::var("CRYSTALLINE_PERF_VMMAP").is_ok() {
        let out = std::process::Command::new("vmmap")
            .args(["--summary", &std::process::id().to_string()])
            .output()
            .unwrap();
        println!("{}", String::from_utf8_lossy(&out.stdout));
    }
    let (first, last) = (footprints[0], *footprints.last().unwrap());
    assert!(
        last <= first + 32.0,
        "the footprint grew from {first:.0} to {last:.0} MiB over {rounds} rounds"
    );
    assert!(threads <= 48, "{threads} threads after the idle hold");
}
