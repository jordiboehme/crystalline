#![allow(dead_code)] // removed by Task 8, which wires the callers
//! Where a harness gets its skills, and the rebalance between harnesses.
//!
//! Some harnesses read more than one user skills folder. Cursor reads
//! `~/.claude/skills` besides the `~/.agents/skills` it writes, so with
//! Claude Code installed too it would show every skill twice. Such a harness
//! is *covered* when another installed harness already writes one of the
//! extra folders it reads: it writes no files of its own and records one
//! marker entry, `covered:<by harness id>`, in its receipt skills list. A
//! folder several harnesses write (Codex, Gemini CLI and Cursor all write
//! `~/.agents/skills`) is shared by reference count: its files go only with
//! the last row that writes it.
//!
//! The rules here are pure: they look at the receipt rows and at paths
//! resolved under the given [`Folders`], never at the disk. Only
//! [`apply_rebalance`] touches files, and only for a planned step, so the
//! rebalance on a receipt with nothing to do costs no file access at all.
//!
//! Only user-scope rows take part, and only a harness whose profile reads a
//! folder other than the one it writes can be covered. The three legacy
//! harnesses read exactly the folder they write, so they are never covered,
//! never handed over and never rebalanced.

use std::path::{Path, PathBuf};

use crystalline_core::{HarnessKind, PathSpec, config};

use crate::install;
use crate::receipt::{self, InstallRecord, Receipt, RecordedSkill};

/// The name prefix of the marker entry a covered row records in its skills
/// list, followed by the id of the harness whose folder covers it. The colon
/// makes the marker fail `is_plain_skill_name`, so every path that reaches
/// the filesystem skips it.
pub(crate) const COVERED_PREFIX: &str = "covered:";

/// The two roots every user skills folder resolves under. Production builds
/// them once per run with [`real_roots`]; tests pass temporary folders.
pub(crate) struct Folders<'a> {
    pub home: &'a Path,
    pub copilot_home: &'a Path,
}

/// This user's real home folder and Copilot home, resolved the same way
/// `harness_paths` resolves the legacy rows' skills folders.
pub(crate) fn real_roots() -> (PathBuf, PathBuf) {
    (
        config::expand_tilde("~"),
        PathSpec::copilot_home("").resolve(),
    )
}

/// Where one harness gets its skills.
#[derive(Debug)]
pub(crate) enum Placement {
    /// Write the managed set into this folder (the harness's own one).
    Write(PathBuf),
    /// Write nothing: `folder`, which the harness also reads, is written by
    /// the `by` harness's row.
    Covered { folder: PathBuf, by: HarnessKind },
}

/// One change the rebalance makes to a row.
#[derive(Debug)]
pub(crate) enum RebalanceStep {
    /// The row became covered by `by`: drop its copies from `drop_from`
    /// (`None` when another row still writes that folder) and record the
    /// marker.
    Cover {
        harness: HarnessKind,
        by: HarnessKind,
        drop_from: Option<PathBuf>,
    },
    /// The row's cover went away: write the managed set into `to`, its own
    /// folder, and record the hashes in place of the marker.
    HandOver { harness: HarnessKind, to: PathBuf },
}

/// The harness whose folder covers a row, when `entry` is a covered marker
/// naming a harness this binary knows.
pub(crate) fn covered_by(entry: &RecordedSkill) -> Option<HarnessKind> {
    entry
        .name
        .strip_prefix(COVERED_PREFIX)
        .and_then(HarnessKind::from_id)
}

/// The marker entry recording that a row is covered by `by`.
pub(crate) fn covered_marker(by: HarnessKind) -> RecordedSkill {
    RecordedSkill {
        name: format!("{COVERED_PREFIX}{}", by.id()),
        sha256: String::new(),
    }
}

/// Whether a row carries a covered marker (any marker, a future harness id
/// included).
pub(crate) fn is_covered(row: &InstallRecord) -> bool {
    row.skills
        .iter()
        .any(|s| s.name.starts_with(COVERED_PREFIX))
}

