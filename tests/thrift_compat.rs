//! Golden vectors produced by upstream linejs' `writeThrift` (node `thrift`
//! 0.20) to pin byte-for-byte wire compatibility.

use line_rs::thrift::{read_message, write_message, Protocol, TType, TValue};

fn sample() -> Vec<(i16, TValue)> {
    vec![
        (1, TValue::I32(-5)),
        (2, TValue::string("hello")),
        (3, TValue::Bool(true)),
        (4, TValue::I64(618946633287860670)),
        (20, TValue::Double(1.5)),
        (
            21,
            TValue::Struct(vec![
                (1, TValue::Bool(false)),
                (2, TValue::List(TType::String, vec![TValue::string("a"), TValue::string("b")])),
            ]),
        ),
        (22, TValue::Map(TType::String, TType::I32, vec![(TValue::string("k"), TValue::I32(7))])),
        (23, TValue::List(TType::Bool, vec![TValue::Bool(true), TValue::Bool(false)])),
        (30, TValue::I64(-1)),
    ]
}

const COMPACT: &str = "8221000b73656e644d6573736167651509180568656c6c6f1116fc86e0c0e0abf896110728000000000000f83f1c\
12192801610162001b0185016b0e1921010276010000";
const BINARY: &str = "800100010000000b73656e644d65737361676500000000080001fffffffb0b00020000000568656c6c6f020003010a\
00040896f0af040c01be0400143ff80000000000000c0015020001000f00020b0000000200000001610000000162000d00160b080000000100\
0000016b000000070f0017020000000201000a001effffffffffffffff0000";

#[test]
fn compact_matches_linejs() {
    assert_eq!(hex::encode(write_message("sendMessage", &sample(), Protocol::Compact).unwrap()), COMPACT);
}

#[test]
fn binary_matches_linejs() {
    assert_eq!(hex::encode(write_message("sendMessage", &sample(), Protocol::Binary).unwrap()), BINARY);
}

#[test]
fn empty_args_match_linejs() {
    assert_eq!(hex::encode(write_message("getProfile", &[], Protocol::Compact).unwrap()), "8221000a67657450726f66696c6500");
}

#[test]
fn golden_vectors_decode() {
    for (hex_str, p) in [(COMPACT, Protocol::Compact), (BINARY, Protocol::Binary)] {
        let msg = read_message(&hex::decode(hex_str).unwrap(), p).unwrap();
        assert_eq!(msg.name, "sendMessage");
        assert_eq!(msg.body, TValue::Struct(sample()));
    }
}
