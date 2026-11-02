use super::*;
use crate::message::MessageWriter;

fn string(w: &mut MessageWriter, value: &[u8]) {
    for &byte in value {
        w.write_bits(i32::from(byte), 8).unwrap();
    }
    w.write_bits(0, 8).unwrap();
}

fn packet(body: impl FnOnce(&mut MessageWriter)) -> Vec<u8> {
    let mut w = MessageWriter::new(MAX_MESSAGE_BYTES).unwrap();
    w.write_bits(17, 32).unwrap();
    body(&mut w);
    w.write_bits(i32::from(SVC_EOF), 8).unwrap();
    w.as_bytes().to_vec()
}

fn gamestate(version: Version) -> Vec<u8> {
    packet(|w| {
        w.write_bits(i32::from(SVC_GAMESTATE), 8).unwrap();
        w.write_bits(100, 32).unwrap();
        w.write_bits(i32::from(SVC_CONFIGSTRING), 8).unwrap();
        w.write_bits(0, 16).unwrap();
        string(
            w,
            format!(
                "\\protocol\\{}\\version\\JK2MP: v{}\\mapname\\ffa_yavin",
                version.protocol(),
                version.name()
            )
            .as_bytes(),
        );
        w.write_bits(i32::from(SVC_BASELINE), 8).unwrap();
        w.write_bits(5, 10).unwrap();
        write_entity(
            w,
            version,
            &[
                ("pos.trTime", 123),
                ("modelindex", 255),
                ("modelGhoul2", 15),
            ],
        );
        w.write_bits(i32::from(SVC_EOF), 8).unwrap();
        w.write_bits(3, 32).unwrap();
        w.write_bits(0x12345678, 32).unwrap();
        // No JKA RMG short, and the outer EOF is 9 rather than 10.
    })
}

fn write_values(
    w: &mut MessageWriter,
    schema: &[(&str, i8)],
    values: &[(&str, u32)],
    entity: bool,
) {
    let count = schema
        .iter()
        .rposition(|&(key, _)| values.iter().any(|&(name, _)| key == name))
        .map_or(0, |i| i + 1);
    w.write_bits(count as i32, 8).unwrap();
    for &(name, width) in schema.iter().take(count) {
        let value = values
            .iter()
            .find(|&&(key, _)| key == name)
            .map(|&(_, value)| value);
        w.write_bits(i32::from(value.is_some()), 1).unwrap();
        let Some(value) = value else { continue };
        if entity {
            w.write_bits(i32::from(value != 0), 1).unwrap();
            if value == 0 {
                continue;
            }
        }
        if width == 0 {
            w.write_bits(1, 1).unwrap();
            w.write_bits(value as i32, 32).unwrap();
        } else {
            w.write_bits(value as i32, width).unwrap();
        }
    }
}

fn write_entity(w: &mut MessageWriter, version: Version, values: &[(&str, u32)]) {
    w.write_bits(0, 1).unwrap(); // not removed
    w.write_bits(1, 1).unwrap(); // delta follows
    write_values(w, version.entity_fields(), values, true);
}

fn snapshot(version: Version, time: i32, distance: i32, arrays: bool, remove: bool) -> Vec<u8> {
    packet(|w| {
        w.write_bits(i32::from(SVC_SNAPSHOT), 8).unwrap();
        w.write_bits(time, 32).unwrap();
        w.write_bits(distance, 8).unwrap();
        w.write_bits(0, 8).unwrap(); // snapflags
        w.write_bits(2, 8).unwrap(); // areamask length
        w.write_bits(0x55, 8).unwrap();
        w.write_bits(0xaa, 8).unwrap();
        write_values(
            w,
            version.player_fields(),
            &[
                ("commandTime", (time - 1) as u32),
                ("origin[0]", (-0.0f32).to_bits()),
                ("origin[2]", 80.5f32.to_bits()),
                ("weaponTime", (-12i32) as u32),
                ("weapon", 2),
                ("saberActive", 1),
                ("saberMove", 44),
            ],
            false,
        );
        w.write_bits(i32::from(arrays), 1).unwrap();
        if arrays {
            for (mask, width, value) in [
                (1 << 4, 16, -1),
                (1 << 5, 16, -2),
                (1 << 8, 16, -32768),
                (1 << 15, 32, 0x1234abcd),
            ] {
                w.write_bits(1, 1).unwrap();
                w.write_bits(mask, 16).unwrap();
                w.write_bits(value, width).unwrap();
            }
        }
        w.write_bits(5, 10).unwrap();
        w.write_bits(i32::from(remove), 1).unwrap();
        if !remove {
            w.write_bits(0, 1).unwrap();
        } // unchanged baseline/delta
        w.write_bits(i32::from(ENTITY_NONE), 10).unwrap();
    })
}

