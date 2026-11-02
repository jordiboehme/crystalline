//! The switch: the policy PATCH, its refusals and the reflection state.

use crystalline_service::Scope;

use crate::fixture::{engram, fixture, fixture_with, manifest, page};

#[tokio::test]
async fn reflection_home_is_never_set_by_hand() {
    let fx = fixture(&[]).await;
    let err = fx
        .engine
        .set_manifest_policies(
            "eng",
            &[("reflection_home".into(), "https://kb.example.com".into())],
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("filled in when Reflection is switched on"),
        "{err}"
    );
}

use crystalline_core::config::{OriginConfig, ServiceConfig};
use crystalline_service::params::EditParams;
use crystalline_service::{ReflectionCaller, ReflectionState};

fn on() -> Vec<(String, String)> {
    vec![("reflection".into(), "on".into())]
}

fn with_origin(cfg: &mut crystalline_core::config::GlobalConfig, url: Option<&str>) {
    let entry = cfg.domains.get_mut("eng").unwrap();
    entry.origin = Some(OriginConfig {
        repo: "acme/kb".into(),
        path: None,
        branch: None,
        poll_secs: None,
    });
    if let Some(url) = url {
        cfg.service = Some(ServiceConfig {
            public_url: Some(url.into()),
            ..Default::default()
        });
    }
}

#[tokio::test]
async fn switching_on_refuses_while_engrams_live_in_reflections_and_names_them() {
    let fx = fixture(&[
        (
            "reflections/old.md",
            &engram("Old", "reflections/old", "- [fact] old"),
        ),
        (
            "reflections/deep/older.md",
            &engram("Older", "reflections/deep/older", "- [fact] older"),
        ),
    ])
    .await;
    let err = fx
        .engine
        .set_manifest_policies("eng", &on(), &Scope::Unrestricted)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("reflections/old.md") && err.contains("reflections/deep/older.md"),
        "{err}"
    );
    assert!(err.contains("move_engram"), "{err}");
    assert_eq!(
        fx.engine.reflection_state("eng").await.unwrap(),
        ReflectionState::Off,
        "nothing was written"
    );
}

/// The refusal names exactly the files the one predicate calls pending
/// (R48): a page with a section id line under `reflections/` is the
/// Reflection's already, so it is neither named nor in the way.
#[tokio::test]
async fn the_switch_names_only_the_files_the_pending_predicate_names() {
    let fx = fixture(&[
        ("reflections/guide.md", &page("Guide")),
        (
            "reflections/old.md",
            &engram("Old", "reflections/old", "- [fact] old"),
        ),
    ])
    .await;
    let err = fx
        .engine
        .set_manifest_policies("eng", &on(), &Scope::Unrestricted)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("reflections/old.md"), "{err}");
    assert!(!err.contains("reflections/guide.md"), "{err}");

    let fx = fixture(&[("reflections/guide.md", &page("Guide"))]).await;
    fx.engine
        .set_manifest_policies("eng", &on(), &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        fx.engine.reflection_state("eng").await.unwrap(),
        ReflectionState::On {
            home: crystalline_service::ReflectionHome::Local
        }
    );
}

/// An engram whose frontmatter the line-preserving editor cannot cut (an
/// anchor and alias, a flow-style mapping) is still an engram under R48:
/// it is named by the switch-on refusal, and a switch that arrives out of
/// band stays pending while it is there.
#[tokio::test]
async fn an_engram_whose_frontmatter_cannot_be_cut_holds_the_switch() {
    let anchored = "---\ntype: note\nstatus: &s draft\nphase: *s\ntitle: Anchored\n---\n\n# Anchored\n\n## Part\n\nText.\n";
    let flow = "---\n{type: note, status: draft, title: Flow}\n---\n\n# Flow\n";
    let fx = fixture(&[
        ("reflections/anchored.md", anchored),
        ("reflections/flow.md", flow),
    ])
    .await;
    let err = fx
        .engine
        .set_manifest_policies("eng", &on(), &Scope::Unrestricted)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("reflections/anchored.md") && err.contains("reflections/flow.md"),
        "{err}"
    );
    std::fs::write(fx.root.join("MANIFEST.md"), manifest("reflection: on\n")).unwrap();
    fx.engine.sync(None).await.unwrap();
    let ReflectionState::Pending { engrams } = fx.engine.reflection_state("eng").await.unwrap()
    else {
        panic!("the switch waits for the engrams");
    };
    assert_eq!(
        engrams,
        vec!["reflections/anchored.md", "reflections/flow.md"]
    );
}

/// An engram whose frontmatter broke after it was indexed holds the switch
/// (R48a), and the refusals name it as a file that does not read as an
/// engram now, not as an engram to move.
#[tokio::test]
async fn a_broken_engram_is_named_as_a_file_to_fix_not_an_engram() {
    let fx = fixture(&[
        (
            "reflections/old.md",
            &engram("Old", "reflections/old", "- [fact] old"),
        ),
        (
            "reflections/kept.md",
            &engram("Kept", "reflections/kept", "- [fact] kept"),
        ),
    ])
    .await;
    std::fs::write(
        fx.root.join("reflections/old.md"),
        "---\n: [\n---\n\n# Old\n",
    )
    .unwrap();
    let err = fx
        .engine
        .set_manifest_policies("eng", &on(), &Scope::Unrestricted)
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        err,
        "These engrams live in reflections/, which Reflection would take over: reflections/kept.md. Move them to another folder first (move_engram). These files in reflections/ do not read as engrams now: reflections/old.md. Fix them so they read again (frontmatter that parses, UTF-8 text), then move them out. Then switch Reflection on"
    );
    std::fs::write(fx.root.join("MANIFEST.md"), manifest("reflection: on\n")).unwrap();
    fx.engine.sync(None).await.unwrap();
    let err = fx
        .engine
        .read_reflection_tree(
            "eng",
            &ReflectionCaller::person(Scope::Unrestricted, "jordi"),
        )
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        err,
        format!(
            "Reflection for 'eng' {}: reflections/kept.md. Reflection for 'eng' {}: reflections/old.md. Fix them so they read again (frontmatter that parses, UTF-8 text), then move them out",
            crystalline_service::engine::REFUSED_PENDING_NAMED,
            crystalline_service::engine::REFUSED_PENDING_UNREADABLE
        )
    );
}

#[tokio::test]
async fn a_manifest_edit_that_switches_on_is_screened_like_the_policy_patch() {
    let fx = fixture(&[(
        "reflections/old.md",
        &engram("Old", "reflections/old", "- [fact] old"),
    )])
    .await;
    // `set_frontmatter` takes only engram keys, so the switch is written
    // the way an agent writes a policy key into the MANIFEST by hand.
    let edit = EditParams {
        domain: "eng".into(),
        identifier: "manifest".into(),
        operation: "find_replace".into(),
        find_text: Some("recorded_at: 2026-01-01\n".into()),
        content: Some("recorded_at: 2026-01-01\nreflection: on\n".into()),
        ..Default::default()
    };
    let err = fx.engine.edit_engram(&edit).await.unwrap_err().to_string();
    assert!(err.contains("reflections/old.md"), "{err}");
    let source = std::fs::read_to_string(fx.root.join("MANIFEST.md")).unwrap();
    let markdown = source.replace(
        "recorded_at: 2026-01-01\n",
        "recorded_at: 2026-01-01\nreflection: on\n",
    );
    let checksum = crate::support::sha256_hex(source.as_bytes());
    let err = fx
        .engine
        .save_manifest("eng", &markdown, &checksum, &Scope::Unrestricted, false)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("reflections/old.md"), "{err}");
}

/// `configure` with a domain (MCP) reaches the same switch branch as the
/// policy PATCH (REST), so it refuses with the same sentence and writes
/// nothing; removing the key is a switch-off and always passes.
#[tokio::test]
async fn configure_switching_on_is_screened_like_the_policy_patch() {
    let fx = fixture(&[(
        "reflections/old.md",
        &engram("Old", "reflections/old", "- [fact] old"),
    )])
    .await;
    let rest = fx
        .engine
        .set_manifest_policies("eng", &on(), &Scope::Unrestricted)
        .await
        .unwrap_err()
        .to_string();
    let set = std::collections::BTreeMap::from([("reflection".to_string(), "on".to_string())]);
    let mcp = fx
        .engine
        .change_domain_settings("eng", &set, &[], &Scope::Unrestricted)
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(mcp, rest, "one sentence on both surfaces");
    assert!(mcp.contains("reflections/old.md"), "{mcp}");
    assert_eq!(
        fx.engine.reflection_state("eng").await.unwrap(),
        ReflectionState::Off,
        "nothing was written"
    );
    assert!(
        !std::fs::read_to_string(fx.root.join("MANIFEST.md"))
            .unwrap()
            .contains("reflection:")
    );

    // A domain whose MANIFEST says on (pending) switches off by removing the key.
    std::fs::write(fx.root.join("MANIFEST.md"), manifest("reflection: on\n")).unwrap();
    fx.engine.sync(None).await.unwrap();
    fx.engine
        .change_domain_settings(
            "eng",
            &Default::default(),
            &["reflection".to_string()],
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(
        fx.engine.reflection_switch("eng"),
        crystalline_core::ReflectionSwitch::Off
    );
}

#[tokio::test]
async fn a_team_domain_refuses_while_the_team_copy_still_holds_reflections() {
    // The owner moved the engram out locally and has not shared the move:
    // the base snapshot (the team's branch) still holds it under reflections/.
    let fx = fixture_with(
        &[("old.md", &engram("Old", "old", "- [fact] moved out"))],
        |cfg, _| with_origin(cfg, Some("https://kb.example.com")),
    )
    .await;
    let mut state = crystalline_remote::state::OriginState::new("acme/kb", "main");
    state.files.insert(
        "reflections/old.md".into(),
        crystalline_remote::state::BaseStamp {
            sha256: "x".into(),
            size: 1,
        },
    );
    state
        .save(&fx.tmp.path().join("origins").join("eng"))
        .unwrap();
    let err = fx
        .engine
        .set_manifest_policies("eng", &on(), &Scope::Unrestricted)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("reflections/old.md") && err.contains("share_changes"),
        "{err}"
    );
}