/// Whether a row's list names at least one real skill file.
fn writes_files(row: &InstallRecord) -> bool {
    row.skills
        .iter()
        .any(|s| install::is_plain_skill_name(&s.name))
}

/// Whether a harness reads a user skills folder besides the one it writes,
/// the only way it can ever be covered.
fn can_be_covered(harness: HarnessKind) -> bool {
    let p = harness.profile();
    p.skills_reads.iter().any(|r| *r != p.skills_write)
}

fn write_folder(harness: HarnessKind, folders: &Folders<'_>) -> PathBuf {
    harness
        .profile()
        .skills_write
        .resolve_in(folders.home, folders.copilot_home)
}

/// The user-scope rows with skills installed that write files, in
/// `HarnessKind::ALL` order. The reference count: a folder's files belong to
/// these rows.
fn writers(book: &Receipt) -> impl Iterator<Item = (HarnessKind, &InstallRecord)> {
    HarnessKind::ALL.into_iter().filter_map(move |h| {
        book.find(h.id(), "user", None)
            .filter(|r| r.parts.skills && writes_files(r))
            .map(|r| (h, r))
    })
}

/// The user-scope rows that can cover another harness, in
/// `HarnessKind::ALL` order: every row with its skills part installed that
/// is not itself covered, whatever its list holds. An emptied list means
/// the person removed every skill, which a covered harness must respect
/// too, so covering stops only when the row is uninstalled or its skills
/// part removed.
fn covers(book: &Receipt) -> impl Iterator<Item = (HarnessKind, &InstallRecord)> {
    HarnessKind::ALL.into_iter().filter_map(move |h| {
        book.find(h.id(), "user", None)
            .filter(|r| r.parts.skills && !is_covered(r))
            .map(|r| (h, r))
    })
}

/// Where `harness` gets its skills given the rows in `book`: covered when a
/// folder it reads, other than its own write folder, is the write folder of
/// another user-scope row with skills installed that is not covered itself;
/// its own folder otherwise.
pub(crate) fn placement(harness: HarnessKind, book: &Receipt, folders: &Folders<'_>) -> Placement {
    let own = write_folder(harness, folders);
    if can_be_covered(harness) {
        let reads: Vec<PathBuf> = harness
            .profile()
            .skills_reads
            .iter()
            .map(|r| r.resolve_in(folders.home, folders.copilot_home))
            .filter(|p| *p != own)
            .collect();
        for (other, _) in covers(book) {
            if other == harness {
                continue;
            }
            let theirs = write_folder(other, folders);
            if reads.contains(&theirs) {
                return Placement::Covered {
                    folder: theirs,
                    by: other,
                };
            }
        }
    }
    Placement::Write(own)
}

/// Whether a user-scope row other than `except`'s, with skills installed and
/// files written, writes `folder`. The reference count behind uninstall: a
/// folder's files go only when this is false.
pub(crate) fn folder_written_by_others(
    folder: &Path,
    book: &Receipt,
    except: HarnessKind,
    folders: &Folders<'_>,
) -> bool {
    writers(book).any(|(h, _)| h != except && write_folder(h, folders) == folder)
}

