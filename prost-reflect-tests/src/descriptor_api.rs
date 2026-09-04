//! Public-API contract tests for the descriptor and dynamic-message
//! surfaces.
//!
//! Origin: the tier1-perf mutant sweep (888 mutants over the dynamic +
//! descriptor layers, `--test-package prost-reflect-tests`). ~250
//! survivors concentrated in public accessor/predicate families no test
//! exercised: extension descriptors, by-name/by-number message accessors,
//! the Value/MapKey accessor matrices, pool serialization, and
//! service/method/oneof descriptors. Wire-semantics mutants in message.rs
//! (encode/decode/merge arms) are covered by the conformance integration
//! test and are deliberately not duplicated here. Mutants of pub(crate)
//! internals (KindIndex predicates, FieldDescriptorLike impl methods) are
//! unkillable from an integration-test crate by construction and are also
//! not asserted here.
//!
//! All lookups go through the public API against the test corpus
//! (desc.proto, test.proto, test2.proto, ext.proto, options.proto), so
//! these tests pin the public contract, not internals.

use prost::Message;
use prost_reflect::{
    DescriptorPool, DynamicMessage, FieldDescriptor, Kind, MapKey, MessageDescriptor, Value,
};

use crate::test_file_descriptor;

fn pool() -> DescriptorPool {
    test_file_descriptor()
}

fn msg(pool: &DescriptorPool, name: &str) -> MessageDescriptor {
    pool.get_message_by_name(name)
        .unwrap_or_else(|| panic!("message {name}"))
}

fn fld(m: &MessageDescriptor, name: &str) -> FieldDescriptor {
    m.get_field_by_name(name)
        .unwrap_or_else(|| panic!("field {name}"))
}

// ---------------------------------------------------------------------------
// File / service / method / oneof descriptors
// ---------------------------------------------------------------------------

#[test]
fn service_method_and_oneof_descriptor_contract() {
    let pool = pool();
    let file = pool
        .files()
        .find(|f| f.name().ends_with("desc.proto"))
        .expect("desc.proto in pool");

    let services: Vec<_> = file.services().collect();
    assert_eq!(services.len(), 1);
    let service = services[0].clone();
    assert_eq!(service.name(), "MyService");
    assert_eq!(service.full_name(), "my.package.MyService");
    assert_eq!(service.index(), 0);
    assert_eq!(service.path(), &[6, 0]);

    let methods: Vec<_> = service.methods().collect();
    assert_eq!(methods.len(), 1);
    let method = methods[0].clone();
    assert_eq!(method.name(), "MyMethod");
    assert_eq!(method.full_name(), "my.package.MyService.MyMethod");
    assert_eq!(method.index(), 0);
    assert_eq!(method.path(), &[6, 0, 2, 0]);
    assert!(!method.is_client_streaming());
    assert!(!method.is_server_streaming());
    assert_eq!(method.input().full_name(), "my.package.MyMessage");
    assert_eq!(method.output().full_name(), "my.package.MyMessage");

    let message = msg(&pool, "my.package.MyMessage");
    assert_eq!(message.name(), "MyMessage");
    // file.index() matches its position in the pool's file order
    let files: Vec<_> = pool.files().collect();
    let pos = files
        .iter()
        .position(|f| f.name() == file.name())
        .expect("file");
    assert_eq!(file.index(), pos);
    assert_eq!(message.full_name(), "my.package.MyMessage");
    assert_eq!(message.path(), &[4, 0]);
    let oneofs: Vec<_> = message.oneofs().collect();
    assert_eq!(oneofs.len(), 1);
    let oneof = oneofs[0].clone();
    assert_eq!(oneof.name(), "my_oneof");
    assert_eq!(oneof.full_name(), "my.package.MyMessage.my_oneof");
    assert_eq!(oneof.path(), &[4, 0, 8, 0]);

    let member = fld(&message, "my_field");
    assert_eq!(
        member.containing_oneof().map(|o| o.full_name().to_string()),
        Some("my.package.MyMessage.my_oneof".to_string())
    );
    assert_eq!(member.kind(), Kind::Int32);
    let plain = fld(&msg(&pool, "test.Scalars"), "int32");
    assert!(plain.containing_oneof().is_none());
}

// ---------------------------------------------------------------------------
// Field predicate witness table
// ---------------------------------------------------------------------------

