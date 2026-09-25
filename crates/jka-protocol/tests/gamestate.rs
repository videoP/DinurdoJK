use jka_protocol::{
    gamestate::{decode_initial_gamestate, read_delta_entity, EntityState, ENTITY_FIELDS},
    message::{MessageReader, MessageWriter},
};

const FIXTURE: &[u8] = include_bytes!("fixtures/gamestate.bin");

#[test]
fn decodes_reference_encoded_gamestate_without_losing_float_bits() {
    let state = decode_initial_gamestate(FIXTURE).unwrap();
    assert_eq!(state.reliable_acknowledge, 123);
    assert_eq!(state.preceding_commands.len(), 1);
    assert_eq!(state.preceding_commands[0].sequence, 450);
    assert_eq!(state.preceding_commands[0].text, b"print ready");
    assert_eq!(state.server_command_sequence, 456);
    assert_eq!(state.client_number, 2);
    assert_eq!(state.checksum_feed, 0x89abcdef);
    assert_eq!(state.configstrings.len(), 1);
    assert_eq!(state.configstrings[&0], b"\\mapname\\mp/ffa3");
    let baseline = &state.baselines[&7];
    assert_eq!(baseline.field_bits("pos.trTime"), Some(1234));
    assert_eq!(
        baseline.field_bits("pos.trBase[1]"),
        Some((-2.0f32).to_bits())
    );
    assert_eq!(baseline.field_bits("pos.trBase[0]"), Some(1.5f32.to_bits()));
    assert_eq!(state.consumed_bytes, FIXTURE.len());
}

#[test]
fn rejects_every_truncated_gamestate_prefix() {
    for length in 0..FIXTURE.len() {
        assert!(
            decode_initial_gamestate(&FIXTURE[..length]).is_err(),
            "length {length}"
        );
    }
}

#[test]
fn entity_removal_unchanged_and_invalid_field_counts() {
    let mut original = EntityState {
        number: 3,
        fields: [0; ENTITY_FIELDS.len()],
    };
    original.fields[0] = 42;
    let mut unchanged = MessageWriter::new(128).unwrap();
    unchanged.write_bits(0, 1).unwrap();
    unchanged.write_bits(0, 1).unwrap();
    let decoded = read_delta_entity(
        &mut MessageReader::new(unchanged.as_bytes()).unwrap(),
        Some(&original),
        7,
    )
    .unwrap()
    .unwrap();
    assert_eq!(decoded.number, 7);
    assert_eq!(decoded.fields, original.fields);
    let mut removed = MessageWriter::new(128).unwrap();
    removed.write_bits(1, 1).unwrap();
    assert!(read_delta_entity(
        &mut MessageReader::new(removed.as_bytes()).unwrap(),
        Some(&original),
        3
    )
    .unwrap()
    .is_none());
    let mut invalid = MessageWriter::new(128).unwrap();
    invalid.write_bits(0, 1).unwrap();
    invalid.write_bits(1, 1).unwrap();
    invalid.write_bits(255, 8).unwrap();
    assert!(read_delta_entity(
        &mut MessageReader::new(invalid.as_bytes()).unwrap(),
        None,
        3
    )
    .is_err());
    assert!(read_delta_entity(&mut MessageReader::new(&[]).unwrap(), None, 1024).is_err());
}

#[test]
fn rejects_invalid_configstrings_and_accepts_legacy_rmg_short() {
    for index in [-1, 1700] {
        let mut writer = MessageWriter::new(128).unwrap();
        for (value, bits) in [(0, 32), (2, 8), (0, 32), (3, 8), (index, 16)] {
            writer.write_bits(value, bits).unwrap();
        }
        assert!(decode_initial_gamestate(writer.as_bytes()).is_err());
    }
    let mut writer = MessageWriter::new(128).unwrap();
    for (value, bits) in [
        (0, 32),
        (2, 8),
        (0, 32),
        (10, 8),
        (0, 32),
        (0, 32),
        (1, 16),
        (10, 8),
    ] {
        writer.write_bits(value, bits).unwrap();
    }
    assert!(decode_initial_gamestate(writer.as_bytes()).is_ok());
}