/// The steps that bring every user-scope row that can be covered in line
/// with the others, in `HarnessKind::ALL` order. Pure: it reads the rows and
/// resolves paths, nothing else. Each step is planned against the rows as
/// the earlier steps leave them, so two rows sharing a folder that both
/// become covered do not each keep the files for the other.
pub(crate) fn rebalance_plan(book: &Receipt, folders: &Folders<'_>) -> Vec<RebalanceStep> {
    // The common case, every legacy-only machine: nothing can be covered,
    // so nothing is copied or resolved.
    if !HarnessKind::ALL
        .into_iter()
        .any(|h| can_be_covered(h) && book.find(h.id(), "user", None).is_some())
    {
        return Vec::new();
    }
    let mut sim = book.clone();
    let mut steps = Vec::new();
    for harness in HarnessKind::ALL {
        if !can_be_covered(harness) {
            continue;
        }
        let Some(row) = sim.find(harness.id(), "user", None) else {
            continue;
        };
        if !row.parts.skills {
            continue;
        }
        let mut next = row.clone();
        match placement(harness, &sim, folders) {
            Placement::Covered { by, .. } if writes_files(row) => {
                let own = write_folder(harness, folders);
                let drop_from =
                    (!folder_written_by_others(&own, &sim, harness, folders)).then_some(own);
                steps.push(RebalanceStep::Cover {
                    harness,
                    by,
                    drop_from,
                });
                next.skills = vec![covered_marker(by)];
            }
            // A marker naming a harness this binary does not know came from
            // a newer binary; handing it over here would only fight that
            // binary's next cover, so it is left alone.
            Placement::Write(to)
                if !writes_files(row) && row.skills.iter().any(|s| covered_by(s).is_some()) =>
            {
                steps.push(RebalanceStep::HandOver { harness, to });
                // Stands in for the hashes the hand-over records.
                next.skills = vec![RecordedSkill {
                    name: "handed-over".to_string(),
                    sha256: String::new(),
                }];
            }
            _ => continue,
        }
        sim.upsert(next);
    }
    steps
}

/// The skill list a hand-over starts from: the list of the row that covered
/// `row` as it was when the cover went away. That is `departed` when the
/// cover was just uninstalled, or the cover's own row when it is still in
/// the receipt with its skills part removed. `None` when neither is known
/// (an older binary removed the cover's row).
fn cover_list<'b>(
    row: &InstallRecord,
    book: &'b Receipt,
    departed: Option<&'b InstallRecord>,
) -> Option<&'b [RecordedSkill]> {
    let by = row.skills.iter().find_map(covered_by)?;
    if let Some(d) = departed.filter(|d| d.harness == by.id() && d.scope == "user") {
        return Some(&d.skills);
    }
    book.find(by.id(), "user", None)
        .map(|r| r.skills.as_slice())
}

/// Write a handed-over row's skills into `to`, its own folder.
///
/// Only names the cover still listed are written (`seed`; every managed
/// skill when the cover's list is unknown), so a skill the person removed
/// stays removed. When other rows write `to` too, a name none of them lists
/// was removed from that shared folder and stays removed as well. A file
/// already there that equals the managed copy is adopted; one whose hash
/// matches a hash the cover or a co-writer recorded for that name is an old
/// clean copy and is updated in place; any other file is a person's edit and
/// is left alone, unrecorded, with a notice. No backup file is ever written
/// here.
fn hand_over(
    to: &Path,
    seed: Option<&[RecordedSkill]>,
    co_writers: &[&InstallRecord],
    notices: &mut Vec<String>,
) -> anyhow::Result<Vec<RecordedSkill>> {
    let wanted = |name: &str| {
        seed.is_none_or(|s| s.iter().any(|r| r.name == name))
            && (co_writers.is_empty()
                || co_writers
                    .iter()
                    .any(|w| w.skills.iter().any(|r| r.name == name)))
    };
    let clean = |name: &str, hash: &str| {
        seed.into_iter()
            .flatten()
            .chain(co_writers.iter().flat_map(|w| w.skills.iter()))
            .any(|r| r.name == name && r.sha256 == hash)
    };
    let mut records = Vec::new();
    for &(name, content) in install::managed_skills().iter() {
        if !wanted(name) {
            continue;
        }
        let path = to.join(name).join("SKILL.md");
        match std::fs::read(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                config::save_bytes(&path, content.as_bytes())?;
            }
            Err(e) => return Err(anyhow::anyhow!("could not read {}: {e}", path.display())),
            Ok(existing) if existing == content.as_bytes() => {}
            Ok(existing) if clean(name, &receipt::sha256_hex(&existing)) => {
                config::save_bytes(&path, content.as_bytes())?;
            }
            Ok(_) => {
                notices.push(format!("kept {name} in {}: it was edited", to.display()));
                continue;
            }
        }
        records.push(RecordedSkill {
            name: name.to_string(),
            sha256: receipt::sha256_hex(content.as_bytes()),
        });
    }
    Ok(records)
}