#[test]
fn field_predicate_contract() {
    let pool = pool();
    let complex = msg(&pool, "test.ComplexType");
    let scalars = msg(&pool, "test.Scalars");
    let groups = msg(&pool, "test2.ContainsGroup");
    let unpacked = msg(&pool, "test2.UnpackedScalarArray");

    // (field, is_list, is_map, is_packed, is_group, is_required, supports_presence)
    let rows: Vec<(FieldDescriptor, bool, bool, bool, bool, bool, bool)> = vec![
        (
            fld(&complex, "my_enum"),
            true,
            false,
            true,
            false,
            false,
            false,
        ),
        (
            fld(&complex, "optional_enum"),
            false,
            false,
            false,
            false,
            false,
            false,
        ),
        (
            fld(&complex, "enum_map"),
            false,
            true,
            false,
            false,
            false,
            false,
        ),
        (
            fld(&complex, "nested"),
            false,
            false,
            false,
            false,
            false,
            true,
        ),
        (
            fld(&complex, "string_map"),
            false,
            true,
            false,
            false,
            false,
            false,
        ),
        (
            fld(&complex, "int_map"),
            false,
            true,
            false,
            false,
            false,
            false,
        ),
        (
            fld(&unpacked, "unpacked_double"),
            true,
            false,
            false,
            false,
            false,
            false,
        ),
        (
            fld(&scalars, "int32"),
            false,
            false,
            false,
            false,
            false,
            false,
        ),
    ];
    for (f, is_list, is_map, is_packed, is_group, is_required, presence) in rows {
        assert_eq!(f.is_list(), is_list, "is_list {}", f.full_name());
        assert_eq!(f.is_map(), is_map, "is_map {}", f.full_name());
        assert_eq!(f.is_packed(), is_packed, "is_packed {}", f.full_name());
        assert_eq!(f.is_group(), is_group, "is_group {}", f.full_name());
        assert_eq!(
            f.is_required(),
            is_required,
            "is_required {}",
            f.full_name()
        );
        assert_eq!(
            f.supports_presence(),
            presence,
            "supports_presence {}",
            f.full_name()
        );
    }

    // group fields: descriptor stores their names lowercased, so resolve
    // by iteration (ascending number order) rather than by name
    let group_fields: Vec<_> = groups.fields().collect();
    assert_eq!(group_fields.len(), 3, "ContainsGroup has exactly 3 groups");
    assert!(group_fields.iter().all(|f| f.is_group()));
    assert!(!group_fields[0].is_list() && !group_fields[0].is_map());
    assert!(group_fields[0].supports_presence(), "optional group");
    assert!(group_fields[1].supports_presence(), "optional group");
    assert!(group_fields[2].is_list(), "repeated group");
    assert!(
        !group_fields[2].supports_presence(),
        "repeated has no presence"
    );

    // fields() iterates in ascending field-number order (the public
    // ordering contract C1's merge-walk relies on) and agrees with
    // get_field
    let all_numbers: Vec<u32> = complex.fields().map(|f| f.number()).collect();
    let mut sorted = all_numbers.clone();
    sorted.sort_unstable();
    assert_eq!(all_numbers, sorted, "fields() must be number-ascending");
    assert!(!all_numbers.is_empty());
    for number in &all_numbers {
        assert_eq!(complex.get_field(*number).expect("field").number(), *number);
    }

    // kind witnesses
    let enum_field = fld(&complex, "my_enum");
    match enum_field.kind() {
        Kind::Enum(desc) => assert_eq!(desc.full_name(), "test.ComplexType.MyEnum"),
        other => panic!("expected enum kind, got {other:?}"),
    }
    let group = group_fields[0].clone();
    match group.kind() {
        Kind::Message(desc) => {
            let inner = desc.get_field_by_name("a").expect("group field a");
            assert!(inner.is_required());
        }
        other => panic!("group should be message kind, got {other:?}"),
    }

    // enum descriptor values
    let values: Vec<_> = match enum_field.kind() {
        Kind::Enum(desc) => desc.values().collect(),
        _ => unreachable!(),
    };
    // values iterate in ascending number order (NEG = -4 sorts first)
    assert_eq!(values[0].name(), "NEG");
    assert_eq!(values[0].number(), -4);
    assert!(values
        .iter()
        .any(|v| v.name() == "DEFAULT" && v.number() == 0));
}

