use jka_protocol::message::{ErrorKind, MessageReader, MessageWriter};

#[test]
fn matches_compiled_openjk_vectors_in_both_directions() {
    let mut count = 0;
    for line in include_str!("fixtures/codec-vectors.tsv")
        .lines()
        .filter(|line| !line.starts_with('#'))
    {
        let fields: Vec<_> = line.split('\t').collect();
        let prefix: i8 = fields[0].parse().unwrap();
        let width: i8 = fields[1].parse().unwrap();
        let input: u32 = fields[2].parse().unwrap();
        let expected: i32 = fields[3].parse().unwrap();
        let bits: usize = fields[4].parse().unwrap();
        let length: usize = fields[5].parse().unwrap();
        let data: Vec<u8> = (0..fields[6].len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&fields[6][i..i + 2], 16).unwrap())
            .collect();
        assert_eq!(length, data.len());
        let mut reader = MessageReader::new(&data).unwrap();
        let mut writer = MessageWriter::new(128).unwrap();
        if prefix > 0 {
            let prefix_value = 0x55 & ((1 << prefix) - 1);
            assert_eq!(reader.read_bits(prefix).unwrap(), prefix_value);
            writer.write_bits(prefix_value, prefix).unwrap();
        }
        assert_eq!(reader.read_bits(width).unwrap(), expected, "{line}");
        assert_eq!(reader.read_byte().unwrap(), 10);
        assert_eq!(reader.bit_position(), bits);
        assert_eq!(reader.legacy_read_count(), length);
        writer.write_bits(input as i32, width).unwrap();
        writer.write_bits(10, 8).unwrap();
        assert_eq!(writer.bit_position(), bits);
        assert_eq!(writer.as_bytes(), data, "{line}");
        count += 1;
    }
    assert_eq!(count, 2016);
}

#[test]
fn every_byte_roundtrips_at_every_alignment() {
    for alignment in 0..8 {
        let mut writer = MessageWriter::new(4096).unwrap();
        if alignment > 0 {
            writer.write_bits(0, alignment).unwrap();
        }
        for byte in 0..256 {
            writer.write_bits(byte, 8).unwrap();
        }
        let mut reader = MessageReader::new(writer.as_bytes()).unwrap();
        if alignment > 0 {
            reader.read_bits(alignment).unwrap();
        }
        for byte in 0..256 {
            assert_eq!(i32::from(reader.read_byte().unwrap()), byte);
        }
    }
}

#[test]
fn errors_are_bounded_and_field_operations_are_transactional() {
    for width in [0, -32, 33, -128, 127] {
        assert_eq!(
            MessageReader::new(&[0])
                .unwrap()
                .read_bits(width)
                .unwrap_err()
                .kind,
            ErrorKind::InvalidWidth(width)
        );
    }
    let mut reader = MessageReader::new(&[]).unwrap();
    assert_eq!(
        reader.read_long().unwrap_err().kind,
        ErrorKind::UnexpectedEnd
    );
    assert_eq!(reader.bit_position(), 0);
    let mut writer = MessageWriter::new(1).unwrap();
    writer.write_bits(1, 1).unwrap();
    let old = writer.as_bytes().to_vec();
    assert!(writer.write_bits(-1, 32).is_err());
    assert_eq!(writer.as_bytes(), old);
    assert_eq!(writer.bit_position(), 1);
    assert!(MessageReader::new(&vec![0; 49153]).is_err());
    assert!(MessageWriter::new(49153).is_err());
}

#[test]
fn strings_preserve_bytes_and_require_a_bounded_terminator() {
    let mut writer = MessageWriter::new(128).unwrap();
    for byte in b"a\xe9%\0" {
        writer.write_bits(i32::from(*byte), 8).unwrap();
    }
    let mut reader = MessageReader::new(writer.as_bytes()).unwrap();
    assert_eq!(reader.read_string(4).unwrap(), b"a\xe9%");
    assert!(MessageReader::new(writer.as_bytes())
        .unwrap()
        .read_string(3)
        .is_err());
    for length in 0..writer.as_bytes().len() {
        assert!(MessageReader::new(&writer.as_bytes()[..length])
            .unwrap()
            .read_string(4)
            .is_err());
    }
}
