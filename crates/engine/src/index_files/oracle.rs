//! The reference test of the `index.md` refresh: a seeded random sequence of
//! changes to a domain, each followed by the refresh under test and then by
//! a plain full pass over the same folder, which must find nothing left to
//! write or remove. Whatever the refresh under test skipped or got wrong, the
//! full pass would put right, so a full pass that does nothing proves the two
//! agree byte for byte.
//!
//! The seed is printed on a failure; run more seeds by hand with
//! `CRYSTALLINE_ORACLE_SEEDS=50 cargo test -p crystalline-engine oracle -- --ignored`.

use std::path::Path;

use crystalline_core::ReflectionMode;

use super::perf::Rng;
use super::refresh;

/// The folders the sequence writes into: flat ones, a deep chain, the
/// domain's attachment folder, a folder the MANIFEST may declare as an
/// artifact folder and the Reflection's folder.
const FOLDERS: &[&str] = &[
    "",
    "a",
    "a/b",
    "a/b/c",
    "a/b/c/d/e/f/g/h",
    "notes",
    "notes/old",
    "skills",
    "skills/inner",
    "reflections",
    "reflections/guides",
    "assets",
    "assets/drafts",
];

const MANIFEST: &str = "---\ntype: manifest\ntitle: Oracle\ndescription: The oracle domain\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\n---\n\n# Oracle\n\n## Scope\n\n- s\n\n## When to Use\n\n- w\n";

const PROVISIONING: &str = "\n## Provisioning\n\n- skills: skills\n";

/// One concept's text, with its title and an optional description.
fn engram(title: &str, description: Option<&str>, body: &str) -> String {
    let mut out = format!("---\ntype: engram\ntitle: {title}\n");
    if let Some(d) = description {
        out.push_str(&format!("description: {d}\n"));
    }
    out.push_str(&format!("status: current\n---\n\n{body}\n"));
    out
}

/// The domain as the sequence built it: the concept files it made and the
/// Reflection mode in effect.
struct Domain<'a> {
    root: &'a Path,
    files: Vec<String>,
    mode: ReflectionMode,
    provisioned: bool,
    counter: usize,
}