// ---------------------------------------------------------------------------
// Extension descriptors
// ---------------------------------------------------------------------------

#[test]
fn extension_descriptor_contract() {
    let pool = pool();
    let file_ext = pool
        .get_extension_by_name("custom.options.file")
        .expect("custom.options.file");
    assert_eq!(file_ext.number(), 1001);
    assert_eq!(file_ext.kind(), Kind::Int32);
    assert!(!file_ext.is_list());
    assert!(!file_ext.is_map());
    assert!(!file_ext.is_group());
    assert_eq!(file_ext.full_name(), "custom.options.file");
    assert_eq!(file_ext.package_name(), "custom.options");
    assert_eq!(
        file_ext
            .parent_pool()
            .get_extension_by_name("custom.options.file")
            .expect("same pool resolves it")
            .full_name(),
        "custom.options.file"
    );
    assert_eq!(
        file_ext.containing_message().full_name(),
        "google.protobuf.FileOptions"
    );

    let field_ext = pool
        .get_extension_by_name("custom.options.field")
        .expect("custom.options.field");
    assert_eq!(field_ext.kind(), Kind::Bytes);

    let oneof_ext = pool
        .get_extension_by_name("custom.options.oneof")
        .expect("custom.options.oneof");
    assert!(oneof_ext.is_list(), "repeated float option");
    assert!(!oneof_ext.is_map());

    let aggregate_ext = pool
        .get_extension_by_name("custom.options.enum")
        .expect("custom.options.enum");
    match aggregate_ext.kind() {
        Kind::Message(_) => {}
        other => panic!("Aggregate option should be message kind, got {other:?}"),
    }

    let value_ext = pool
        .get_extension_by_name("custom.options.value")
        .expect("custom.options.value");
    match value_ext.kind() {
        Kind::Enum(_) => {}
        other => panic!("value option should be enum kind, got {other:?}"),
    }

    let len_ext = pool.get_extension_by_name("demo.len").expect("demo.len");
    assert_eq!(len_ext.kind(), Kind::Uint32);
    assert_eq!(
        len_ext.containing_message().full_name(),
        "google.protobuf.EnumValueOptions"
    );

    // message-scoped lookup must agree with pool-scoped lookup
    let file_options = msg(&pool, "google.protobuf.FileOptions");
    let by_number = file_options.get_extension(1001).expect("ext by number");
    assert_eq!(by_number.full_name(), file_ext.full_name());
    let by_full = file_options
        .get_extension_by_full_name("custom.options.file")
        .expect("ext by full name");
    assert_eq!(by_full.full_name(), file_ext.full_name());
    assert_eq!(
        file_options
            .get_extension_by_json_name(by_number.json_name())
            .expect("ext by json name")
            .full_name(),
        file_ext.full_name()
    );
}

// ---------------------------------------------------------------------------
// Extension values: operations and wire bytes
// ---------------------------------------------------------------------------

/// Minimal protobuf varint encoder for hand-built golden extension bytes.
fn varint(mut n: u64, out: &mut Vec<u8>) {
    loop {
        let mut b = (n & 0x7f) as u8;
        n >>= 7;
        if n != 0 {
            b |= 0x80;
        }
        out.push(b);
        if n == 0 {
            break;
        }
    }
}

#[test]
fn extension_wire_bytes_match_handcrafted_golden() {
    let pool = pool();
    let file_options = msg(&pool, "google.protobuf.FileOptions");
    let ext = pool
        .get_extension_by_name("custom.options.file")
        .expect("custom.options.file");

    let mut message = DynamicMessage::new(file_options.clone());
    message.set_extension(&ext, Value::I32(42));

    // field 1001, wire type 0 (varint): tag = 1001 << 3 | 0 = 8008
    let mut expected = Vec::new();
    varint(8008, &mut expected);
    varint(42, &mut expected);
    assert_eq!(message.encode_to_vec(), expected, "scalar ext wire bytes");

    // list extension: repeated float on OneofOptions, field 1001, wire type 2
    let list_ext = pool
        .get_extension_by_name("custom.options.oneof")
        .expect("custom.options.oneof");
    assert_eq!(
        list_ext.containing_message().full_name(),
        "google.protobuf.OneofOptions"
    );
    let oneof_options = msg(&pool, "google.protobuf.OneofOptions");
    let mut m2 = DynamicMessage::new(oneof_options);
    m2.set_extension(
        &list_ext,
        Value::List(vec![Value::F32(1.0), Value::F32(2.0)]),
    );
    let mut expected = Vec::new();
    varint(8008 | 2, &mut expected); // 1001 << 3 | 2
    varint(8, &mut expected); // 2 floats * 4 bytes
    expected.extend_from_slice(&1.0f32.to_le_bytes());
    expected.extend_from_slice(&2.0f32.to_le_bytes());
    assert_eq!(m2.encode_to_vec(), expected, "list ext wire bytes");
}

