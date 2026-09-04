//! Shared payload builders for the proxy bench and the callgrind driver
//! (scripts/profile_callgrind.sh driver). Single source of truth so the
//! profiled shape matches the measured bench shape exactly.

use std::collections::HashMap;

use prost_reflect_tests::proto::{ComplexType, Scalars};

pub fn scalars_sample() -> Scalars {
    Scalars {
        double: 1.1,
        float: 2.2,
        int32: -3,
        int64: 4,
        uint32: 5,
        uint64: 6,
        sint32: -7,
        sint64: 8,
        fixed32: 9,
        fixed64: 10,
        sfixed32: -11,
        sfixed64: 12,
        bool: true,
        string: "hello".to_owned(),
        bytes: b"world".to_vec(),
    }
}

pub fn complex_sample() -> ComplexType {
    ComplexType {
        string_map: HashMap::from_iter([
            ("one".to_owned(), scalars_sample()),
            ("two".to_owned(), scalars_sample()),
        ]),
        int_map: HashMap::from_iter([(1, scalars_sample())]),
        nested: Some(scalars_sample()),
        my_enum: vec![0, 1, 3],
        optional_enum: 1,
        enum_map: HashMap::from_iter([(1, 3), (2, 0)]),
    }
}
