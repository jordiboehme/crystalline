//! The y-sync wire format, pinned byte-for-byte against a reference lib0
//! encoder written here from the protocol definition (y-protocols 1.0.7,
//! verified 2026-08-09; see research/2026-08-09-fluid-collab-protocol.md).
//! yrs::sync is the implementation the server speaks with; these tests are
//! what keeps a yrs upgrade from drifting the wire under the JS clients.

use yrs::ClientID;
use yrs::StateVector;
use yrs::sync::{Message, SyncMessage};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;

/// lib0 writeVarUint: 7-bit groups, least-significant first, high bit 0x80 as
/// the continuation flag, final byte with the high bit clear.
fn var_uint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    while value > 0x7f {
        out.push(0x80 | (value & 0x7f) as u8);
        value >>= 7;
    }
    out.push(value as u8);
    out
}

/// lib0 writeVarUint8Array: varUint byte length, then the raw bytes.
fn var_buf(bytes: &[u8]) -> Vec<u8> {
    let mut out = var_uint(bytes.len() as u64);
    out.extend_from_slice(bytes);
    out
}

#[test]
fn the_worked_varint_example_holds() {
    // The reference vector from the protocol research: 300 -> [0xAC, 0x02].
    assert_eq!(var_uint(300), vec![0xac, 0x02]);
    assert_eq!(var_uint(0), vec![0x00]);
    assert_eq!(var_uint(127), vec![0x7f]);
    assert_eq!(var_uint(128), vec![0x80, 0x01]);
}

#[test]
fn sync_step1_frames_as_type0_subtype0_with_a_state_vector() {
    // Outer varUint 0 (sync), inner varUint 0 (step1), varUint8Array payload.
    // An empty state vector encodes as a single 0x00 (zero clients).
    let message = Message::Sync(SyncMessage::SyncStep1(StateVector::default()));
    let mut expected = var_uint(0);
    expected.extend(var_uint(0));
    expected.extend(var_buf(&[0x00]));
    assert_eq!(message.encode_v1(), expected);
}

#[test]
fn sync_step2_and_update_frame_their_payloads_verbatim() {
    let payload = vec![1u8, 2, 3];
    let step2 = Message::Sync(SyncMessage::SyncStep2(payload.clone()));
    let mut expected = var_uint(0);
    expected.extend(var_uint(1));
    expected.extend(var_buf(&payload));
    assert_eq!(step2.encode_v1(), expected);

    let update = Message::Sync(SyncMessage::Update(payload.clone()));
    let mut expected = var_uint(0);
    expected.extend(var_uint(2));
    expected.extend(var_buf(&payload));
    assert_eq!(update.encode_v1(), expected);

    // A payload longer than 127 bytes exercises the multi-byte varUint on the
    // length prefix - the 300 example embedded in a real frame.
    let long = vec![0u8; 300];
    let framed = Message::Sync(SyncMessage::Update(long.clone())).encode_v1();
    let mut expected = var_uint(0);
    expected.extend(var_uint(2));
    expected.extend(var_buf(&long));
    assert_eq!(framed, expected);
    assert_eq!(
        &framed[2..4],
        &[0xac, 0x02],
        "the length prefix is varUint(300)"
    );
}

#[test]
fn awareness_updates_decode_from_reference_bytes_and_reencode_identically() {
    // One client (id 7, clock 1) whose state is the JSON string {"user":"a"};
    // composed with the reference encoder: varUint count, then per client
    // varUint clientID, varUint clock, varString stateJSON.
    let state_json = br#"{"user":"a"}"#;
    let mut inner = var_uint(1);
    inner.extend(var_uint(7));
    inner.extend(var_uint(1));
    inner.extend(var_buf(state_json)); // varString = varUint(len) + UTF-8
    let mut frame = var_uint(1); // outer type 1 = awareness
    frame.extend(var_buf(&inner));

    let decoded = Message::decode_v1(&frame).expect("the frame parses");
    let Message::Awareness(update) = decoded else {
        panic!("expected an awareness message");
    };
    let entry = update
        .clients
        .get(&ClientID::new(7))
        .expect("client 7 is present");
    assert_eq!(entry.clock, 1);
    assert_eq!(&*entry.json, r#"{"user":"a"}"#);
    // And the re-encode is byte-identical, so what yrs sends is what
    // y-protocols wrote.
    assert_eq!(Message::Awareness(update).encode_v1(), frame);
}

#[test]
fn a_removed_client_is_the_json_null_state() {
    // Removal on the awareness wire is the literal string "null".
    let mut inner = var_uint(1);
    inner.extend(var_uint(7));
    inner.extend(var_uint(2));
    inner.extend(var_buf(b"null"));
    let mut frame = var_uint(1);
    frame.extend(var_buf(&inner));
    let decoded = Message::decode_v1(&frame).expect("parses");
    let Message::Awareness(update) = decoded else {
        panic!("expected awareness");
    };
    assert_eq!(
        &*update.clients.get(&ClientID::new(7)).unwrap().json,
        "null"
    );
}

#[test]
fn concatenated_messages_in_one_frame_all_decode() {
    // Both sides may pack several protocol messages into one WS frame; the
    // reader loops. MessageReader is the iterator yrs ships for that.
    use yrs::sync::MessageReader;
    use yrs::updates::decoder::DecoderV1;

    let mut frame = Message::Sync(SyncMessage::SyncStep1(StateVector::default())).encode_v1();
    frame.extend(Message::Sync(SyncMessage::Update(vec![9, 9])).encode_v1());
    let mut decoder = DecoderV1::from(frame.as_slice());
    let messages: Vec<Message> = MessageReader::new(&mut decoder)
        .collect::<Result<_, _>>()
        .expect("both messages parse");
    assert_eq!(messages.len(), 2);
}

#[test]
fn the_custom_control_tag_stays_outside_the_reserved_range() {
    // Session control rides Message::Custom(4, json): 0..=3 are the protocol's
    // own types (sync, awareness, auth, queryAwareness) and must stay free.
    // NOTE: yrs encodes a Custom tag with write_u8 while the reserved types
    // are varUints - the two encodings produce identical bytes only for tags
    // below 128. CONTROL_TAG = 4 is safe; never raise it past 127.
    let message = Message::Custom(4, br#"{"kind":"flush"}"#.to_vec());
    let encoded = message.encode_v1();
    assert_eq!(encoded[0], 4);
    let decoded = Message::decode_v1(&encoded).expect("parses");
    assert_eq!(decoded, message);
}