#[test]
fn dynamic_message_extension_operations() {
    let pool = pool();
    let file_options = msg(&pool, "google.protobuf.FileOptions");
    let ext = pool
        .get_extension_by_name("custom.options.file")
        .expect("custom.options.file");

    let mut message = DynamicMessage::new(file_options);
    assert!(!message.has_extension(&ext));
    assert_eq!(
        message.get_extension(&ext).as_ref(),
        &Value::I32(0),
        "unset default"
    );

    message.set_extension(&ext, Value::I32(42));
    assert!(message.has_extension(&ext));
    assert_eq!(message.get_extension(&ext).as_ref(), &Value::I32(42));
    *message.get_extension_mut(&ext) = Value::I32(7);
    assert_eq!(message.take_extension(&ext), Some(Value::I32(7)));
    assert!(!message.has_extension(&ext));

    message.set_extension(&ext, Value::I32(1));
    message.clear_extension(&ext);
    assert!(!message.has_extension(&ext));
    assert_eq!(message.extensions().count(), 0);

    // a second extension on its proper container message, list-typed
    let oneof_options = msg(&pool, "google.protobuf.OneofOptions");
    let list_ext = pool
        .get_extension_by_name("custom.options.oneof")
        .expect("custom.options.oneof");
    let mut m2 = DynamicMessage::new(oneof_options);
    m2.set_extension(
        &list_ext,
        Value::List(vec![Value::F32(1.0), Value::F32(2.0)]),
    );
    assert!(m2.has_extension(&list_ext));
    assert_eq!(m2.extensions().count(), 1);
    let taken: Vec<_> = m2.take_extensions().collect();
    assert_eq!(taken.len(), 1);
    assert_eq!(taken[0].0.full_name(), "custom.options.oneof");
}

// ---------------------------------------------------------------------------
// DynamicMessage field accessors by name and by number
// ---------------------------------------------------------------------------