#[test]
fn prediction_history_is_raw_sequence_checked_and_cleared_by_gamestate() {
    for version in [Version::V102, Version::V104] {
        let mut decoder = Decoder::new();
        decoder.parse_packet(1, &gamestate(version)).unwrap();
        decoder.parse_packet(2, &snapshot(version, 1000, 0, true, false)).unwrap();
        let raw = decoder.snapshot(2).unwrap();
        assert_eq!(raw.player_state.fields.field_i32("weapon"), Some(2));
        assert_eq!(raw.player_state.fields.field_bits("origin[0]"), Some((-0.0_f32).to_bits()));
        assert!(decoder.snapshot(2 + PACKET_BACKUP as i32).is_none());
        decoder.parse_packet(2 + PACKET_BACKUP as i32, &snapshot(version, 2000, 0, false, false)).unwrap();
        assert!(decoder.snapshot(2).is_none());
        decoder.parse_packet(3 + PACKET_BACKUP as i32, &gamestate(version)).unwrap();
        assert!(decoder.snapshot(2 + PACKET_BACKUP as i32).is_none());
    }
}

#[test]
fn live_bootstrap_consumes_snapshots_without_publishing_or_identifying_gamestate() {
    for version in [Version::V102, Version::V104] {
        let early = snapshot(version, 1000, 0, true, false);
        // Offline/demo streams still require their identifying gamestate.
        assert!(Decoder::new().parse_packet(10, &early).is_err());
        let mut live = Decoder::for_live(version);
        let result = live.parse_packet(10, &early).unwrap();
        assert_eq!(result.events, [Event::UnprimedSnapshot { message_num:10, server_time:1000 }]);
        assert_eq!(result.reliable_acknowledge, 17);
        assert_eq!(live.version(), None);
        assert!(!live.have_gamestate);
        assert!(live.snapshots.iter().all(Option::is_none));
        live.parse_packet(11, &gamestate(version)).unwrap();
        let result = live.parse_packet(12, &snapshot(version, 1100, 2, false, false)).unwrap();
        assert!(matches!(result.events[0], Event::DroppedSnapshot { delta_num:10, .. }));
        let result = live.parse_packet(13, &snapshot(version, 1150, 0, true, false)).unwrap();
        assert!(matches!(result.events[0], Event::Snapshot(_)));
    }
}

#[test]
fn live_bootstrap_still_validates_the_complete_snapshot() {
    let invalid = packet(|w| {
        w.write_bits(i32::from(SVC_SNAPSHOT), 8).unwrap();
        w.write_bits(1000, 32).unwrap();
        w.write_bits(0, 8).unwrap();
        w.write_bits(2, 8).unwrap(); // inactive
        w.write_bits(33, 8).unwrap(); // invalid areamask length
    });
    assert!(Decoder::for_live(Version::V104).parse_packet(10, &invalid).is_err());
}

#[test]
fn jk2_layouts_preserve_raw_bits_baselines_and_signed_array_values() {
    for version in [Version::V102, Version::V104] {
        let mut decoder = Decoder::new();
        decoder.parse_packet(10, &gamestate(version)).unwrap();
        assert_eq!(decoder.version(), Some(version));
        assert_eq!(decoder.client_number, 3);
        assert_eq!(decoder.checksum_feed, 0x12345678);
        let decoded = decoder
            .parse_packet(11, &snapshot(version, 1000, 0, true, false))
            .unwrap();
        let Event::Snapshot(snap) = &decoded.events[0] else {
            panic!("expected full snapshot")
        };
        assert_eq!(
            snap.player_state.fields.field_bits("origin[0]"),
            Some(0x80000000)
        );
        assert_eq!(snap.player_state.fields.field_f32("origin[2]"), Some(80.5));
        assert_eq!(snap.player_state.fields.field_i32("weaponTime"), Some(-12));
        assert_eq!(snap.player_state.stats[4], -1); // 16 bits; not JKA's 19-bit weapon mask
        assert_eq!(snap.player_state.persistant[5], -2);
        assert_eq!(snap.player_state.ammo[8], -32768);
        assert_eq!(snap.player_state.powerups[15], 0x1234abcd);
        assert_eq!(snap.area_mask[..3], [0x55, 0xaa, 0]);
        assert_eq!(snap.entities[0].fields.field_i32("pos.trTime"), Some(123));
        assert_eq!(snap.entities[0].fields.field_i32("modelindex"), Some(-1));
        let decoded = decoder
            .parse_packet(12, &snapshot(version, 1050, 1, false, true))
            .unwrap();
        let Event::Snapshot(snap) = &decoded.events[0] else {
            panic!("expected delta snapshot")
        };
        assert!(snap.entities.is_empty());
        assert_eq!(snap.delta_num, Some(11));
        assert_eq!(snap.player_state.stats[4], -1);
    }
}