#[tokio::test]
async fn switching_on_a_virtual_domain_is_refused() {
    let fx = fixture_with(&[], |cfg, _| {
        cfg.domains.insert(
            "vd".into(),
            crystalline_core::config::DomainEntry::virtual_domain(),
        );
    })
    .await;
    let err = fx
        .engine
        .set_manifest_policies("vd", &on(), &Scope::Unrestricted)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("virtual"), "{err}");
}

#[tokio::test]
async fn a_local_domain_switches_on_with_no_home_key_and_seeds_its_pact() {
    let fx = fixture(&[]).await;
    fx.engine
        .set_manifest_policies("eng", &on(), &Scope::Unrestricted)
        .await
        .unwrap();
    let manifest_text = std::fs::read_to_string(fx.root.join("MANIFEST.md")).unwrap();
    assert!(manifest_text.contains("reflection: on") && !manifest_text.contains("reflection_home"));
    assert_eq!(
        fx.engine.reflection_state("eng").await.unwrap(),
        ReflectionState::On {
            home: crystalline_service::ReflectionHome::Local
        }
    );
    let pact = std::fs::read_to_string(fx.root.join("reflections/PACT.md")).unwrap();
    assert!(
        pact.contains("## Audience")
            && pact.contains("## Presentation")
            && pact.contains("## Structure"),
        "{pact}"
    );
    // The seeded pact carries its section ids, so an unedited pact never
    // holds the switch pending (R48): after a switch off and on again, or
    // after a rebuild of the index.
    assert!(
        !crystalline_core::is_pending_engram(
            &fx.root,
            "reflections/PACT.md",
            crystalline_core::EngramRow::Unknown
        ),
        "{pact}"
    );
}

#[tokio::test]
async fn a_github_domain_fills_its_home_or_refuses_without_a_public_url() {
    let fx = fixture_with(&[], |cfg, _| with_origin(cfg, None)).await;
    let err = fx
        .engine
        .set_manifest_policies("eng", &on(), &Scope::Unrestricted)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("switch Reflection on at the team server"),
        "{err}"
    );

    let fx = fixture_with(&[], |cfg, _| {
        with_origin(cfg, Some("https://kb.example.com"))
    })
    .await;
    fx.engine
        .set_manifest_policies("eng", &on(), &Scope::Unrestricted)
        .await
        .unwrap();
    let manifest_text = std::fs::read_to_string(fx.root.join("MANIFEST.md")).unwrap();
    assert!(
        manifest_text.contains("reflection_home: https://kb.example.com"),
        "{manifest_text}"
    );
    assert_eq!(
        fx.engine.reflection_state("eng").await.unwrap(),
        ReflectionState::On {
            home: crystalline_service::ReflectionHome::Here
        }
    );
}

/// `service.public_url` written in another spelling (upper-case host, the
/// default port, a trailing slash) is the same instance: the home is
/// written and compared in the one canonical spelling, so the instance
/// that filled the home reads it back as its own.
#[tokio::test]
async fn the_home_is_the_canonical_spelling_of_the_public_url() {
    let fx = fixture_with(&[], |cfg, _| {
        with_origin(cfg, Some("https://KB.Example.com:443/team/"))
    })
    .await;
    fx.engine
        .set_manifest_policies("eng", &on(), &Scope::Unrestricted)
        .await
        .unwrap();
    let manifest_text = std::fs::read_to_string(fx.root.join("MANIFEST.md")).unwrap();
    assert!(
        manifest_text.contains("reflection_home: https://kb.example.com/team\n"),
        "{manifest_text}"
    );
    assert_eq!(
        fx.engine.reflection_state("eng").await.unwrap(),
        ReflectionState::On {
            home: crystalline_service::ReflectionHome::Here
        }
    );
}

#[tokio::test]
async fn a_non_home_instance_refuses_and_points_to_the_home() {
    let fx = fixture_with(
        &[(
            "MANIFEST.md",
            &manifest("reflection: on\nreflection_home: https://team.example.com\n"),
        )],
        |cfg, _| with_origin(cfg, Some("https://laptop.example.com")),
    )
    .await;
    assert_eq!(
        fx.engine.reflection_state("eng").await.unwrap(),
        ReflectionState::Elsewhere {
            url: "https://team.example.com".into()
        }
    );
    let status = fx
        .engine
        .reflection_status("eng", &ReflectionCaller::owner())
        .await
        .unwrap();
    assert_eq!(status["state"], "elsewhere");
    assert_eq!(status["home_url"], "https://team.example.com");
    // Unset public_url: not the home either.
    let fx = fixture_with(
        &[(
            "MANIFEST.md",
            &manifest("reflection: on\nreflection_home: https://team.example.com\n"),
        )],
        |cfg, _| with_origin(cfg, None),
    )
    .await;
    assert!(matches!(
        fx.engine.reflection_state("eng").await.unwrap(),
        ReflectionState::Elsewhere { .. }
    ));
}

/// Switching on a domain that already says on but has no home fills
/// the home and seeds the pact, so the Homeless advice works; on configure
/// and on the PATCH alike. Without a public_url it still refuses.
#[tokio::test]
async fn switching_on_again_fills_the_home_of_a_homeless_domain() {
    let fx = fixture_with(
        &[("MANIFEST.md", &manifest("reflection: on\n"))],
        |cfg, _| with_origin(cfg, Some("https://kb.example.com")),
    )
    .await;
    assert_eq!(
        fx.engine.reflection_state("eng").await.unwrap(),
        ReflectionState::Homeless
    );
    assert!(!fx.root.join("reflections/PACT.md").exists());
    let set = std::collections::BTreeMap::from([("reflection".to_string(), "on".to_string())]);
    fx.engine
        .change_domain_settings("eng", &set, &[], &Scope::Unrestricted)
        .await
        .unwrap();
    let manifest_text = std::fs::read_to_string(fx.root.join("MANIFEST.md")).unwrap();
    assert!(
        manifest_text.contains("reflection_home: https://kb.example.com"),
        "{manifest_text}"
    );
    assert_eq!(
        manifest_text.matches("reflection: on").count(),
        1,
        "the switch is not written twice"
    );
    assert_eq!(
        fx.engine.reflection_state("eng").await.unwrap(),
        ReflectionState::On {
            home: crystalline_service::ReflectionHome::Here
        }
    );
    assert!(
        fx.root.join("reflections/PACT.md").exists(),
        "the pact is seeded"
    );
    // Once on and at home, a further switch-on is a no-op again.
    fx.engine
        .set_manifest_policies("eng", &on(), &Scope::Unrestricted)
        .await
        .unwrap();

    let fx = fixture_with(
        &[("MANIFEST.md", &manifest("reflection: on\n"))],
        |cfg, _| with_origin(cfg, None),
    )
    .await;
    let err = fx
        .engine
        .set_manifest_policies("eng", &on(), &Scope::Unrestricted)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("switch Reflection on at the team server"),
        "{err}"
    );
}

/// A restored MANIFEST that says `reflection: on` meets the screens every
/// other MANIFEST writer meets.
#[tokio::test]
async fn a_restored_manifest_that_switches_on_is_screened() {
    let fx = fixture(&[(
        "reflections/old.md",
        &engram("Old", "reflections/old", "- [fact] old"),
    )])
    .await;
    let err = fx
        .engine
        .restore_engram(
            "eng",
            "MANIFEST.md",
            &manifest("reflection: on\n"),
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("reflections/old.md"), "{err}");
    assert!(
        !std::fs::read_to_string(fx.root.join("MANIFEST.md"))
            .unwrap()
            .contains("reflection")
    );
}