#[test]
fn dynamic_message_field_surface() {
    let pool = pool();
    let scalars = msg(&pool, "test.Scalars");
    let mut message = DynamicMessage::new(scalars.clone());

    assert!(!message.has_field_by_name("int32"));
    assert!(!message.has_field_by_number(3));

    // no-presence scalar set to zero: stored but not "present"
    message.set_field_by_name("int32", Value::I32(0));
    assert_eq!(
        message.get_field_by_name("int32").as_deref(),
        Some(&Value::I32(0))
    );
    assert!(!message.has_field_by_name("int32"));
    assert_eq!(message.take_field_by_name("int32"), None);

    message.set_field_by_number(3, Value::I32(5));
    assert!(message.has_field_by_number(3));
    assert_eq!(message.take_field_by_number(3), Some(Value::I32(5)));
    assert!(!message.has_field_by_number(3));

    // mut accessor writes through
    message.set_field_by_name("int32", Value::I32(0));
    *message.get_field_by_number_mut(3).expect("field 3 mut") = Value::I32(9);
    assert_eq!(
        message.get_field_by_name("int32").as_deref(),
        Some(&Value::I32(9))
    );
    message.set_field_by_name("int32", Value::I32(0));
    *message.get_field_by_name_mut("int32").expect("name mut") = Value::I32(4);
    assert_eq!(
        message.get_field_by_number(3).as_deref(),
        Some(&Value::I32(4))
    );

    // clear variants
    message.set_field_by_number(3, Value::I32(3));
    message.clear_field_by_name("int32");
    assert_eq!(
        message.get_field_by_name("int32").as_deref(),
        Some(&Value::I32(0))
    );
    message.set_field_by_number(3, Value::I32(3));
    message.clear_field_by_number(3);
    assert_eq!(
        message.get_field_by_name("int32").as_deref(),
        Some(&Value::I32(0))
    );
    message.set_field_by_number(3, Value::I32(3));
    message.clear_field(&fld(&scalars, "int32"));
    assert_eq!(
        message.get_field_by_name("int32").as_deref(),
        Some(&Value::I32(0))
    );

    // take_field by descriptor
    message.set_field_by_number(3, Value::I32(3));
    assert_eq!(
        message.take_field(&fld(&scalars, "int32")),
        Some(Value::I32(3))
    );

    // an invalid value type must be rejected by the panicking setter
    // (pins Value::is_valid_for_field and its kind guards)
    let panicked = std::panic::catch_unwind(|| {
        let mut m = DynamicMessage::new(scalars.clone());
        m.set_field_by_name("int32", Value::String("nope".into()));
    });
    assert!(panicked.is_err(), "wrong-typed set must panic");

    // invalid map-value type must panic too (pins the map-kind validity
    // guards in is_valid_for_field)
    let complex = msg(&test_file_descriptor(), "test.ComplexType");
    let panicked = std::panic::catch_unwind(|| {
        let mut m = DynamicMessage::new(complex);
        m.set_field_by_name(
            "string_map",
            Value::Map(Default::default()), // map-typed value is fine
        );
    });
    assert!(panicked.is_ok(), "map-typed value is valid");
    let panicked = std::panic::catch_unwind(|| {
        let mut m = DynamicMessage::new(msg(&test_file_descriptor(), "test.ComplexType"));
        // wrong container: a list where the map belongs
        m.set_field_by_name("string_map", Value::List(vec![]));
    });
    assert!(panicked.is_err(), "wrong container type must panic");

    // invalid extension-value type must panic (is_valid_for_extension)
    let p = test_file_descriptor();
    let file_options = msg(&p, "google.protobuf.FileOptions");
    let ext = p
        .get_extension_by_name("custom.options.file")
        .expect("custom.options.file");
    let panicked = std::panic::catch_unwind(|| {
        let mut m = DynamicMessage::new(file_options);
        m.set_extension(&ext, Value::String("nope".into()));
    });
    assert!(panicked.is_err(), "wrong-typed extension set must panic");

    // presence-capable field: proto3 optional enum
    let opt = msg(&pool, "test.MessageWithOptionalEnum");
    let mut m2 = DynamicMessage::new(opt.clone());
    assert!(!m2.has_field_by_name("optional_enum"));
    m2.set_field_by_name("optional_enum", Value::EnumNumber(0));
    assert!(m2.has_field_by_name("optional_enum"), "explicit presence");
    m2.clear_field_by_name("optional_enum");
    assert!(!m2.has_field_by_name("optional_enum"));
}

// ---------------------------------------------------------------------------
// Value accessor matrix
// ---------------------------------------------------------------------------

#[test]
fn value_as_matrix() {
    let v = Value::I32(-7);
    assert_eq!(v.as_i32(), Some(-7));
    assert_eq!(v.as_i64(), None, "strict: I32 is not I64");
    assert_eq!(v.as_u32(), None);
    assert_eq!(v.as_bool(), None);
    assert_eq!(v.as_f64(), None);
    assert_eq!(v.as_str(), None);

    let v = Value::U64(7);
    assert_eq!(v.as_u64(), Some(7));
    assert_eq!(v.as_i64(), None, "strict: U64 is not I64");
    assert_eq!(v.as_u32(), None);

    let v = Value::Bool(true);
    assert_eq!(v.as_bool(), Some(true));
    assert_eq!(v.as_u64(), None);

    let v = Value::F64(2.5);
    assert_eq!(v.as_f64(), Some(2.5));
    assert_eq!(v.as_f32(), None, "strict: F64 is not F32");
    assert_eq!(v.as_i64(), None);

    let v = Value::String("s".to_string());
    assert_eq!(v.as_str(), Some("s"));
    assert_eq!(v.as_bytes(), None);

    let v = Value::Bytes(prost::bytes::Bytes::from_static(b"b"));
    assert_eq!(v.as_bytes(), Some(&prost::bytes::Bytes::from_static(b"b")));
    assert_eq!(v.as_str(), None);

    let v = Value::List(vec![Value::I32(1)]);
    assert_eq!(v.as_list(), Some(&[Value::I32(1)][..]));
    assert_eq!(v.as_bool(), None);
    assert!(v.as_map().is_none());

    let v = Value::I32(0);
    assert_eq!(v.as_enum_number(), None, "plain int is not an enum value");
    let v = Value::EnumNumber(5);
    assert_eq!(v.as_enum_number(), Some(5));
    assert_eq!(v.as_i32(), None, "EnumNumber is not I32");
}