#[test]
fn missing_delta_is_dropped_and_a_full_snapshot_recovers() {
    let version = Version::V104;
    let mut decoder = Decoder::new();
    decoder.parse_packet(10, &gamestate(version)).unwrap();
    decoder
        .parse_packet(11, &snapshot(version, 1000, 0, false, false))
        .unwrap();
    let result = decoder
        .parse_packet(50, &snapshot(version, 1200, 1, false, false))
        .unwrap();
    assert!(matches!(
        result.events[0],
        Event::DroppedSnapshot { delta_num: 49, .. }
    ));
    let result = decoder
        .parse_packet(51, &snapshot(version, 1250, 0, false, false))
        .unwrap();
    assert!(matches!(result.events[0], Event::Snapshot(_)));
    // A new gamestate invalidates every prior snapshot base.
    decoder.parse_packet(52, &gamestate(version)).unwrap();
    let result = decoder
        .parse_packet(53, &snapshot(version, 1300, 2, false, false))
        .unwrap();
    assert!(matches!(
        result.events[0],
        Event::DroppedSnapshot { delta_num: 51, .. }
    ));
}

#[test]
fn malformed_packets_fail_at_the_wire_boundary() {
    for version in [Version::V102, Version::V104] {
        let bytes = gamestate(version);
        for len in 0..bytes.len() {
            assert!(
                Decoder::new().parse_packet(10, &bytes[..len]).is_err(),
                "accepted prefix {len}"
            );
        }
        let mut decoder = Decoder::new();
        decoder.parse_packet(10, &bytes).unwrap();
        assert!(decoder
            .parse_packet(11, &vec![0; MAX_MESSAGE_BYTES + 1])
            .is_err());
        assert!(Decoder::new()
            .parse_packet(11, &snapshot(version, 1, 0, false, false))
            .is_err());
        let invalid = packet(|w| {
            w.write_bits(10, 8).unwrap();
        }); // JKA EOF is not JK2 EOF
        assert!(decoder.parse_packet(11, &invalid).is_err());
    }
}

#[test]
fn version_detection_never_assumes_protocol_15_is_always_102() {
    assert_eq!(
        Version::from_serverinfo(br"\protocol\15\version\JK2MP: v1.02"),
        Ok(Version::V102)
    );
    assert_eq!(
        Version::from_serverinfo(br"\protocol\16\version\JK2MP: v1.04"),
        Ok(Version::V104)
    );
    assert!(Version::from_serverinfo(br"\protocol\15\version\JK2MP: v1.03").is_err());
    assert!(Version::from_serverinfo(br"\protocol\26").is_err());
}

#[test]
fn real_102_and_104_recordings_decode_through_full_and_delta_snapshots() {
    let cases: &[(&[u8], Version, &str)] = &[
        (
            include_bytes!("../../tests/fixtures/jk2-102-prefix.dm_15"),
            Version::V102,
            "ctf_yavin",
        ),
        (
            include_bytes!("../../tests/fixtures/jk2-104-prefix.dm_16"),
            Version::V104,
            "ctf_imperial",
        ),
    ];
    for &(bytes, version, map) in cases {
        let report = demo::inspect(bytes).unwrap();
        assert_eq!(report.records, 16);
        assert_eq!(report.segments.len(), 1);
        let segment = &report.segments[0];
        assert_eq!(segment.version, version);
        assert_eq!(segment.map, map);
        assert!(segment.snapshots > 1);
        assert!(segment.full_snapshots >= 1);
        assert!(segment.snapshots > segment.full_snapshots);
        assert_eq!(segment.dropped_snapshots, 0);
        assert_eq!(segment.nonfinite_snapshots, 0);
        // A missing demo end marker must never be reported as complete.
        assert!(demo::inspect(&bytes[..bytes.len() - 1]).is_err());
    }
}
