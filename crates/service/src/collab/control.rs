//! The session control channel: JSON riding `Message::Custom(CONTROL_TAG)`,
//! outside the reserved y-protocols types 0..=3. Both ends are ours; unknown
//! kinds decode to None so either side can grow messages without breaking the
//! other mid-upgrade.

use yrs::sync::Message;
use yrs::updates::encoder::Encode;

/// The `Message::Custom` tag the control channel rides. Must stay outside the
/// reserved y-protocols range 0..=3, and below 128: yrs writes a custom tag
/// with `write_u8` while the reserved types are varUints, and the two
/// encodings only agree under 128.
pub const CONTROL_TAG: u8 = 4;

/// One session control message. Server-to-client kinds carry state changes the
/// y-protocols messages have no room for; client-to-server kinds ask the
/// session for something outside the CRDT.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Control {
    /// Server greeting: the session identity a client checks on reconnect.
    Hello {
        /// This session's epoch; a change means the session restarted.
        epoch: String,
        /// The file's recorded line separator ("\r\n" or "\n").
        separator: String,
        /// The checksum of the last saved text.
        checksum: String,
        /// The engram's permalink, which a rename can move.
        permalink: String,
        /// The session's save state: "ok", "failed" or "conflict".
        save_state: String,
        /// Why saving is refused, when `save_state` is "failed" and the
        /// session has words for it. A joiner is told the standing failure
        /// here or not at all: the SaveFailed broadcast that carried it went
        /// out before this socket was subscribed, and the session suppresses
        /// a repeat of a refusal it has already announced.
        ///
        /// Skipped when absent so a client from before this field sees the
        /// greeting it has always seen; decoding is lenient the same way.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// A save landed; the checksum is the new CAS token.
    Saved {
        /// The checksum of the freshly saved text.
        checksum: String,
        /// The engram's permalink after the save.
        permalink: String,
    },
    /// A save was refused or failed; `detail` is shown to the user.
    SaveFailed {
        /// Why the save did not land.
        detail: String,
    },
    /// An external change was merged into the live session.
    Merged,
    /// An external change could not be merged; the client must resolve.
    Conflict {
        /// What kind of conflict this is ("edit" for colliding edits,
        /// "deleted" for an externally removed engram).
        conflict_kind: String,
        /// The other side's text, when there is one to show.
        theirs: Option<String>,
        /// A human explanation of the conflict.
        detail: String,
    },
    /// The session is closing; `reason` says why.
    Closed {
        /// Why the session closed ("deleted", "shutdown", ...).
        reason: String,
    },
    /// Client request: save now rather than on the debounce.
    Flush,
    /// Client request: resolve a standing conflict with this choice.
    Resolve {
        /// The resolution the user picked ("mine", "theirs", ...).
        choice: String,
    },
}

/// Frame a control message as `Message::Custom(CONTROL_TAG, json)`.
pub fn encode(control: &Control) -> Vec<u8> {
    let json = serde_json::to_vec(control).expect("control serializes");
    Message::Custom(CONTROL_TAG, json).encode_v1()
}

/// Parse a control payload, leniently: unknown kinds and broken JSON are
/// `None` rather than an error, so either end can grow messages first.
pub fn decode(payload: &[u8]) -> Option<Control> {
    serde_json::from_slice(payload).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use yrs::updates::decoder::Decode;

    #[test]
    fn controls_round_trip_through_the_custom_message() {
        for control in [
            Control::Hello {
                epoch: "1a2b.1".into(),
                separator: "\r\n".into(),
                checksum: "abc".into(),
                permalink: "notes/alpha".into(),
                save_state: "ok".into(),
                detail: None,
            },
            Control::Hello {
                epoch: "1a2b.1".into(),
                separator: "\n".into(),
                checksum: "abc".into(),
                permalink: "notes/alpha".into(),
                save_state: "failed".into(),
                detail: Some("the document carries no frontmatter".into()),
            },
            Control::Saved {
                checksum: "def".into(),
                permalink: "alpha".into(),
            },
            Control::SaveFailed {
                detail: "the document carries no frontmatter".into(),
            },
            Control::Merged,
            Control::Conflict {
                conflict_kind: "edit".into(),
                theirs: Some("---\ntitle: B\n---\n".into()),
                detail: "an agent rewrote this engram".into(),
            },
            Control::Closed {
                reason: "deleted".into(),
            },
            Control::Flush,
            Control::Resolve {
                choice: "mine".into(),
            },
        ] {
            let encoded = encode(&control);
            assert_eq!(encoded[0], CONTROL_TAG, "framed as Custom({CONTROL_TAG})");
            let yrs::sync::Message::Custom(tag, payload) =
                yrs::sync::Message::decode_v1(&encoded).unwrap()
            else {
                panic!("expected a custom message");
            };
            assert_eq!(tag, CONTROL_TAG);
            assert_eq!(decode(&payload), Some(control));
        }
    }

    #[test]
    fn a_healthy_hello_carries_no_detail_key_and_one_without_it_still_parses() {
        // Both directions of the compatibility the field was added under: a
        // room with nothing to explain sends the greeting it always sent, and
        // a greeting written before the field existed still decodes.
        let healthy = Control::Hello {
            epoch: "1a2b.1".into(),
            separator: "\n".into(),
            checksum: "abc".into(),
            permalink: "alpha".into(),
            save_state: "ok".into(),
            detail: None,
        };
        let json = serde_json::to_string(&healthy).unwrap();
        assert!(
            !json.contains("detail"),
            "no empty detail on the wire: {json}"
        );
        assert_eq!(decode(json.as_bytes()), Some(healthy));
        assert_eq!(
            decode(
                br#"{"kind":"hello","epoch":"e1","separator":"\n","checksum":"abc","permalink":"alpha","save_state":"failed"}"#
            ),
            Some(Control::Hello {
                epoch: "e1".into(),
                separator: "\n".into(),
                checksum: "abc".into(),
                permalink: "alpha".into(),
                save_state: "failed".into(),
                detail: None,
            })
        );
    }

    #[test]
    fn unknown_or_broken_control_json_is_none_not_a_panic() {
        assert_eq!(decode(b"{not json"), None);
        assert_eq!(decode(br#"{"kind":"from-the-future"}"#), None);
    }
}