#[test]
fn value_mut_accessors_write_through() {
    // every mut accessor on its own variant, read back through the
    // immutable accessor, plus cross-variant None probes
    let mut v = Value::I32(0);
    *v.as_i32_mut().expect("i32 mut") = 3;
    assert_eq!(v.as_i32(), Some(3));
    assert!(v.as_bool_mut().is_none());

    let mut v = Value::U32(1);
    *v.as_u32_mut().expect("u32 mut") = 9;
    assert_eq!(v.as_u32(), Some(9));
    assert!(v.as_i64_mut().is_none());

    let mut v = Value::U64(1);
    *v.as_u64_mut().expect("u64 mut") = 9;
    assert_eq!(v.as_u64(), Some(9));

    let mut v = Value::I64(1);
    *v.as_i64_mut().expect("i64 mut") = -9;
    assert_eq!(v.as_i64(), Some(-9));

    let mut v = Value::F32(1.0);
    *v.as_f32_mut().expect("f32 mut") = 2.0;
    assert_eq!(v.as_f32(), Some(2.0));
    assert!(v.as_f64_mut().is_none());

    let mut v = Value::F64(1.0);
    *v.as_f64_mut().expect("f64 mut") = 2.0;
    assert_eq!(v.as_f64(), Some(2.0));

    let mut v = Value::EnumNumber(0);
    *v.as_enum_number_mut().expect("enum mut") = 7;
    assert_eq!(v.as_enum_number(), Some(7));
    assert!(v.as_i32_mut().is_none());

    let mut v = Value::String("a".into());
    v.as_string_mut().expect("string mut").push('b');
    assert_eq!(v.as_str(), Some("ab"));

    let mut v = Value::Bytes(prost::bytes::Bytes::from_static(b"a"));
    *v.as_bytes_mut().expect("bytes mut") = prost::bytes::Bytes::from_static(b"ab");
    assert_eq!(v.as_bytes(), Some(&prost::bytes::Bytes::from_static(b"ab")));

    let scalars_desc = msg(&pool(), "test.Scalars");
    let mut v = Value::Message(DynamicMessage::new(scalars_desc));
    v.as_message_mut()
        .expect("message mut")
        .set_field_by_name("int32", Value::I32(5));
    use std::borrow::Cow;
    assert_eq!(
        v.as_message().expect("message").get_field_by_name("int32"),
        Some(Cow::Owned(Value::I32(5)))
    );
    assert!(v.as_bool_mut().is_none());

    let mut v = Value::List(vec![]);
    v.as_list_mut().expect("list mut").push(Value::I32(1));
    assert_eq!(v.as_list(), Some(&[Value::I32(1)][..]));

    let mut v = Value::Bool(false);
    *v.as_bool_mut().expect("bool mut") = true;
    assert_eq!(v.as_bool(), Some(true));

    let mut v = Value::Map(Default::default());
    v.as_map_mut()
        .expect("map mut")
        .insert(MapKey::String("k".into()), Value::I32(1));
    assert_eq!(v.as_map().expect("map").len(), 1);
    assert!(v.as_list_mut().is_none());
}