/// A full-document save of the MANIFEST (the HTTP PUT) is screened too.
#[tokio::test]
async fn a_saved_manifest_that_switches_on_is_screened() {
    let fx = fixture(&[(
        "reflections/old.md",
        &engram("Old", "reflections/old", "- [fact] old"),
    )])
    .await;
    let source = std::fs::read_to_string(fx.root.join("MANIFEST.md")).unwrap();
    let save = crystalline_service::params::SaveParams {
        domain: "eng".into(),
        identifier: "manifest".into(),
        content: manifest("reflection: on\n"),
        expected_checksum: crate::support::sha256_hex(source.as_bytes()),
    };
    let err = fx
        .engine
        .save_engram(&save, &Scope::Unrestricted)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("reflections/old.md"), "{err}");
    assert_eq!(
        std::fs::read_to_string(fx.root.join("MANIFEST.md")).unwrap(),
        source
    );
}

/// `reflections/` and `Reflections/` side by side would land on the same
/// rows, so the switch is refused and both are named. A filesystem that
/// folds case (the macOS and Windows defaults) cannot hold both: there the
/// test has nothing to check and returns, and the engine's unit test
/// `two_spellings_of_the_folder_refuse_the_switch_and_name_both` pins the
/// sentence.
#[tokio::test]
async fn two_spellings_of_the_reflection_folder_refuse_the_switch() {
    let fx = fixture(&[]).await;
    std::fs::create_dir_all(fx.root.join("reflections")).unwrap();
    std::fs::create_dir_all(fx.root.join("Reflections")).unwrap();
    let spellings = std::fs::read_dir(&fx.root)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().eq_ignore_ascii_case("reflections"))
        .count();
    if spellings < 2 {
        eprintln!(
            "skipped: this filesystem folds letter case, so reflections/ and Reflections/ cannot both exist; the engine unit test pins the refusal"
        );
        return;
    }
    let err = fx
        .engine
        .set_manifest_policies("eng", &on(), &Scope::Unrestricted)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("reflections/") && err.contains("Reflections/"),
        "{err}"
    );
    assert_eq!(
        fx.engine.reflection_switch("eng"),
        crystalline_core::ReflectionSwitch::Off
    );
}

/// A switch that arrives out of band (a plain editor, a pull) while the
/// folder is spelled twice is held pending, naming both spellings, even when
/// every file in them is a page: one Reflection cannot read two folders.
/// Skipped on a filesystem that folds letter case.
#[tokio::test]
async fn an_out_of_band_switch_with_two_folder_spellings_stays_pending() {
    let fx = fixture(&[("reflections/a.md", &page("A"))]).await;
    std::fs::create_dir_all(fx.root.join("Reflections")).unwrap();
    if crystalline_core::reflection::reflection_folders(&fx.root).len() < 2 {
        eprintln!(
            "skipped: this filesystem folds letter case, so reflections/ and Reflections/ cannot both exist"
        );
        return;
    }
    std::fs::write(fx.root.join("Reflections/b.md"), page("B")).unwrap();
    std::fs::write(fx.root.join("MANIFEST.md"), manifest("reflection: on\n")).unwrap();
    fx.engine.sync(None).await.unwrap();
    assert_eq!(
        fx.engine.reflection_state("eng").await.unwrap(),
        ReflectionState::Pending {
            engrams: vec!["Reflections/".to_string(), "reflections/".to_string()]
        }
    );
    assert!(
        !fx.engine.reflection_effective("eng").await.unwrap(),
        "the mode shares and refreshes read agrees"
    );
    let err = fx
        .engine
        .read_reflection_tree(
            "eng",
            &ReflectionCaller::person(Scope::Unrestricted, "jordi"),
        )
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        err,
        format!(
            "Reflection for 'eng' {}: Reflections/, reflections/. Keep one and move the files of the others out",
            crystalline_service::engine::REFUSED_PENDING_SPELLINGS
        )
    );
}

/// A move onto a missing MANIFEST.md writes the moved text as the MANIFEST,
/// so a text that says `reflection: on` meets the switch-on screen.
#[tokio::test]
async fn a_move_onto_a_missing_manifest_that_switches_on_is_screened() {
    let fx = fixture(&[
        (
            "reflections/old.md",
            &engram("Old", "reflections/old", "- [fact] old"),
        ),
        (
            "drafts/next.md",
            &manifest("reflection: on\n").replace("permalink: manifest", "permalink: drafts/next"),
        ),
    ])
    .await;
    std::fs::remove_file(fx.root.join("MANIFEST.md")).unwrap();
    fx.engine.sync(None).await.unwrap();
    let err = fx
        .engine
        .move_engram(
            &crate::fixture::move_params("drafts/next", "MANIFEST.md"),
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("reflections/old.md"), "{err}");
    assert!(!fx.root.join("MANIFEST.md").exists(), "nothing was moved");
    assert!(fx.root.join("drafts/next.md").exists());
}

/// R14 fails closed: a record of the team's copy that exists and cannot be
/// read may hold files under reflections/, so the switch-on is refused and
/// the record is named.
#[tokio::test]
async fn an_unreadable_team_copy_record_refuses_the_switch() {
    let fx = fixture_with(&[], |cfg, _| {
        with_origin(cfg, Some("https://kb.example.com"))
    })
    .await;
    let dir = fx.tmp.path().join("origins").join("eng");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("state.json"), "{ not json").unwrap();
    let err = fx
        .engine
        .set_manifest_policies("eng", &on(), &Scope::Unrestricted)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("cannot be read") && err.contains(&dir.display().to_string()),
        "{err}"
    );
    assert_eq!(
        fx.engine.reflection_switch("eng"),
        crystalline_core::ReflectionSwitch::Off,
        "nothing was written"
    );
}

