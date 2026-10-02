//! The always-on pointer text and the Kiro steering file.
//!
//! The pointer is never the routing block. It names no domain and only tells
//! the agent to fetch the block when it is missing, so a harness that gets
//! both the pointer and the block from a hook still sees the routing once.

/// The fixed pointer a rules file carries (spec 3.4), verbatim.
pub const ALWAYS_ON_POINTER: &str = "Crystalline is your crystallized intelligence across sessions. If the Crystalline knowledge routing block is not already in your context, call the crystalline tool list_domains with include_routing=true before you answer from memory or start a task.";

/// The line that marks a steering file install wrote and may replace.
pub const STEERING_MARKER: &str = "<!-- managed by crystalline install; edits are replaced -->";

/// The whole text of the owned Kiro steering file: the `inclusion: always`
/// frontmatter, the marker and the pointer.
pub fn steering_file_text() -> String {
    format!("---\ninclusion: always\n---\n{STEERING_MARKER}\n{ALWAYS_ON_POINTER}\n")
}

/// Whether a steering file holds the marker line, so install wrote it.
pub fn is_managed_steering(text: &str) -> bool {
    text.lines()
        .any(|line| line.trim_end_matches('\r') == STEERING_MARKER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pointer_is_the_spec_sentence_and_names_no_domain() {
        assert_eq!(
            ALWAYS_ON_POINTER,
            "Crystalline is your crystallized intelligence across sessions. If the Crystalline knowledge routing block is not already in your context, call the crystalline tool list_domains with include_routing=true before you answer from memory or start a task."
        );
        assert!(!ALWAYS_ON_POINTER.contains("domain:"));
        assert_eq!(
            STEERING_MARKER,
            "<!-- managed by crystalline install; edits are replaced -->"
        );
    }

    #[test]
    fn the_steering_file_is_always_included_and_carries_the_marker() {
        let text = steering_file_text();
        assert!(text.starts_with("---\ninclusion: always\n---\n"), "{text}");
        assert!(text.contains(STEERING_MARKER));
        assert!(text.contains(ALWAYS_ON_POINTER));
        assert_eq!(
            text,
            format!("---\ninclusion: always\n---\n{STEERING_MARKER}\n{ALWAYS_ON_POINTER}\n")
        );
        assert!(is_managed_steering(&text));
        let without = text.replace(&format!("{STEERING_MARKER}\n"), "");
        assert!(!is_managed_steering(&without));
    }
}