#[test]
fn value_is_default_contract() {
    // is_default is kind-aware: the zero of the field's kind is default
    assert!(Value::I32(0).is_default(&Kind::Int32));
    assert!(!Value::I32(1).is_default(&Kind::Int32));
    assert!(Value::EnumNumber(0).is_default(&Kind::Enum(enum_desc_probe())));
    assert!(Value::String(String::new()).is_default(&Kind::String));
    assert!(!Value::String("x".into()).is_default(&Kind::String));
    let scalars_desc = msg(&pool(), "test.Scalars");
    let message_default = Kind::Message(scalars_desc.clone()).default_value();
    assert!(message_default.is_default(&Kind::Message(scalars_desc.clone())));
    let mut non_default = DynamicMessage::new(scalars_desc.clone());
    non_default.set_field_by_name("int32", Value::I32(5));
    assert!(!Value::Message(non_default).is_default(&Kind::Message(scalars_desc.clone())));
    assert!(!Value::List(vec![]).is_default(&Kind::Message(scalars_desc)));
    assert!(Value::Bool(false).is_default(&Kind::Bool));
    assert!(!Value::Bool(true).is_default(&Kind::Bool));
    assert!(Value::U64(0).is_default(&Kind::Uint64));
    assert!(Value::EnumNumber(0).is_default(&Kind::Enum(enum_desc_probe())));
    assert!(!Value::EnumNumber(3).is_default(&Kind::Enum(enum_desc_probe())));
}

