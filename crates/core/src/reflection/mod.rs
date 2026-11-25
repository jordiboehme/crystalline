//! The Reflection: a domain's documents for people under `reflections/`.
//!
//! Pure pieces only - paths and file classes, sections and their ids, the
//! provenance a reflection carries in its frontmatter, the baseline and the
//! feedback changes taken against it, where a thread's anchor stands, and
//! relative links. Deciding who may do
//! what, reading and writing files and the index are the engine's. A
//! reflection is never an engram: nothing here calls `parse_engram`, and a
//! page with no frontmatter at all is valid. What a reflection shares with an
//! engram (the heading walker, the body line walker, the link target scan,
//! the path classifier) lives beside the engram code and is used from here.

pub mod anchors;
pub mod assets;
pub mod baseline;
pub mod layout;
pub mod links;
pub mod page;
pub mod provenance;
pub mod reconcile;
pub mod sections;
pub mod state;

/// Why an import leaves out a file under `reflections/` where a Reflection
/// keeps that folder (the receiving domain's switch, or the switch the
/// imported MANIFEST declares): its pages are no engrams, and a Reflection
/// comes back from its backup branch. One sentence for every import road
/// (the engine's two verbs, the archive screen and `crystalline import`).
pub const REFLECTION_NOT_IMPORTED: &str = "under reflections/, which a Reflection keeps; a Reflection is restored from its backup branch, never imported as engrams";

pub use anchors::anchor_state;
pub use assets::{
    AssetTarget, classify_target, engram_attachment_embeds, reflection_asset_refs,
    reflection_page_refs, refuse_engram_attachments,
};
pub use baseline::{
    ChangeKind, Relinked, SectionChange, advance_baseline, baseline_ids, feedback_changes,
    parse_baseline, relink, render_baseline, same_but_task_boxes, seed_baseline,
};
pub use layout::{
    BASELINE_DIR, MATCHES_NO_PAGE, NESTED_NOT_READ, NOT_A_VALID_GLOB, PACT_FILE,
    READS_AS_A_SENTENCE, REFLECTIONS_DIR, ReflectionFile, Structure, StructureFinding,
    THREADS_SUFFIX, baseline_path, classify, domain_rel, fits_structure, is_pact_file,
    is_under_reflections, reflection_folders, short_name_refusal, structure_findings,
    structure_globs, threads_path, validate_new_reflection_path, validate_reflection_path,
};
pub use links::{rebase_links, relative_to, rewrite_asset_links, rewrite_links_to};
pub use page::{
    FOLDER_PARENT, FOLLOWS_CYCLE, FOLLOWS_KEY, FOLLOWS_MISSING, FOLLOWS_NO_PAGE,
    FOLLOWS_OTHER_DOMAIN, FRESHNESS_KEY, FRESHNESS_ON_OWNED_PAGE, FRESHNESS_TOO_SHORT,
    FRESHNESS_UNREADABLE, Freshness, MARK_UNREADABLE, MIN_FRESHNESS_SECONDS, OWNER_KEY,
    OWNER_MISSING, PAGE_KEYS, PAGE_PACT_SUFFIX, PactChain, PageFinding, PageLinks, PageSettings,
    PageTiming, RunSpan, STATUS_KEY, carry_page_keys, follows_refusal, is_loose_pact, pact_chain,
    pact_page_of, page_key_values, page_pact_path, parse_freshness, read_page_settings,
    repeated_page_keys, rewrite_follows, without_page_keys, would_cycle,
};
pub use provenance::{
    PAGE_RECORD_KEY, PROVENANCE_KEY, PageRecord, Provenance, REFRESH_REQUESTED_BY_KEY,
    REFRESH_REQUESTED_KEY, Retirement, SectionRecord, SourceName, SourceRef, block_as_written,
    parse_source_name, provenance_unreadable, read_provenance, respell_source_domains,
    source_domains_named, text_changed, title_of, write_provenance,
};
pub use reconcile::{
    CoverChange, cover_change, owned_pages, owner_refusal, pact_hash, pact_hashes, page_pact_hash,
    rewrite_owner,
};
pub use sections::{
    Section, SectionKeyMiss, TOP_SECTION, WrittenIds, assign_ids, drop_written_ids, edit_section,
    find_section, id_line, normalize_section_text, parse_id_line, random_section_id, reattach_ids,
    resolve_section_key, section_hash, split_sections,
};
pub use state::{ReflectionHome, ReflectionState};