/// Carry out planned steps on disk and in `book`. `departed` is the row an
/// uninstall just removed, whose skill list seeds a hand-over it causes.
/// Returns whether any row changed and the notices to show; a step that
/// fails becomes a notice and leaves its row as it was, so the rebalance
/// never fails the run around it.
pub(crate) fn apply_rebalance(
    book: &mut Receipt,
    steps: &[RebalanceStep],
    folders: &Folders<'_>,
    departed: Option<&InstallRecord>,
) -> (bool, Vec<String>) {
    let mut changed = false;
    let mut notices = Vec::new();
    for step in steps {
        let harness = match step {
            RebalanceStep::Cover { harness, .. } | RebalanceStep::HandOver { harness, .. } => {
                *harness
            }
        };
        let Some(mut row) = book.find(harness.id(), "user", None).cloned() else {
            continue;
        };
        match step {
            RebalanceStep::Cover { by, drop_from, .. } => {
                if let Some(dir) = drop_from {
                    match install::uninstall_skills(dir, &row.skills, false) {
                        Ok(report) => {
                            let by_folder = write_folder(*by, folders);
                            for name in report.kept_modified() {
                                notices.push(format!(
                                    "kept {name} in {}: it was edited; {} also reads {}, so it may show twice",
                                    dir.display(),
                                    harness.display_name(),
                                    by_folder.display()
                                ));
                            }
                        }
                        Err(e) => {
                            notices.push(format!(
                                "Could not remove the {} skills from {}: {e}",
                                harness.display_name(),
                                dir.display()
                            ));
                            continue;
                        }
                    }
                }
                row.skills = vec![covered_marker(*by)];
            }
            RebalanceStep::HandOver { to, .. } => {
                let seed = cover_list(&row, book, departed).map(<[RecordedSkill]>::to_vec);
                let co_writers: Vec<&InstallRecord> = writers(book)
                    .filter(|(h, _)| *h != harness && write_folder(*h, folders) == *to)
                    .map(|(_, r)| r)
                    .collect();
                match hand_over(to, seed.as_deref(), &co_writers, &mut notices) {
                    Ok(records) => row.skills = records,
                    Err(e) => {
                        notices.push(format!(
                            "Could not write the {} skills to {}: {e}",
                            harness.display_name(),
                            to.display()
                        ));
                        continue;
                    }
                }
            }
        }
        book.upsert(row);
        changed = true;
    }
    (changed, notices)
}