/// A real enum descriptor for kind-aware default checks.
fn enum_desc_probe() -> prost_reflect::EnumDescriptor {
    let f = fld(&msg(&pool(), "test.ComplexType"), "my_enum");
    match f.kind() {
        Kind::Enum(d) => d.clone(),
        other => panic!("expected enum kind, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// MapKey accessor matrix
// ---------------------------------------------------------------------------

#[test]
fn mapkey_accessor_matrix() {
    let key = MapKey::I32(-3);
    assert_eq!(key.as_i32(), Some(-3));
    assert_eq!(key.as_i64(), None, "strict: I32 key is not I64");
    assert_eq!(key.as_u32(), None);
    assert_eq!(key.as_str(), None);
    let mut k = key;
    *k.as_i32_mut().expect("mut") = 4;
    assert_eq!(k.as_i32(), Some(4));

    let key = MapKey::String("s".into());
    assert_eq!(key.as_str(), Some("s"));
    assert_eq!(key.as_i64(), None);
    let mut k = key;
    k.as_string_mut().expect("mut").push('t');
    assert_eq!(k.as_str(), Some("st"));

    let key = MapKey::Bool(true);
    assert_eq!(key.as_bool(), Some(true));
    assert_eq!(key.as_u32(), None);
    let mut k = key;
    *k.as_bool_mut().expect("mut") = false;
    assert_eq!(k.as_bool(), Some(false));
    assert!(k.as_string_mut().is_none());

    let key = MapKey::U32(7);
    assert_eq!(key.as_u32(), Some(7));
    assert_eq!(key.as_i64(), None);
    let mut k = key;
    *k.as_u32_mut().expect("mut") = 8;
    assert_eq!(k.as_u32(), Some(8));
    assert!(k.as_i64_mut().is_none());

    let key = MapKey::U64(7);
    assert_eq!(key.as_u64(), Some(7));
    assert_eq!(key.as_i64(), None, "strict: U64 key is not I64");
    let mut k = key;
    *k.as_u64_mut().expect("mut") = 9;
    assert_eq!(k.as_u64(), Some(9));
    assert!(k.as_i32_mut().is_none());

    let key = MapKey::I64(-7);
    assert_eq!(key.as_i64(), Some(-7));
    assert_eq!(key.as_u64(), None);
    let mut k = key;
    *k.as_i64_mut().expect("mut") = 8;
    assert_eq!(k.as_i64(), Some(8));
}

// ---------------------------------------------------------------------------
// Unknown-field surface
// ---------------------------------------------------------------------------

#[test]
fn unknown_field_surface() {
    let pool = pool();
    let scalars = msg(&pool, "test.Scalars");
    // payload: known field 3 (int32, varint 7) plus unknown fields
    // 200 (varint 99), 201 (fixed64), 202 (length-delimited "abc")
    let mut payload = Vec::new();
    varint(3 << 3 | 0, &mut payload);
    varint(7, &mut payload);
    varint(200 << 3 | 0, &mut payload);
    varint(99, &mut payload);
    varint(201 << 3 | 1, &mut payload);
    payload.extend_from_slice(&42u64.to_le_bytes());
    varint(202 << 3 | 2, &mut payload);
    varint(3, &mut payload);
    payload.extend_from_slice(b"abc");

    let mut message = DynamicMessage::decode(scalars, payload.as_slice()).expect("decode");
    assert_eq!(
        message.get_field_by_name("int32").as_deref(),
        Some(&Value::I32(7))
    );
    let unknowns: Vec<_> = message.unknown_fields().collect();
    assert_eq!(unknowns.len(), 3, "three unknown fields");
    let numbers: Vec<u32> = unknowns.iter().map(|u| u.number()).collect();
    assert_eq!(numbers, vec![200, 201, 202], "ascending number order");
    // 200<<3 needs a 2-byte tag: varint = 2+1 bytes
    assert_eq!(unknowns[0].encoded_len(), 3, "varint 99");
    assert_eq!(unknowns[1].encoded_len(), 10, "fixed64: 2-byte tag + 8");
    assert_eq!(unknowns[2].encoded_len(), 6, "len-delimited: 2+1+3");

    // byte-identical roundtrip incl. unknown fields
    assert_eq!(message.encode_to_vec(), payload, "unknowns preserved");

    // take clears them
    assert_eq!(message.take_unknown_fields().count(), 3);
    assert_eq!(message.unknown_fields().count(), 0);
}

// ---------------------------------------------------------------------------
// Pool and file serialization roundtrips
// ---------------------------------------------------------------------------

#[test]
fn pool_serialization_roundtrip() {
    let pool = pool();
    let bytes = pool.encode_to_vec();
    assert!(!bytes.is_empty());

    let fds = prost_types::FileDescriptorSet::decode(bytes.as_slice()).expect("decode fds");
    let decoded = DescriptorPool::from_file_descriptor_set(fds).expect("decode set");
    assert_eq!(decoded.files().count(), pool.files().count());
    assert!(decoded.get_message_by_name("test.ComplexType").is_some());
    assert!(decoded
        .get_message_by_name("my.package.MyMessage")
        .is_some());

    let mut target = DescriptorPool::new();
    let file = pool
        .files()
        .find(|f| f.name().ends_with("desc.proto"))
        .expect("desc.proto");
    let file_bytes = file.encode_to_vec();
    target
        .decode_file_descriptor_proto(file_bytes.as_slice())
        .expect("decode_file_descriptor_proto");
    assert!(target.get_message_by_name("my.package.MyMessage").is_some());

    let mut target2 = DescriptorPool::new();
    target2
        .decode_file_descriptor_set(pool.encode_to_vec().as_slice())
        .expect("decode_file_descriptor_set");
    assert!(target2.get_message_by_name("test.Scalars").is_some());
}

// ---------------------------------------------------------------------------
// Debug/Display smoke (Debug bodies were an untested family)
// ---------------------------------------------------------------------------

#[test]
fn debug_fmt_is_informative() {
    let pool = pool();
    let message = msg(&pool, "my.package.MyMessage");
    let file = pool
        .files()
        .find(|f| f.name().ends_with("desc.proto"))
        .expect("desc.proto");
    let service = file.services().next().expect("service");
    let method = service.methods().next().expect("method");
    let oneof = message.oneofs().next().expect("oneof");
    let field = fld(&message, "my_field");
    let enum_field = fld(&msg(&pool, "test.ComplexType"), "my_enum");
    let enum_desc = match enum_field.kind() {
        Kind::Enum(desc) => desc.clone(),
        other => panic!("enum kind, got {other:?}"),
    };
    let enum_value = enum_desc.get_value_by_name("NEG").expect("enum value NEG");

    let rendered = [
        (format!("{file:?}"), "desc.proto"),
        (format!("{message:?}"), "MyMessage"),
        (format!("{service:?}"), "MyService"),
        (format!("{method:?}"), "MyMethod"),
        (format!("{oneof:?}"), "my_oneof"),
        (format!("{field:?}"), "my_field"),
        (format!("{enum_desc:?}"), "MyEnum"),
        (format!("{enum_value:?}"), "NEG"),
    ];
    for (text, expected) in rendered {
        assert!(!text.is_empty());
        assert!(
            text.contains(expected),
            "Debug for {expected} uninformative: {text}"
        );
    }

    let mut dynamic = DynamicMessage::new(message);
    dynamic.set_field_by_name("my_field", Value::I32(5));
    let rendered = format!("{dynamic}");
    assert!(!rendered.is_empty(), "Display is empty");
    assert!(
        rendered.contains("5"),
        "Display shows the value: {rendered}"
    );
}