impl Domain<'_> {
    fn write(&self, rel: &str, text: &str) {
        let path = self.root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // The engine's way: a sibling temp file renamed over the target.
        let tmp = path.with_file_name(".oracle.tmp");
        std::fs::write(&tmp, text).unwrap();
        std::fs::rename(&tmp, &path).unwrap();
    }

    /// Rewrite a file in place, as an editor that saves over the old bytes.
    fn overwrite(&self, rel: &str, text: &str) {
        std::fs::write(self.root.join(rel), text).unwrap();
    }

    fn fresh_name(&mut self, folder: &str) -> String {
        self.counter += 1;
        let name = format!("note-{}.md", self.counter);
        if folder.is_empty() {
            name
        } else {
            format!("{folder}/{name}")
        }
    }

    fn pick(&self, rng: &mut Rng) -> Option<String> {
        if self.files.is_empty() {
            None
        } else {
            Some(self.files[rng.below(self.files.len())].clone())
        }
    }

    fn manifest(&self) -> String {
        if self.provisioned {
            format!("{MANIFEST}{PROVISIONING}")
        } else {
            MANIFEST.to_string()
        }
    }

    /// Apply one random change and return the paths it touched, as the
    /// engine would pass them to the refresh.
    fn step(&mut self, rng: &mut Rng, round: usize) -> Vec<String> {
        let folder = FOLDERS[rng.below(FOLDERS.len())];
        match rng.below(16) {
            0..=3 => {
                let rel = self.fresh_name(folder);
                let description = (rng.below(2) == 0).then(|| format!("Made in round {round}"));
                self.write(
                    &rel,
                    &engram(&format!("Added {round}"), description.as_deref(), "Body"),
                );
                self.files.push(rel.clone());
                vec![rel]
            }
            4 => {
                let Some(rel) = self.pick(rng) else {
                    return Vec::new();
                };
                self.write(&rel, &engram(&format!("Retitled {round}"), None, "Body"));
                vec![rel]
            }
            5 => {
                let Some(rel) = self.pick(rng) else {
                    return Vec::new();
                };
                // In place, and a different length, as an editor saves.
                self.overwrite(
                    &rel,
                    &engram(
                        "Described",
                        Some(&format!("Now described\nover two lines in round {round}")),
                        "Body",
                    ),
                );
                vec![rel]
            }
            6 => {
                let Some(rel) = self.pick(rng) else {
                    return Vec::new();
                };
                let text = std::fs::read_to_string(self.root.join(&rel)).unwrap_or_default();
                self.write(&rel, &format!("{text}\nMore body in round {round}.\n"));
                vec![rel]
            }
            7 => {
                let Some(rel) = self.pick(rng) else {
                    return Vec::new();
                };
                std::fs::remove_file(self.root.join(&rel)).unwrap();
                self.files.retain(|f| f != &rel);
                vec![rel]
            }
            8 => {
                let Some(from) = self.pick(rng) else {
                    return Vec::new();
                };
                let to = self.fresh_name(folder);
                let target = self.root.join(&to);
                std::fs::create_dir_all(target.parent().unwrap()).unwrap();
                std::fs::rename(self.root.join(&from), &target).unwrap();
                self.files.retain(|f| f != &from);
                self.files.push(to.clone());
                vec![from, to]
            }
            9 => {
                // Rename a whole folder outside Crystalline.
                let candidates = ["notes", "a/b", "skills/inner", "notes-moved", "a/b-moved"];
                let from = candidates[rng.below(candidates.len())];
                let to = if let Some(base) = from.strip_suffix("-moved") {
                    base.to_string()
                } else {
                    format!("{from}-moved")
                };
                let (src, dst) = (self.root.join(from), self.root.join(&to));
                if !src.is_dir() || dst.exists() {
                    return Vec::new();
                }
                std::fs::rename(&src, &dst).unwrap();
                let mut touched = Vec::new();
                for f in &mut self.files {
                    if let Some(rest) = f.strip_prefix(&format!("{from}/")) {
                        touched.push(f.clone());
                        *f = format!("{to}/{rest}");
                        touched.push(f.clone());
                    }
                }
                touched
            }
            10 => {
                // Empty one folder.
                let gone: Vec<String> = self
                    .files
                    .iter()
                    .filter(|f| super::parent_of(f).unwrap_or_default() == folder)
                    .cloned()
                    .collect();
                for rel in &gone {
                    std::fs::remove_file(self.root.join(rel)).unwrap();
                }
                self.files.retain(|f| !gone.contains(f));
                gone
            }
            11 => {
                let rel = self.fresh_name(folder);
                if rng.below(2) == 0 {
                    self.write(&rel, "no frontmatter at all\n");
                } else {
                    self.write(&rel, "---\ntitle: [unclosed\n---\n\nBroken\n");
                }
                self.files.push(rel.clone());
                vec![rel]
            }
            12 => {
                let Some(rel) = self.pick(rng) else {
                    return Vec::new();
                };
                self.overwrite(&rel, "---\ntype: [broken\n");
                vec![rel]
            }
            13 => {
                self.provisioned = !self.provisioned;
                let text = self.manifest();
                self.write("MANIFEST.md", &text);
                vec!["MANIFEST.md".to_string()]
            }
            14 => {
                // A pending switch walks like no Reflection at all.
                self.mode = if rng.below(2) == 0 {
                    ReflectionMode::Off
                } else {
                    ReflectionMode::Effective
                };
                Vec::new()
            }
            _ => {
                // A page a person wrote, in a folder that may hold nothing.
                let rel = if folder.is_empty() {
                    "index.md".to_string()
                } else {
                    format!("{folder}/index.md")
                };
                if !self.root.join(&rel).exists() {
                    self.write(&rel, "# Our own page\n\nHello.\n");
                }
                Vec::new()
            }
        }
    }
}

/// Run `steps` random changes from `seed` with `under_test` refreshing after
/// each, and check after each one that a full pass has nothing left to do.
fn run(seed: u64, steps: usize, under_test: &mut dyn FnMut(&Path, ReflectionMode, &[String])) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut rng = Rng::new(seed);
    let mut domain = Domain {
        root,
        files: Vec::new(),
        mode: ReflectionMode::Off,
        provisioned: false,
        counter: 0,
    };
    domain.write("MANIFEST.md", &domain.manifest());
    domain.files.push("MANIFEST.md".to_string());
    for round in 0..steps {
        let touched = domain.step(&mut rng, round);
        under_test(root, domain.mode, &touched);
        let check = refresh(root, domain.mode);
        assert!(
            check.written == 0 && check.removed == 0,
            "seed {seed}, round {round}: the refresh under test left {} to write and {} to \
             remove (touched {touched:?}, mode {:?})",
            check.written,
            check.removed,
            domain.mode
        );
    }
}

/// The full pass agrees with itself: the sequence and the check are sound.
#[test]
fn oracle_full_pass_against_itself() {
    for seed in [1, 2, 3] {
        run(seed, 200, &mut |root, mode, _| {
            refresh(root, mode);
        });
    }
}