/// Plan and apply the rebalance in one call: what every install, uninstall
/// and session-start refresh ends with. An uninstall passes the row it just
/// removed as `departed`; everything else passes `None`.
pub(crate) fn rebalance(
    book: &mut Receipt,
    folders: &Folders<'_>,
    departed: Option<&InstallRecord>,
) -> (bool, Vec<String>) {
    let steps = rebalance_plan(book, folders);
    if steps.is_empty() {
        return (false, Vec::new());
    }
    apply_rebalance(book, &steps, folders, departed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::receipt::{InstallRecord, Parts, Receipt, RecordedSkill};

    fn row(h: &str, skills: bool, recorded: usize) -> InstallRecord {
        InstallRecord {
            harness: h.into(),
            scope: "user".into(),
            project_path: None,
            version: "0.22.1".into(),
            parts: Parts {
                mcp: true,
                hooks: true,
                skills,
            },
            skills: (0..recorded)
                .map(|i| RecordedSkill {
                    name: format!("s{i}"),
                    sha256: "x".into(),
                })
                .collect(),
        }
    }
    fn covered_row(h: &str, by: &str) -> InstallRecord {
        let mut r = row(h, true, 0);
        r.skills.push(RecordedSkill {
            name: format!("{COVERED_PREFIX}{by}"),
            sha256: String::new(),
        });
        r
    }
    fn book(rows: Vec<InstallRecord>) -> Receipt {
        let mut b = Receipt::default();
        for r in rows {
            b.upsert(r);
        }
        b
    }
    fn folders(t: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
        (t.join("home"), t.join("home/.copilot"))
    }

    #[test]
    fn claude_code_first_then_cursor_is_covered() {
        let t = tempfile::tempdir().unwrap();
        let (home, cop) = folders(t.path());
        let f = Folders {
            home: &home,
            copilot_home: &cop,
        };
        let b = book(vec![row("claude-code", true, 4)]);
        assert!(matches!(placement(HarnessKind::Cursor, &b, &f),
            Placement::Covered { ref folder, by: HarnessKind::ClaudeCode } if *folder == home.join(".claude/skills")));
        assert!(
            matches!(placement(HarnessKind::Gemini, &b, &f), Placement::Write(ref p) if *p == home.join(".agents/skills"))
        );
        // Ruling R2: an emptied list means the person removed every skill,
        // which Cursor respects too, so the row still covers.
        let b = book(vec![row("claude-code", true, 0)]);
        assert!(
            matches!(
                placement(HarnessKind::Cursor, &b, &f),
                Placement::Covered {
                    by: HarnessKind::ClaudeCode,
                    ..
                }
            ),
            "a Claude Code row whose skills were all removed still covers"
        );
        let b = book(vec![row("claude-code", false, 4)]);
        assert!(
            matches!(placement(HarnessKind::Cursor, &b, &f), Placement::Write(_)),
            "a row without its skills part covers nothing"
        );
        let b = book(vec![covered_row("claude-code", "cursor")]);
        assert!(
            matches!(placement(HarnessKind::Cursor, &b, &f), Placement::Write(_)),
            "a covered row covers nothing"
        );
    }

    /// Decision 4.
    #[test]
    fn codex_then_cursor_share_agents_skills_by_reference_count() {
        let t = tempfile::tempdir().unwrap();
        let (home, cop) = folders(t.path());
        let f = Folders {
            home: &home,
            copilot_home: &cop,
        };
        let b = book(vec![row("codex", true, 4)]);
        assert!(
            matches!(placement(HarnessKind::Cursor, &b, &f), Placement::Write(ref p) if *p == home.join(".agents/skills")),
            "its own writes folder, never covered by it"
        );
        let b = book(vec![row("codex", true, 4), row("cursor", true, 4)]);
        assert!(folder_written_by_others(
            &home.join(".agents/skills"),
            &b,
            HarnessKind::Cursor,
            &f
        ));
        assert!(folder_written_by_others(
            &home.join(".agents/skills"),
            &b,
            HarnessKind::Codex,
            &f
        ));
        let b = book(vec![row("cursor", true, 4)]);
        assert!(!folder_written_by_others(
            &home.join(".agents/skills"),
            &b,
            HarnessKind::Cursor,
            &f
        ));
        let b = book(vec![
            row("codex", true, 4),
            covered_row("cursor", "claude-code"),
        ]);
        assert!(
            !folder_written_by_others(&home.join(".agents/skills"), &b, HarnessKind::Codex, &f),
            "a covered row writes nothing"
        );
    }

    /// Decision 3, the order the spec misses.
    #[test]
    fn cursor_first_then_claude_code_plans_a_cover_that_drops_cursors_copies() {
        let t = tempfile::tempdir().unwrap();
        let (home, cop) = folders(t.path());
        let f = Folders {
            home: &home,
            copilot_home: &cop,
        };
        let b = book(vec![row("cursor", true, 4), row("claude-code", true, 4)]);
        let plan = rebalance_plan(&b, &f);
        assert!(
            matches!(plan.as_slice(), [RebalanceStep::Cover { harness: HarnessKind::Cursor, by: HarnessKind::ClaudeCode, drop_from: Some(p) }] if *p == home.join(".agents/skills"))
        );
        let b = book(vec![
            row("codex", true, 4),
            row("cursor", true, 4),
            row("claude-code", true, 4),
        ]);
        assert!(
            matches!(
                rebalance_plan(&b, &f).as_slice(),
                [RebalanceStep::Cover {
                    drop_from: None,
                    ..
                }]
            ),
            "Codex still writes ~/.agents/skills, so nothing is dropped there"
        );
    }

    #[test]
    fn a_covered_row_whose_cover_left_is_handed_over() {
        let t = tempfile::tempdir().unwrap();
        let (home, cop) = folders(t.path());
        let f = Folders {
            home: &home,
            copilot_home: &cop,
        };
        let b = book(vec![covered_row("cursor", "claude-code")]);
        assert!(
            matches!(rebalance_plan(&b, &f).as_slice(), [RebalanceStep::HandOver { harness: HarnessKind::Cursor, to }] if *to == home.join(".agents/skills"))
        );
        assert!(
            rebalance_plan(
                &book(vec![
                    row("claude-code", true, 4),
                    covered_row("cursor", "claude-code")
                ]),
                &f
            )
            .is_empty(),
            "still covered: nothing to do"
        );
    }

    /// The Auto-mode state that an empty list already means.
    #[test]
    fn a_row_whose_user_removed_every_skill_is_not_handed_over() {
        let t = tempfile::tempdir().unwrap();
        let (home, cop) = folders(t.path());
        let f = Folders {
            home: &home,
            copilot_home: &cop,
        };
        assert!(rebalance_plan(&book(vec![row("cursor", true, 0)]), &f).is_empty());
    }

    /// Review M3: a marker from a newer binary names a harness this one does
    /// not know; handing it over would fight that binary's next cover.
    #[test]
    fn a_marker_naming_an_unknown_harness_is_left_alone() {
        let t = tempfile::tempdir().unwrap();
        let (home, cop) = folders(t.path());
        let f = Folders {
            home: &home,
            copilot_home: &cop,
        };
        assert!(rebalance_plan(&book(vec![covered_row("cursor", "zed")]), &f).is_empty());
    }

    #[test]
    fn a_legacy_only_machine_never_rebalances() {
        let t = tempfile::tempdir().unwrap();
        let (home, cop) = folders(t.path());
        let f = Folders {
            home: &home,
            copilot_home: &cop,
        };
        for rows in [
            vec![
                row("claude-code", true, 4),
                row("codex", true, 4),
                row("copilot", true, 4),
            ],
            vec![
                row("claude-code", true, 0),
                row("codex", true, 0),
                row("copilot", true, 0),
            ],
        ] {
            assert!(rebalance_plan(&book(rows), &f).is_empty());
        }
    }

    /// The gate behind the session-start no-op path: the plan takes only the
    /// rows and the two roots, and decides without the disk. Roots below a
    /// regular file can never exist, yet the plan is the same as anywhere.
    #[test]
    fn the_plan_decides_from_rows_and_paths_alone() {
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join("a-file");
        std::fs::write(&file, "").unwrap();
        let (home, cop) = (file.join("home"), file.join("copilot"));
        let f = Folders {
            home: &home,
            copilot_home: &cop,
        };
        let legacy = book(vec![
            row("claude-code", true, 4),
            row("codex", true, 4),
            row("copilot", true, 4),
        ]);
        assert!(rebalance_plan(&legacy, &f).is_empty());
        let mixed = book(vec![row("cursor", true, 4), row("claude-code", true, 4)]);
        assert!(matches!(
            rebalance_plan(&mixed, &f).as_slice(),
            [RebalanceStep::Cover { .. }]
        ));
    }

    #[test]
    fn the_covered_marker_is_never_a_plain_skill_name() {
        assert!(!crate::install::is_plain_skill_name(&format!(
            "{COVERED_PREFIX}claude-code"
        )));
    }
}