/// A pact that cannot be seeded leaves the switch off, so the error the
/// caller gets is true and sending the switch again seeds the pact.
#[tokio::test]
async fn a_pact_that_cannot_be_seeded_leaves_the_switch_off_for_a_retry() {
    let fx = fixture(&[]).await;
    // A file where the folder would go: the seed cannot create it.
    std::fs::write(fx.root.join("reflections"), "in the way").unwrap();
    fx.engine
        .set_manifest_policies("eng", &on(), &Scope::Unrestricted)
        .await
        .unwrap_err();
    assert_eq!(
        fx.engine.reflection_switch("eng"),
        crystalline_core::ReflectionSwitch::Off,
        "the switch is not written when the pact is not"
    );
    std::fs::remove_file(fx.root.join("reflections")).unwrap();
    fx.engine
        .set_manifest_policies("eng", &on(), &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(
        fx.root.join("reflections/PACT.md").exists(),
        "the retry seeds it"
    );
    assert_eq!(
        fx.engine.reflection_switch("eng"),
        crystalline_core::ReflectionSwitch::On
    );
}

/// A switch-off says what it means for sharing on a domain connected to
/// GitHub, on the policy PATCH and on configure alike, and only there: a
/// switch-off of a local domain, or of one already off, says nothing.
#[tokio::test]
async fn a_switch_off_of_a_team_domain_says_not_to_share_while_it_is_off() {
    let fx = fixture_with(&[], |cfg, _| {
        with_origin(cfg, Some("https://kb.example.com"))
    })
    .await;
    let off = vec![("reflection".to_string(), "off".to_string())];
    fx.engine
        .set_manifest_policies("eng", &on(), &Scope::Unrestricted)
        .await
        .unwrap();
    let receipt = fx
        .engine
        .set_manifest_policies("eng", &off, &Scope::Unrestricted)
        .await
        .unwrap();
    let note = receipt["note"].as_str().unwrap_or_default();
    assert!(
        note.contains("\"share changes\" offers the files in reflections/")
            && note.contains("Do not share changes while it is off"),
        "{receipt}"
    );
    let again = fx
        .engine
        .set_manifest_policies("eng", &off, &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(again.get("note").is_none(), "already off: {again}");

    // configure, removing the key.
    fx.engine
        .set_manifest_policies("eng", &on(), &Scope::Unrestricted)
        .await
        .unwrap();
    let view = fx
        .engine
        .change_domain_settings(
            "eng",
            &Default::default(),
            &["reflection".to_string()],
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert!(
        view["note"]
            .as_str()
            .is_some_and(|n| n.contains("Do not share changes while it is off")),
        "{view}"
    );

    // A local domain: no origin, nothing to share, no note.
    let fx = fixture(&[("MANIFEST.md", &manifest("reflection: on\n"))]).await;
    let receipt = fx
        .engine
        .set_manifest_policies("eng", &off, &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(receipt.get("note").is_none(), "{receipt}");
}

/// Every switch a Crystalline writer makes is announced as one `switched`
/// event with an empty path; a MANIFEST write that leaves the switch where
/// it was announces none.
#[tokio::test]
async fn every_switch_through_crystalline_announces_switched_with_an_empty_path() {
    use crystalline_service::changes::{Change, ReflectionChangeKind};
    let fx = fixture(&[]).await;
    let mut rx = fx.engine.changes().subscribe();
    let off = vec![("reflection".to_string(), "off".to_string())];
    fx.engine
        .set_manifest_policies("eng", &on(), &Scope::Unrestricted)
        .await
        .unwrap();
    fx.engine
        .set_manifest_policies("eng", &off, &Scope::Unrestricted)
        .await
        .unwrap();
    let source = std::fs::read_to_string(fx.root.join("MANIFEST.md")).unwrap();
    let save = crystalline_service::params::SaveParams {
        domain: "eng".into(),
        identifier: "manifest".into(),
        content: manifest("reflection: on\n"),
        expected_checksum: crate::support::sha256_hex(source.as_bytes()),
    };
    fx.engine
        .save_engram(&save, &Scope::Unrestricted)
        .await
        .unwrap();
    fx.engine
        .edit_engram(&EditParams {
            identifier: "manifest".into(),
            domain: "eng".into(),
            operation: "append".into(),
            content: Some("- Also its runbooks".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let mut switched = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        if let Change::Reflection(change) = envelope.change {
            assert_eq!(change.kind, ReflectionChangeKind::Switched, "{change:?}");
            switched.push((change.path, change.from));
        }
    }
    assert_eq!(switched, vec![(String::new(), None); 3]);
}

/// The actors of the `switched` events the bus holds, in order.
fn switched(
    rx: &mut tokio::sync::broadcast::Receiver<crystalline_service::changes::Envelope>,
) -> Vec<Option<String>> {
    use crystalline_service::changes::{Change, ReflectionChangeKind};
    let mut heard = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        if let Change::Reflection(change) = envelope.change
            && change.kind == ReflectionChangeKind::Switched
        {
            assert_eq!((change.path.as_str(), change.from), ("", None));
            heard.push(change.actor);
        }
    }
    heard
}

fn ada() -> Scope {
    Scope::User {
        account: "ada".into(),
        admin: true,
    }
}

/// The MANIFEST editor's save announces the switch it moves.
#[tokio::test]
async fn a_manifest_editor_save_announces_the_switch() {
    let fx = fixture(&[]).await;
    let source = std::fs::read_to_string(fx.root.join("MANIFEST.md")).unwrap();
    let mut rx = fx.engine.changes().subscribe();
    fx.engine
        .save_manifest(
            "eng",
            &manifest("reflection: on\n"),
            &crate::support::sha256_hex(source.as_bytes()),
            &ada(),
            false,
        )
        .await
        .unwrap();
    assert_eq!(switched(&mut rx), vec![Some("ada".to_string())]);
}

/// A restored MANIFEST announces the switch it moves, under the account
/// that restored it.
#[tokio::test]
async fn a_restored_manifest_announces_the_switch_under_its_account() {
    let fx = fixture(&[]).await;
    let mut rx = fx.engine.changes().subscribe();
    fx.engine
        .restore_engram("eng", "MANIFEST.md", &manifest("reflection: on\n"), &ada())
        .await
        .unwrap();
    assert_eq!(switched(&mut rx), vec![Some("ada".to_string())]);
}

/// A move onto a missing MANIFEST.md announces the switch the moved text
/// sets.
#[tokio::test]
async fn a_move_onto_the_manifest_announces_the_switch() {
    let fx = fixture(&[(
        "drafts/next.md",
        &manifest("reflection: on\n").replace("permalink: manifest", "permalink: drafts/next"),
    )])
    .await;
    std::fs::remove_file(fx.root.join("MANIFEST.md")).unwrap();
    fx.engine.sync(None).await.unwrap();
    let mut rx = fx.engine.changes().subscribe();
    fx.engine
        .move_engram(
            &crate::fixture::move_params("drafts/next", "MANIFEST.md"),
            &ada(),
        )
        .await
        .unwrap();
    assert_eq!(switched(&mut rx), vec![Some("ada".to_string())]);
}

/// An edit of the MANIFEST that moves the switch announces it.
#[tokio::test]
async fn a_manifest_edit_that_moves_the_switch_announces_it() {
    let fx = fixture(&[("MANIFEST.md", &manifest("reflection: on\n"))]).await;
    let mut rx = fx.engine.changes().subscribe();
    fx.engine
        .edit_engram_as(
            &EditParams {
                identifier: "manifest".into(),
                domain: "eng".into(),
                operation: "find_replace".into(),
                find_text: Some("reflection: on".into()),
                content: Some("reflection: off".into()),
                ..Default::default()
            },
            None,
            &ada(),
        )
        .await
        .unwrap();
    assert_eq!(switched(&mut rx), vec![Some("ada".to_string())]);
}

/// The cost of the Reflection gate on a large effective domain: 20,000
/// engrams and 300 pages, 500 `reflection_status` calls and 50 rounds of
/// what `GET /reflection/domains` asks the engine for each domain (the
/// status, then the open threads of a Reflection that is on). Never a gate:
/// the numbers depend on the machine and go into the task report. Run with
/// `cargo nextest run -p crystalline-service --test reflection --run-ignored only reflection_gate_timing --no-capture`.
#[tokio::test]
#[ignore = "timing evidence for the task report, not a gate"]
async fn reflection_gate_timing() {
    use std::time::Instant;

    use crate::docs::person;

    let manifest_on = manifest("reflection: on\n");
    let mut files: Vec<(String, String)> = vec![("MANIFEST.md".into(), manifest_on)];
    for i in 0..20_000 {
        files.push((
            format!("notes/d{}/n{i}.md", i % 100),
            engram(
                &format!("Note {i}"),
                &format!("note-{i}"),
                "- [fact] a note",
            ),
        ));
    }
    for i in 0..300 {
        files.push((format!("reflections/p{i}.md"), page(&format!("Page {i}"))));
    }
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    let built = Instant::now();
    let (fx, store) = fixture_holding_the_store(&refs).await;
    eprintln!("fixture: {:?}", built.elapsed());
    assert!(
        fx.engine.reflection_effective("eng").await.unwrap(),
        "the timing is taken on an effective Reflection"
    );

    // The two index questions alone, as the gate asks them under the lock.
    let started = Instant::now();
    for _ in 0..500 {
        let store = store.lock().await;
        let did = store.domain_id("eng").await.unwrap().unwrap();
        let under = store.engram_rows_under_reflections(did, 64).await.unwrap();
        assert!(under.is_empty() && store.has_engram_rows(did).await.unwrap());
    }
    let probes = started.elapsed();

    // The gate without the caller's rights: the state alone.
    let started = Instant::now();
    for _ in 0..500 {
        fx.engine.reflection_state("eng").await.unwrap();
    }
    let state = started.elapsed();

    let caller = person();
    let started = Instant::now();
    for _ in 0..500 {
        fx.engine.reflection_status("eng", &caller).await.unwrap();
    }
    let status = started.elapsed();

    let started = Instant::now();
    for _ in 0..50 {
        let mut names: Vec<String> = fx.engine.registered_domain_names().into_iter().collect();
        names.sort();
        for name in names {
            let status = fx.engine.reflection_status(&name, &caller).await.unwrap();
            if status["state"] == "on" {
                fx.engine
                    .reflection_open_threads(&name, &caller)
                    .await
                    .unwrap();
            }
        }
    }
    let domains = started.elapsed();

    // The two halves of the open threads count: the stamps listing alone,
    // and the whole count behind its gate.
    let started = Instant::now();
    for _ in 0..50 {
        let store = store.lock().await;
        let did = store.domain_id("eng").await.unwrap().unwrap();
        store.list_reflection_stamps(did).await.unwrap();
    }
    let stamps = started.elapsed();
    let started = Instant::now();
    for _ in 0..50 {
        fx.engine
            .reflection_open_threads("eng", &caller)
            .await
            .unwrap();
    }
    let threads = started.elapsed();

    // The two store questions every gate asked besides the probe: the
    // visibility screen and the domain row (F2).
    let started = Instant::now();
    for _ in 0..500 {
        fx.engine
            .hidden_for(&crystalline_service::Scope::Unrestricted)
            .await
            .unwrap();
    }
    let hidden = started.elapsed();
    let root = fx.root.to_string_lossy().to_string();
    let started = Instant::now();
    for _ in 0..500 {
        store
            .lock()
            .await
            .upsert_domain("eng", Some(&root), crystalline_index::DomainKind::File)
            .await
            .unwrap();
    }
    let upsert = started.elapsed();
    let started = Instant::now();
    for _ in 0..500 {
        fx.engine
            .require_reflection_edit("eng", "p0.md", &caller)
            .await
            .unwrap();
    }
    let gate = started.elapsed();

    // A page citing ten engrams, read whole, and the same page's threads
    // listed with every other page's (F2).
    let agent = crate::docs::agent();
    let mut cited = crate::docs::write("cited.md", "# Cited\n\nFor people.\n");
    let mut sources = std::collections::BTreeMap::new();
    for s in 0..5 {
        cited
            .markdown
            .as_mut()
            .unwrap()
            .push_str(&format!("\n## Part {s}\n\nText {s}.\n"));
        sources.insert(
            format!("Part {s}"),
            vec![format!("note-{}", 2 * s), format!("note-{}", 2 * s + 1)],
        );
    }
    cited.sources = sources;
    fx.engine.write_reflection(&cited, &agent).await.unwrap();
    for s in 0..5 {
        fx.engine
            .discuss(
                &crystalline_service::params::DiscussParams {
                    domain: "eng".into(),
                    action: Some("open".into()),
                    path: Some("cited.md".into()),
                    section: Some(format!("Part {s}")),
                    text: Some("Still true?".into()),
                    ..Default::default()
                },
                &agent,
            )
            .await
            .unwrap();
    }
    let doc = fx
        .engine
        .read_reflection("eng", "cited.md", &agent)
        .await
        .unwrap();
    assert_eq!(doc["sections"][0]["sources"][0]["state"], "current");
    let started = Instant::now();
    for _ in 0..50 {
        fx.engine
            .read_reflection("eng", "cited.md", &agent)
            .await
            .unwrap();
    }
    let cited_read = started.elapsed();
    let started = Instant::now();
    for _ in 0..50 {
        fx.engine
            .read_reflection("eng", "p0.md", &agent)
            .await
            .unwrap();
    }
    let plain_read = started.elapsed();
    let started = Instant::now();
    for _ in 0..50 {
        fx.engine
            .list_reflection_threads("eng", None, "open", &agent)
            .await
            .unwrap();
    }
    let listed = started.elapsed();
    let ids: Vec<String> = doc["sections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect();
    let started = Instant::now();
    for _ in 0..50 {
        fx.engine
            .settle_reflection("eng", "cited.md", &ids, "taken_in", &agent)
            .await
            .unwrap();
    }
    let settled = started.elapsed();

    eprintln!("index probes x500: {probes:?} ({:?} each)", probes / 500);
    eprintln!("reflection_state x500: {state:?} ({:?} each)", state / 500);
    eprintln!(
        "reflection_status x500: {status:?} ({:?} each)",
        status / 500
    );
    eprintln!(
        "GET /reflection/domains x50 (engine calls): {domains:?} ({:?} each)",
        domains / 50
    );
    eprintln!("stamps listing x50: {stamps:?} ({:?} each)", stamps / 50);
    eprintln!(
        "reflection_open_threads x50: {threads:?} ({:?} each)",
        threads / 50
    );
    eprintln!("hidden_for x500: {hidden:?} ({:?} each)", hidden / 500);
    eprintln!(
        "upsert_domain under the lock x500: {upsert:?} ({:?} each)",
        upsert / 500
    );
    eprintln!(
        "require_reflection_edit x500: {gate:?} ({:?} each)",
        gate / 500
    );
    eprintln!(
        "read_reflection, ten cited engrams x50: {cited_read:?} ({:?} each)",
        cited_read / 50
    );
    eprintln!(
        "read_reflection, no sources x50: {plain_read:?} ({:?} each)",
        plain_read / 50
    );
    eprintln!(
        "list_reflection_threads x50: {listed:?} ({:?} each)",
        listed / 50
    );
    eprintln!(
        "settle_reflection taken_in, ten cited engrams x50: {settled:?} ({:?} each)",
        settled / 50
    );
}

/// A store the test keeps a handle on, under the fixture's engine.
async fn fixture_holding_the_store(
    files: &[(&str, &str)],
) -> (
    crate::fixture::Fx,
    std::sync::Arc<tokio::sync::Mutex<dyn crystalline_index::Store>>,
) {
    let store: std::sync::Arc<tokio::sync::Mutex<dyn crystalline_index::Store>> =
        std::sync::Arc::new(tokio::sync::Mutex::new(
            crystalline_index::TursoStore::open_in_memory()
                .await
                .unwrap(),
        ));
    let fx = crate::fixture::fixture_on(store.clone(), files, |_, _| {}, None, None, None).await;
    (fx, store)
}

/// The engram rows and the reflection rows the index holds of `eng`.
async fn rows_of(
    store: &std::sync::Arc<tokio::sync::Mutex<dyn crystalline_index::Store>>,
) -> (Vec<String>, Vec<String>) {
    let store = store.lock().await;
    let did = store.domain_id("eng").await.unwrap().unwrap();
    let mut engrams: Vec<String> = store.file_stamps(did).await.unwrap().into_keys().collect();
    engrams.sort();
    let pages = store
        .list_reflections(did)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.path)
        .collect();
    (engrams, pages)
}

/// Every gate asks whether anything is pending. On a Reflection in effect
/// (rows recorded, none under the folder) the answer comes from one probe of
/// the index, never from loading every recorded path of the domain (R82).
#[tokio::test]
async fn a_hundred_gates_on_an_effective_domain_load_no_recorded_paths() {
    use crate::docs::{person, reflecting, write};

    let fx = reflecting().await;
    fx.engine
        .write_reflection(&write("a.md", &page("A")), &person())
        .await
        .unwrap();
    let before = fx.engine.recorded_path_loads();
    for _ in 0..50 {
        fx.engine.reflection_status("eng", &person()).await.unwrap();
    }
    for _ in 0..50 {
        fx.engine
            .read_reflection("eng", "a.md", &person())
            .await
            .unwrap();
    }
    assert_eq!(
        fx.engine.recorded_path_loads(),
        before,
        "a gate on an effective Reflection loads no recorded path"
    );
}

/// A hand-written page with no section id line and no `reflection:` key is a
/// pending engram to the disk walk, which an empty index takes. On an index
/// that records the domain's engrams and holds the file as a reflection row,
/// the fast path answers "nothing pending" without handing an empty path
/// list to the index functions, which would read it as an empty index and
/// walk the disk.
#[tokio::test]
async fn an_unmarked_page_keeps_an_effective_reflection_on_after_the_fast_path() {
    let (fx, store) = fixture_holding_the_store(&[
        ("MANIFEST.md", &manifest("reflection: on\n")),
        (
            "runbooks/deploy.md",
            &engram("Deploy", "runbooks/deploy", "- [fact] deploys run at night"),
        ),
    ])
    .await;
    std::fs::create_dir_all(fx.root.join("reflections")).unwrap();
    std::fs::write(
        fx.root.join("reflections/plain.md"),
        "---\ntitle: Plain\n---\n\n# Plain\n\nWritten by hand.\n",
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
    assert!(
        crystalline_core::is_pending_engram(
            &fx.root,
            "reflections/plain.md",
            crystalline_core::EngramRow::Unknown
        ),
        "the disk walk would call the page a pending engram"
    );
    let (engrams, pages) = rows_of(&store).await;
    assert!(
        pages.iter().any(|p| p == "plain.md"),
        "a reflection row: {pages:?}"
    );
    assert!(
        !engrams.is_empty()
            && !engrams
                .iter()
                .any(|p| crystalline_core::reflection::is_under_reflections(p)),
        "engram rows recorded, none under the folder: {engrams:?}"
    );
    assert_eq!(
        fx.engine.reflection_state("eng").await.unwrap(),
        ReflectionState::On {
            home: crystalline_service::ReflectionHome::Local
        }
    );
    assert!(fx.engine.reflection_effective("eng").await.unwrap());
}

const PENDING_FILES: [(&str, &str); 2] = [
    (
        "reflections/old.md",
        "---\ntype: engram\ntitle: Old\npermalink: reflections/old\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-09-01\n---\n\n# Old\n\n- [fact] old\n",
    ),
    (
        "reflections/deep/older.md",
        "---\ntype: engram\ntitle: Older\npermalink: reflections/deep/older\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-09-01\n---\n\n# Older\n\n- [fact] older\n",
    ),
];

/// A switch that arrives out of band while two engrams live under the folder
/// stays pending and names both, in order: the engrams' rows under the folder
/// are what the fast path hands on.
#[tokio::test]
async fn a_pending_switch_still_names_every_engram() {
    let (fx, store) = fixture_holding_the_store(&PENDING_FILES).await;
    std::fs::write(fx.root.join("MANIFEST.md"), manifest("reflection: on\n")).unwrap();
    fx.engine.sync(None).await.unwrap();
    let (engrams, _) = rows_of(&store).await;
    assert!(
        engrams.iter().any(|p| p == "reflections/old.md"),
        "{engrams:?}"
    );
    assert_eq!(
        fx.engine.reflection_state("eng").await.unwrap(),
        ReflectionState::Pending {
            engrams: vec![
                "reflections/deep/older.md".to_string(),
                "reflections/old.md".to_string()
            ]
        }
    );
    assert!(!fx.engine.reflection_effective("eng").await.unwrap());
}

/// The same switch on an index that holds no row of the domain (a wipe and
/// rebuild, a new machine): the files on disk decide, so it stays pending
/// and names both engrams rather than reading "no row under the folder" as
/// "nothing pending".
#[tokio::test]
async fn an_out_of_band_switch_on_an_empty_index_still_names_every_engram() {
    let (fx, store) = fixture_holding_the_store(&PENDING_FILES).await;
    std::fs::write(fx.root.join("MANIFEST.md"), manifest("reflection: on\n")).unwrap();
    {
        let store = store.lock().await;
        let did = store.domain_id("eng").await.unwrap().unwrap();
        store.clear_domain(did).await.unwrap();
    }
    let (engrams, _) = rows_of(&store).await;
    assert!(engrams.is_empty(), "{engrams:?}");
    assert_eq!(
        fx.engine.reflection_state("eng").await.unwrap(),
        ReflectionState::Pending {
            engrams: vec![
                "reflections/deep/older.md".to_string(),
                "reflections/old.md".to_string()
            ]
        }
    );
    assert!(!fx.engine.reflection_effective("eng").await.unwrap());
}

/// More engrams under the folder than the gate's first read takes: the
/// pending switch still names every one, so the rest is read.
#[tokio::test]
async fn a_pending_switch_past_the_probe_names_every_engram() {
    let files: Vec<(String, String)> = (0..70)
        .map(|i| {
            (
                format!("reflections/many/n{i:02}.md"),
                engram(
                    &format!("N{i}"),
                    &format!("reflections/many/n{i:02}"),
                    "- [fact] n",
                ),
            )
        })
        .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    let fx = fixture(&refs).await;
    std::fs::write(fx.root.join("MANIFEST.md"), manifest("reflection: on\n")).unwrap();
    fx.engine.sync(None).await.unwrap();
    let before = fx.engine.recorded_path_loads();
    let ReflectionState::Pending { engrams } = fx.engine.reflection_state("eng").await.unwrap()
    else {
        panic!("the switch waits for the engrams");
    };
    let mut expected: Vec<String> = files.into_iter().map(|(p, _)| p).collect();
    expected.sort();
    assert_eq!(engrams, expected);
    assert_eq!(
        fx.engine.recorded_path_loads(),
        before + 1,
        "one read past the probe"
    );
}

/// A page that cites engrams reads the facts of its own sources only, never
/// every engram of the domain (F2), and each source still answers its state:
/// current, changed, retired or missing.
#[tokio::test]
async fn a_page_citing_engrams_reads_only_its_sources() {
    use crate::docs::{agent, write};

    let notes: Vec<(String, String)> = ["a", "b", "c", "d"]
        .into_iter()
        .map(|n| {
            (
                format!("notes/{n}.md"),
                engram(&format!("Note {n}"), &format!("notes/{n}"), "- [fact] x"),
            )
        })
        .collect();
    let manifest_on = manifest("reflection: on\n");
    let mut files: Vec<(&str, &str)> = vec![("MANIFEST.md", &manifest_on)];
    files.extend(notes.iter().map(|(p, c)| (p.as_str(), c.as_str())));
    let fx = fixture(&files).await;
    let mut p = write(
        "a.md",
        "# Notes\n\nFor people.\n\n## First\n\nOne.\n\n## Second\n\nTwo.\n",
    );
    p.sources = std::collections::BTreeMap::from([
        (
            "First".to_string(),
            vec!["notes/a".to_string(), "notes/b".to_string()],
        ),
        (
            "Second".to_string(),
            vec!["notes/c".to_string(), "notes/d".to_string()],
        ),
    ]);
    let before_write = fx.engine.recorded_path_loads();
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    assert_eq!(
        fx.engine.recorded_path_loads(),
        before_write,
        "a sourced write reads only the engrams it names"
    );

    // b changes, c is retired, d goes; a stays as it was cited.
    std::fs::write(
        fx.root.join("notes/b.md"),
        engram("Note b", "notes/b", "- [fact] a longer fact"),
    )
    .unwrap();
    std::fs::write(
        fx.root.join("notes/c.md"),
        engram("Note c", "notes/c", "- [fact] x").replace("status: stable", "status: superseded"),
    )
    .unwrap();
    std::fs::remove_file(fx.root.join("notes/d.md")).unwrap();
    fx.engine.sync(None).await.unwrap();

    let before = fx.engine.recorded_path_loads();
    let doc = fx
        .engine
        .read_reflection("eng", "a.md", &agent())
        .await
        .unwrap();
    let states: Vec<(String, String, String)> = doc["sections"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["sources"].as_array().unwrap().iter())
        .map(|src| {
            (
                src["permalink"].as_str().unwrap().to_string(),
                src["state"].as_str().unwrap().to_string(),
                src["title"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let want = [
        ("notes/a", "current", "Note a"),
        ("notes/b", "changed", "Note b"),
        ("notes/c", "retired", "Note c"),
        ("notes/d", "missing", ""),
    ]
    .map(|(p, s, t)| (p.to_string(), s.to_string(), t.to_string()));
    assert_eq!(states, want);
    assert_eq!(
        fx.engine.recorded_path_loads(),
        before,
        "a cited page loads no engram facts of the whole domain"
    );

    // A source no engram holds is still refused by name, nothing written.
    let mut q = write("b.md", "# B\n\nFor people.\n\n## Only\n\nText.\n");
    q.sources = std::collections::BTreeMap::from([(
        "Only".to_string(),
        vec!["notes/a".to_string(), "notes/nowhere".to_string()],
    )]);
    let err = fx
        .engine
        .write_reflection(&q, &agent())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("no engram 'notes/nowhere' in domain 'eng' to name as a source"),
        "{err}"
    );
    assert!(!fx.root.join("reflections/b.md").exists());
}

/// A page with sources in another domain reads that domain one seek per
/// cited engram: a queue pass scans only its own domain, so the local cost
/// floor holds whatever the page cites elsewhere.
#[tokio::test]
async fn sources_in_another_domain_cost_no_scan_of_it() {
    use crate::docs::{agent, write};
    let fx = crate::sources::crossing().await;
    let mut p = write(
        "a.md",
        "# Notes\n\nFor people.\n\n## Team\n\nOne.\n\n## Local\n\nTwo.\n",
    );
    p.sources = std::collections::BTreeMap::from([
        (
            "Team".to_string(),
            vec![
                "crystalline://harbor/team/harbor-team".to_string(),
                "crystalline://harbor/team/rota".to_string(),
            ],
        ),
        ("Local".to_string(), vec!["runbooks/deploy".to_string()]),
    ]);
    let before = fx.engine.recorded_path_loads();
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    fx.engine
        .read_reflection("eng", "a.md", &agent())
        .await
        .unwrap();
    assert_eq!(
        fx.engine.recorded_path_loads(),
        before,
        "a write and a read scan no domain"
    );
    let params = crystalline_service::params::ReflectParams {
        domains: vec!["eng".into()],
        ..Default::default()
    };
    let before = fx.engine.recorded_path_loads();
    fx.engine.reflect(&params, &agent()).await.unwrap();
    assert_eq!(
        fx.engine.recorded_path_loads(),
        before + 1,
        "one scan, of eng only"
    );
}

/// The engrams the gate remembers per domain follow the rows. The switch
/// screen asks for them while the Reflection is off, so an engram a verb
/// moves into the folder after a refused switch is named by the next one,
/// and the one that moved out is not (R83).
#[tokio::test]
async fn a_cached_state_follows_an_engram_moved_into_the_folder() {
    let notes = engram("N", "notes/n", "- [fact] n");
    let fx = fixture(&[PENDING_FILES[0], ("notes/n.md", &notes)]).await;
    follows_an_engram_moved_in(fx).await;
}

/// The same on a database other instances may share, where a remembered
/// entry also ages out: an announced change still forgets it at once.
#[cfg(feature = "postgres")]
#[tokio::test]
async fn a_cached_state_follows_an_engram_moved_into_the_folder_on_postgres() {
    use crate::fixture::{fixture_on, scratch_schema};

    let Some(scratch) = scratch_schema() else {
        return;
    };
    let store = crystalline_index::PostgresStore::open_in_schema(scratch.url(), scratch.name())
        .await
        .expect("open the postgres test schema");
    let notes = engram("N", "notes/n", "- [fact] n");
    let fx = fixture_on(
        std::sync::Arc::new(tokio::sync::Mutex::new(store)),
        &[PENDING_FILES[0], ("notes/n.md", &notes)],
        |_, _| {},
        None,
        None,
        None,
    )
    .await;
    follows_an_engram_moved_in(fx).await;
}

async fn follows_an_engram_moved_in(fx: crate::fixture::Fx) {
    let policies = on();
    let switch_on = || {
        fx.engine
            .set_manifest_policies("eng", &policies, &Scope::Unrestricted)
    };
    let err = switch_on().await.unwrap_err().to_string();
    assert!(err.contains("reflections/old.md"), "{err}");
    fx.engine
        .move_engram(
            &crate::fixture::move_params("notes/n", "reflections/n.md"),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    fx.engine
        .move_engram(
            &crate::fixture::move_params("reflections/old", "old.md"),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let err = switch_on().await.unwrap_err().to_string();
    assert!(
        err.contains("reflections/n.md") && !err.contains("reflections/old.md"),
        "the engram moved in is named, the one moved out is not: {err}"
    );
    fx.engine
        .move_engram(
            &crate::fixture::move_params("reflections/n", "n.md"),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    switch_on().await.unwrap();
    assert_eq!(
        fx.engine.reflection_state("eng").await.unwrap(),
        ReflectionState::On {
            home: crystalline_service::ReflectionHome::Local
        }
    );
    assert!(fx.engine.reflection_effective("eng").await.unwrap());
}

/// An engram a plain editor drops into the folder of a pending Reflection is
/// named as soon as a sync has indexed it, though the gate read the state
/// before (R83).
#[tokio::test]
async fn a_cached_state_follows_an_engram_synced_into_the_folder() {
    let fx = fixture(&[PENDING_FILES[0]]).await;
    std::fs::write(fx.root.join("MANIFEST.md"), manifest("reflection: on\n")).unwrap();
    fx.engine.sync(None).await.unwrap();
    assert_eq!(
        fx.engine.reflection_state("eng").await.unwrap(),
        ReflectionState::Pending {
            engrams: vec!["reflections/old.md".to_string()]
        },
        "read once, so the gate remembers it"
    );
    std::fs::write(
        fx.root.join("reflections/new.md"),
        engram("New", "reflections/new", "- [fact] new"),
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
    assert_eq!(
        fx.engine.reflection_state("eng").await.unwrap(),
        ReflectionState::Pending {
            engrams: vec![
                "reflections/new.md".to_string(),
                "reflections/old.md".to_string()
            ]
        },
        "the synced engram is named"
    );
}

/// The switch is read fresh on every gate, never remembered: a MANIFEST
/// rewritten by hand to `reflection: off` (same length, same modification
/// time, no sync) turns the Reflection off at once (R83).
#[tokio::test]
async fn a_cached_state_follows_a_manifest_edit_outside_crystalline() {
    let on = manifest("reflection: on \n");
    let off = manifest("reflection: off\n");
    assert_eq!(on.len(), off.len(), "the rewrite keeps the length");
    let fx = fixture(&[
        ("MANIFEST.md", &on),
        (
            "runbooks/deploy.md",
            &engram("Deploy", "runbooks/deploy", "- [fact] deploys run at night"),
        ),
    ])
    .await;
    assert_eq!(
        fx.engine.reflection_state("eng").await.unwrap(),
        ReflectionState::On {
            home: crystalline_service::ReflectionHome::Local
        }
    );
    let path = fx.root.join("MANIFEST.md");
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    std::fs::write(&path, &off).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        modified
    );
    assert_eq!(
        fx.engine.reflection_state("eng").await.unwrap(),
        ReflectionState::Off
    );
}

/// A take-in reads the facts of the settled sections' sources only, never
/// every engram of the domain (F2 fix round): the sources of the sections it
/// settles take the engrams' current checksums, and a section it leaves
/// alone keeps its old ones.
#[tokio::test]
async fn a_take_in_reads_only_the_sources_it_settles() {
    use crate::docs::{agent, write};

    let notes: Vec<(String, String)> = ["a", "b"]
        .into_iter()
        .map(|n| {
            (
                format!("notes/{n}.md"),
                engram(&format!("Note {n}"), &format!("notes/{n}"), "- [fact] x"),
            )
        })
        .collect();
    let manifest_on = manifest("reflection: on\n");
    let mut files: Vec<(&str, &str)> = vec![("MANIFEST.md", &manifest_on)];
    files.extend(notes.iter().map(|(p, c)| (p.as_str(), c.as_str())));
    let fx = fixture(&files).await;
    let mut p = write(
        "a.md",
        "# Notes\n\nFor people.\n\n## First\n\nOne.\n\n## Second\n\nTwo.\n",
    );
    p.sources = std::collections::BTreeMap::from([
        ("First".to_string(), vec!["notes/a".to_string()]),
        ("Second".to_string(), vec!["notes/b".to_string()]),
    ]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    for n in ["a", "b"] {
        std::fs::write(
            fx.root.join(format!("notes/{n}.md")),
            engram(
                &format!("Note {n}"),
                &format!("notes/{n}"),
                "- [fact] a longer fact",
            ),
        )
        .unwrap();
    }
    fx.engine.sync(None).await.unwrap();
    let states = |doc: &serde_json::Value| -> Vec<String> {
        doc["sections"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["sources"][0]["state"].as_str().unwrap().to_string())
            .collect()
    };
    let doc = fx
        .engine
        .read_reflection("eng", "a.md", &agent())
        .await
        .unwrap();
    assert_eq!(states(&doc), ["changed", "changed"]);
    let first = doc["sections"][0]["id"].as_str().unwrap().to_string();

    let before = fx.engine.recorded_path_loads();
    fx.engine
        .settle_reflection("eng", "a.md", &[first], "taken_in", &agent())
        .await
        .unwrap();
    assert_eq!(
        fx.engine.recorded_path_loads(),
        before,
        "a take-in loads no engram facts of the whole domain"
    );
    let doc = fx
        .engine
        .read_reflection("eng", "a.md", &agent())
        .await
        .unwrap();
    assert_eq!(states(&doc), ["current", "changed"]);
}

/// A Reflection poll and a verb's gate take no store lock once the gate
/// remembers the domain (F2): the visibility screen of a registered domain
/// needs no index read, and the domain's row id is remembered with its rows.
#[tokio::test]
async fn a_poll_and_a_gate_answer_without_the_store_lock() {
    let fx = fixture(&[
        ("MANIFEST.md", &manifest("reflection: on\n")),
        (
            "runbooks/deploy.md",
            &engram("Deploy", "runbooks/deploy", "- [fact] deploys run at night"),
        ),
    ])
    .await;
    poll_and_gate_without_the_store_lock(fx).await;
}

/// The same on an instance with an id, as a daemon is: its sync claimed
/// the domain, so the host check answers from memory too.
#[tokio::test]
async fn a_poll_and_a_gate_on_an_instance_answer_without_the_store_lock() {
    let store: std::sync::Arc<tokio::sync::Mutex<dyn crystalline_index::Store>> =
        std::sync::Arc::new(tokio::sync::Mutex::new(
            crystalline_index::TursoStore::open_in_memory()
                .await
                .unwrap(),
        ));
    let fx = crate::fixture::instance_on(
        store,
        "this-machine",
        &[
            ("MANIFEST.md", &manifest("reflection: on\n")),
            (
                "runbooks/deploy.md",
                &engram("Deploy", "runbooks/deploy", "- [fact] deploys run at night"),
            ),
        ],
    )
    .await;
    poll_and_gate_without_the_store_lock(fx).await;
}

/// A warm status and gate, then both again within 200 ms while the test
/// holds the store lock.
async fn poll_and_gate_without_the_store_lock(fx: crate::fixture::Fx) {
    use crate::docs::person;

    let caller = person();
    let status = fx.engine.reflection_status("eng", &caller).await.unwrap();
    assert_eq!(status["state"], "on");
    fx.engine
        .require_reflection_edit("eng", "a.md", &caller)
        .await
        .unwrap();
    let store = fx.engine.store();
    let held = store.lock().await;
    let budget = std::time::Duration::from_millis(200);
    let polled = tokio::time::timeout(budget, fx.engine.reflection_status("eng", &caller))
        .await
        .expect("the status answers while the store lock is held")
        .unwrap();
    assert_eq!(polled, status);
    tokio::time::timeout(
        budget,
        fx.engine.require_reflection_edit("eng", "a.md", &caller),
    )
    .await
    .expect("the gate answers while the store lock is held")
    .unwrap();
    drop(held);
}

/// A domain removed and added again gets a new index row, and the gate's
/// remembered row id goes with the removal: a page written after the re-add
/// is indexed under the new row, not the one the removal dropped (F2).
#[tokio::test]
async fn a_re_added_domain_indexes_its_pages_under_its_new_row() {
    use crate::docs::{person, write};

    let fx = fixture(&[
        ("MANIFEST.md", &manifest("reflection: on\n")),
        (
            "runbooks/deploy.md",
            &engram("Deploy", "runbooks/deploy", "- [fact] deploys run at night"),
        ),
    ])
    .await;
    let caller = person();
    fx.engine
        .require_reflection_edit("eng", "a.md", &caller)
        .await
        .unwrap();
    let store = fx.engine.store();
    let old = store.lock().await.domain_id("eng").await.unwrap().unwrap();
    fx.engine.domain_remove("eng").await.unwrap();
    fx.engine
        .domain_add_local(Some("eng"), Some(&fx.root.to_string_lossy()))
        .await
        .unwrap();
    fx.engine.sync(Some("eng")).await.unwrap();
    let new = store.lock().await.domain_id("eng").await.unwrap().unwrap();
    assert_ne!(old, new, "the re-add made a new row");
    fx.engine
        .write_reflection(
            &write("a.md", "# A\n\nFor people.\n\n## Part\n\nText.\n"),
            &caller,
        )
        .await
        .unwrap();
    let pages: Vec<String> = store
        .lock()
        .await
        .list_reflection_stamps(new)
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.path)
        .collect();
    assert!(pages.iter().any(|p| p == "a.md"), "{pages:?}");
}

/// A domain removed here and registered again by another process (the
/// configuration file edited, as `domain add` does with a daemon running)
/// is gated before any sync or announce of it: the removal alone has to
/// forget the row id the gate remembered, or a page written now would land
/// under the row the removal dropped (F2).
#[tokio::test]
async fn a_domain_registered_again_by_file_gets_a_new_row_at_its_first_gate() {
    use crate::docs::{person, write};

    let fx = fixture(&[
        ("MANIFEST.md", &manifest("reflection: on\n")),
        (
            "runbooks/deploy.md",
            &engram("Deploy", "runbooks/deploy", "- [fact] deploys run at night"),
        ),
    ])
    .await;
    let caller = person();
    fx.engine
        .require_reflection_edit("eng", "a.md", &caller)
        .await
        .unwrap();
    let config_path = fx.tmp.path().join("config.yaml");
    let before: crystalline_core::config::GlobalConfig =
        crystalline_core::config::load_yaml(&config_path).unwrap();
    let entry = before.domains["eng"].clone();
    fx.engine.domain_remove("eng").await.unwrap();
    let store = fx.engine.store();
    assert_eq!(store.lock().await.domain_id("eng").await.unwrap(), None);

    let mut after: crystalline_core::config::GlobalConfig =
        crystalline_core::config::load_yaml(&config_path).unwrap();
    after.domains.insert("eng".to_string(), entry);
    crystalline_core::config::save_yaml(&config_path, &after).unwrap();
    fx.engine
        .write_reflection(
            &write("a.md", "# A\n\nFor people.\n\n## Part\n\nText.\n"),
            &caller,
        )
        .await
        .unwrap();
    let id = store
        .lock()
        .await
        .domain_id("eng")
        .await
        .unwrap()
        .expect("the gate made the domain a row again");
    let pages: Vec<String> = store
        .lock()
        .await
        .list_reflection_stamps(id)
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.path)
        .collect();
    assert!(pages.iter().any(|p| p == "a.md"), "{pages:?}");
}

/// Once the gate remembers a domain's rows, the state and the mode answer
/// without the store lock: a long sync or embedding pass holding it does
/// not stall every Reflection poll behind it.
#[tokio::test]
async fn the_cache_answers_without_the_store_lock() {
    let fx = fixture(&[
        ("MANIFEST.md", &manifest("reflection: on\n")),
        (
            "runbooks/deploy.md",
            &engram("Deploy", "runbooks/deploy", "- [fact] deploys run at night"),
        ),
    ])
    .await;
    let on = ReflectionState::On {
        home: crystalline_service::ReflectionHome::Local,
    };
    assert_eq!(fx.engine.reflection_state("eng").await.unwrap(), on);
    let store = fx.engine.store();
    let _held = store.lock().await;
    let budget = std::time::Duration::from_millis(200);
    let state = tokio::time::timeout(budget, fx.engine.reflection_state("eng"))
        .await
        .expect("the state answers while the store lock is held")
        .unwrap();
    assert_eq!(state, on);
    let effective = tokio::time::timeout(budget, fx.engine.reflection_effective("eng"))
        .await
        .expect("the mode answers while the store lock is held")
        .unwrap();
    assert!(effective);
}

// --- The team marker on a domain that is not connected here ------------------

/// A plain clone of a team repository, registered as a local domain: its
/// MANIFEST carries `team: github`, and nothing connects it here. Switching
/// Reflection on is refused through every surface that switches it, and the
/// refusal says what to do instead.
#[tokio::test]
async fn an_unconnected_team_domain_refuses_the_switch() {
    let fx = fixture(&[("MANIFEST.md", &manifest("team: github\n"))]).await;
    let err = fx
        .engine
        .set_manifest_policies("eng", &on(), &Scope::Unrestricted)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("'eng' is a team domain") && err.contains("not connected here"),
        "{err}"
    );
    assert!(
        err.contains("crystalline domain add eng --origin <owner>/<repo>")
            && err.contains("at the team server"),
        "{err}"
    );
    let set = std::collections::BTreeMap::from([("reflection".to_string(), "on".to_string())]);
    let err = fx
        .engine
        .change_domain_settings("eng", &set, &[], &Scope::Unrestricted)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("'eng' is a team domain"), "{err}");
    // A raw MANIFEST edit meets the same screen.
    let err = fx
        .engine
        .edit_engram(&EditParams {
            identifier: "manifest".into(),
            domain: "eng".into(),
            operation: "find_replace".into(),
            find_text: Some("team: github\n".into()),
            content: Some("team: github\nreflection: on\n".into()),
            ..Default::default()
        })
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("'eng' is a team domain"), "{err}");
    let text = std::fs::read_to_string(fx.root.join("MANIFEST.md")).unwrap();
    assert!(!text.contains("reflection: on"), "{text}");
    assert!(!fx.root.join("reflections/PACT.md").exists());
}

/// One edit that drops the marker and switches Reflection on is screened
/// against the text it replaces, so it is refused too.
#[tokio::test]
async fn dropping_the_marker_in_the_switching_edit_is_still_refused() {
    let fx = fixture(&[("MANIFEST.md", &manifest("team: github\n"))]).await;
    let err = fx
        .engine
        .edit_engram(&EditParams {
            identifier: "manifest".into(),
            domain: "eng".into(),
            operation: "find_replace".into(),
            find_text: Some("team: github\n".into()),
            content: Some("reflection: on\n".into()),
            ..Default::default()
        })
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("'eng' is a team domain"), "{err}");
}

/// A personal backup repository of a local domain: a `.git` folder with a
/// GitHub remote, and no team marker. Reflection switches on as it always
/// did, with no home, and is served here.
#[tokio::test]
async fn a_local_backup_repository_without_the_marker_switches_on() {
    let fx = fixture(&[(
        ".git/config",
        "[remote \"origin\"]\n\turl = git@github.com:jordi/notes-backup.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n",
    )])
    .await;
    fx.engine
        .set_manifest_policies("eng", &on(), &Scope::Unrestricted)
        .await
        .unwrap();
    let text = std::fs::read_to_string(fx.root.join("MANIFEST.md")).unwrap();
    assert!(
        text.contains("reflection: on") && !text.contains("reflection_home"),
        "{text}"
    );
    assert_eq!(
        fx.engine.reflection_state("eng").await.unwrap(),
        ReflectionState::On {
            home: crystalline_service::ReflectionHome::Local
        }
    );
    fx.engine
        .read_reflection_tree("eng", &ReflectionCaller::owner())
        .await
        .unwrap();
}

/// A clone whose MANIFEST already says `reflection: on` (a hand edit, or a
/// pull of somebody's push): the gate of every reflection verb refuses, and
/// the state reads as homeless, so no Reflection runs on the stray copy.
/// The engram side keeps working.
#[tokio::test]
async fn the_gate_refuses_an_unconnected_team_domain_that_says_on() {
    let fx = fixture(&[
        ("MANIFEST.md", &manifest("team: github\nreflection: on\n")),
        ("reflections/page.md", &page("Page")),
    ])
    .await;
    assert_eq!(
        fx.engine.reflection_state("eng").await.unwrap(),
        ReflectionState::Homeless
    );
    let err = fx
        .engine
        .read_reflection_tree("eng", &ReflectionCaller::owner())
        .await
        .unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains(crystalline_service::engine::REFUSED_UNCONNECTED_TEAM)
            && text.starts_with("'eng' is a team domain"),
        "{text}"
    );
    // Switching on again (the homeless refill) is refused the same way and
    // writes no home.
    let err = fx
        .engine
        .set_manifest_policies("eng", &on(), &Scope::Unrestricted)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("'eng' is a team domain"), "{err}");
    let manifest_text = std::fs::read_to_string(fx.root.join("MANIFEST.md")).unwrap();
    assert!(
        !manifest_text.contains("reflection_home"),
        "{manifest_text}"
    );
    // The engrams are still there to read.
    fx.engine
        .read_engram(
            &crystalline_service::params::ReadParams {
                identifier: "manifest".into(),
                domain: Some("eng".into()),
                share_link: None,
                include_images: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
}

/// A plain clone on the team server's own machine: its MANIFEST carries the
/// marker, says `reflection: on` and names this instance as the home, and
/// nothing connects it here. It is refused like any other stray clone. Once
/// the same domain is connected, the gate lets go and the Reflection is
/// served here, at its home.
#[tokio::test]
async fn a_clone_naming_this_instance_as_home_is_refused_until_it_is_connected() {
    let files = [
        (
            "MANIFEST.md",
            manifest("team: github\nreflection: on\nreflection_home: https://kb.example.com\n"),
        ),
        ("reflections/page.md", page("Page")),
    ];
    let files: Vec<(&str, &str)> = files.iter().map(|(p, t)| (*p, t.as_str())).collect();
    let fx = fixture_with(&files, |cfg, _| {
        cfg.service = Some(ServiceConfig {
            public_url: Some("https://kb.example.com".into()),
            ..Default::default()
        });
    })
    .await;
    assert_eq!(
        fx.engine.reflection_state("eng").await.unwrap(),
        ReflectionState::Homeless
    );
    let err = fx
        .engine
        .read_reflection_tree("eng", &ReflectionCaller::owner())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains(crystalline_service::engine::REFUSED_UNCONNECTED_TEAM),
        "{err}"
    );

    let fx = fixture_with(&files, |cfg, _| {
        with_origin(cfg, Some("https://kb.example.com"))
    })
    .await;
    assert_eq!(
        fx.engine.reflection_state("eng").await.unwrap(),
        ReflectionState::On {
            home: crystalline_service::ReflectionHome::Here
        }
    );
    fx.engine
        .read_reflection_tree("eng", &ReflectionCaller::owner())
        .await
        .unwrap();
}
