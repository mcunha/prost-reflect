//! Proxy-flavoured benchmarks for the GFE-style use case: runtime descriptor
//! loading, per-request dynamic decode/encode, route-field reads, unknown-field
//! passthrough, JSON translation, and multithreaded encode (Arc refcount
//! contention, upstream issue #7).

use std::{
    collections::HashMap,
    hint::black_box,
    sync::{Barrier, Once},
    time::Instant,
};

use criterion::{criterion_group, criterion_main, Criterion};
use prost::Message;
use prost_reflect::{
    DescriptorPool, DeserializeOptions, DynamicMessage, ReflectMessage, SerializeOptions, Value,
};
use prost_reflect_tests::proto::{ComplexType, Scalars};
use prost_reflect_tests::test_file_descriptor;

const DESCRIPTOR_SET_BYTES: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/file_descriptor_set.bin"));

fn scalars_sample() -> Scalars {
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

fn complex_sample() -> ComplexType {
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

/// Scalars payload with trailing fields unknown to the descriptor
/// (100-104): a proxy decoding a message from a newer backend version.
fn scalars_with_unknown_bytes() -> Vec<u8> {
    let mut bytes = scalars_sample().encode_to_vec();
    // field 100 varint = 42
    bytes.extend_from_slice(&[0xA0, 0x06, 0x2A]);
    // field 101 fixed64
    bytes.extend_from_slice(&[0xA9, 0x06, 1, 2, 3, 4, 5, 6, 7, 8]);
    // field 102 length-delimited "hello"
    bytes.extend_from_slice(&[0xB2, 0x06, 0x05, b'h', b'e', b'l', b'l', b'o']);
    // field 103 fixed32
    bytes.extend_from_slice(&[0xBD, 0x06, 1, 2, 3, 4]);
    // group 104 { 1: 7 } ... end (key 104<<3|4 = 836)
    bytes.extend_from_slice(&[0xC3, 0x06, 0x08, 0x07, 0xC4, 0x06]);
    bytes
}

/// Five varint unknowns (fields 100-104), each 42.
fn scalars_with_varint_unknowns() -> Vec<u8> {
    let mut bytes = scalars_sample().encode_to_vec();
    for key in [
        [0xA0u8, 0x06],
        [0xA8, 0x06],
        [0xB0, 0x06],
        [0xB8, 0x06],
        [0xC0, 0x06],
    ] {
        bytes.extend_from_slice(&key);
        bytes.push(0x2A);
    }
    bytes
}

/// Five length-delimited unknowns (fields 110-114), each "Hi".
fn scalars_with_len_unknowns() -> Vec<u8> {
    let mut bytes = scalars_sample().encode_to_vec();
    for key in [
        [0xF2u8, 0x06],
        [0xFA, 0x06],
        [0x82, 0x07],
        [0x8A, 0x07],
        [0x92, 0x07],
    ] {
        bytes.extend_from_slice(&key);
        bytes.extend_from_slice(&[0x02, b'H', b'i']);
    }
    bytes
}

/// Five fixed64 unknowns (fields 120-124).
fn scalars_with_fixed64_unknowns() -> Vec<u8> {
    let mut bytes = scalars_sample().encode_to_vec();
    for key in [
        [0xC1u8, 0x07],
        [0xC9, 0x07],
        [0xD1, 0x07],
        [0xD9, 0x07],
        [0xE1, 0x07],
    ] {
        bytes.extend_from_slice(&key);
        bytes.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    }
    bytes
}

/// Two group unknowns (fields 130, 131), each { 1: 7 }.
fn scalars_with_group_unknowns() -> Vec<u8> {
    let mut bytes = scalars_sample().encode_to_vec();
    bytes.extend_from_slice(&[0x93, 0x08, 0x08, 0x07, 0x94, 0x08]);
    bytes.extend_from_slice(&[0x9B, 0x08, 0x08, 0x07, 0x9C, 0x08]);
    bytes
}

fn pool_summary() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let pool = DescriptorPool::decode(DESCRIPTOR_SET_BYTES)
            .expect("descriptor set must be standalone-decodable");
        let t0 = Instant::now();
        for _ in 0..50 {
            black_box(DescriptorPool::decode(DESCRIPTOR_SET_BYTES).unwrap());
        }
        let per = t0.elapsed() / 50;
        eprintln!(
            "publish payload: {} files, {} messages, {} bytes; standalone pool decode {:.3} ms",
            pool.files().count(),
            pool.all_messages().count(),
            DESCRIPTOR_SET_BYTES.len(),
            per.as_secs_f64() * 1e3,
        );
    });
}

fn wkt_global_pool_init(_c: &mut Criterion) {
    // Measures at registration time (first call wins); intentionally
    // registers no criterion benchmark, so the parameter stays unused.
    // One-time startup cost: global well-known-types pool. This must be the
    // first bench to run (it is), so nothing has touched the global pool yet.
    let t0 = Instant::now();
    let pool = DescriptorPool::global();
    let ms = t0.elapsed().as_secs_f64() * 1e3;
    eprintln!(
        "wkt global pool init: {ms:.3} ms ({} files)",
        pool.files().count()
    );
}

fn pool_decode(c: &mut Criterion) {
    pool_summary();
    c.bench_function("pool_decode_publish", |b| {
        b.iter(|| DescriptorPool::decode(black_box(DESCRIPTOR_SET_BYTES)).unwrap())
    });
}

fn lookup_by_name(c: &mut Criterion) {
    let pool = test_file_descriptor();
    c.bench_function("lookup_message_by_name", |b| {
        b.iter(|| {
            pool.get_message_by_name(black_box("test.ComplexType"))
                .unwrap()
        })
    });
    c.bench_function("lookup_field_by_name", |b| {
        let desc = pool.get_message_by_name("test.ComplexType").unwrap();
        b.iter(|| desc.get_field_by_name(black_box("nested")).unwrap())
    });
}

fn decode_encode_scalars(c: &mut Criterion) {
    let bytes = scalars_sample().encode_to_vec();
    let desc = test_file_descriptor()
        .get_message_by_name("test.Scalars")
        .unwrap();

    c.bench_function("decode_scalars_dynamic", |b| {
        b.iter(|| DynamicMessage::decode(desc.clone(), black_box(bytes.as_slice())).unwrap())
    });
    c.bench_function("decode_scalars_static", |b| {
        b.iter(|| Scalars::decode(black_box(bytes.as_slice())).unwrap())
    });

    let dynamic = DynamicMessage::decode(desc, bytes.as_slice()).unwrap();
    c.bench_function("encode_scalars_dynamic", |b| {
        b.iter(|| black_box(dynamic.encode_to_vec()))
    });
    let static_msg = scalars_sample();
    c.bench_function("encode_scalars_static", |b| {
        b.iter(|| black_box(static_msg.encode_to_vec()))
    });
}

fn decode_encode_complex(c: &mut Criterion) {
    let bytes = complex_sample().encode_to_vec();
    let desc = test_file_descriptor()
        .get_message_by_name("test.ComplexType")
        .unwrap();

    c.bench_function("decode_complex_dynamic", |b| {
        b.iter(|| DynamicMessage::decode(desc.clone(), black_box(bytes.as_slice())).unwrap())
    });
    c.bench_function("decode_complex_static", |b| {
        b.iter(|| ComplexType::decode(black_box(bytes.as_slice())).unwrap())
    });

    let dynamic = DynamicMessage::decode(desc, bytes.as_slice()).unwrap();
    c.bench_function("encode_complex_dynamic", |b| {
        b.iter(|| black_box(dynamic.encode_to_vec()))
    });
    let static_msg = complex_sample();
    c.bench_function("encode_complex_static", |b| {
        b.iter(|| black_box(static_msg.encode_to_vec()))
    });
}

fn decode_encode_sparse(c: &mut Criterion) {
    // Fixed per-call overhead vs per-field marginal cost.
    let mut one_field = scalars_sample();
    one_field.string = String::new();
    one_field.bytes = Vec::new();
    one_field.double = 0.0;
    one_field.float = 0.0;
    one_field.int64 = 0;
    one_field.uint32 = 0;
    one_field.uint64 = 0;
    one_field.sint32 = 0;
    one_field.sint64 = 0;
    one_field.fixed32 = 0;
    one_field.fixed64 = 0;
    one_field.sfixed32 = 0;
    one_field.sfixed64 = 0;
    one_field.bool = false;
    one_field.int32 = 7;
    // only int32 != 0 remains set
    let full_bytes = scalars_sample().encode_to_vec();
    let one_bytes = one_field.encode_to_vec();
    let desc = test_file_descriptor()
        .get_message_by_name("test.Scalars")
        .unwrap();

    c.bench_function("decode_scalars_dynamic_1field", |b| {
        b.iter(|| DynamicMessage::decode(desc.clone(), black_box(one_bytes.as_slice())).unwrap())
    });
    c.bench_function("decode_scalars_dynamic_15field", |b| {
        b.iter(|| DynamicMessage::decode(desc.clone(), black_box(full_bytes.as_slice())).unwrap())
    });
    c.bench_function("decode_scalars_dynamic_empty_bytes", |b| {
        b.iter(|| DynamicMessage::decode(desc.clone(), black_box(&[][..])).unwrap())
    });

    let one_dyn = DynamicMessage::decode(desc.clone(), one_bytes.as_slice()).unwrap();
    let full_dyn = DynamicMessage::decode(desc.clone(), full_bytes.as_slice()).unwrap();
    let empty_dyn = DynamicMessage::decode(desc.clone(), &[][..]).unwrap();
    c.bench_function("encode_scalars_dynamic_1field", |b| {
        b.iter(|| black_box(one_dyn.encode_to_vec()))
    });
    c.bench_function("encode_scalars_dynamic_15field", |b| {
        b.iter(|| black_box(full_dyn.encode_to_vec()))
    });
    c.bench_function("encode_scalars_dynamic_0field", |b| {
        b.iter(|| black_box(empty_dyn.encode_to_vec()))
    });
}

fn decode_unknown_fields(c: &mut Criterion) {
    let bytes = scalars_with_unknown_bytes();
    let desc = test_file_descriptor()
        .get_message_by_name("test.Scalars")
        .unwrap();

    // Byte-exact passthrough check (decode then re-encode).
    let dynamic = DynamicMessage::decode(desc.clone(), bytes.as_slice()).unwrap();
    assert_eq!(dynamic.encode_to_vec(), bytes);

    c.bench_function("decode_scalars_with_unknown_fields", |b| {
        b.iter(|| DynamicMessage::decode(desc.clone(), black_box(bytes.as_slice())).unwrap())
    });
    c.bench_function("encode_scalars_with_unknown_fields", |b| {
        b.iter(|| black_box(dynamic.encode_to_vec()))
    });
}

fn decode_unknown_decomposed(c: &mut Criterion) {
    let desc = test_file_descriptor()
        .get_message_by_name("test.Scalars")
        .unwrap();
    let varint = scalars_with_varint_unknowns();
    let len = scalars_with_len_unknowns();
    let fixed64 = scalars_with_fixed64_unknowns();
    let group = scalars_with_group_unknowns();
    for (name, bytes) in [
        ("varint", &varint),
        ("len", &len),
        ("fixed64", &fixed64),
        ("group", &group),
    ] {
        let dynamic = DynamicMessage::decode(desc.clone(), bytes.as_slice()).unwrap();
        assert_eq!(dynamic.encode_to_vec(), *bytes, "roundtrip {name}");
        c.bench_function(&format!("decode_scalars_unknown_{name}"), |b| {
            b.iter(|| DynamicMessage::decode(desc.clone(), black_box(bytes.as_slice())).unwrap())
        });
    }
}

fn decode_cold_rotating(c: &mut Criterion) {
    // Table-residency contrast to the hot single-type benches: decode empty
    // payloads while rotating over every message type in the pool, so each
    // iteration misses the per-type table slice in L1/L2. The corpus is
    // small enough to stay L3-resident, so this measures the L2->L3 regime,
    // not DRAM-cold (a true cold registry needs a working set beyond L3).
    let pool = DescriptorPool::decode(DESCRIPTOR_SET_BYTES).unwrap();
    let descs: Vec<_> = pool.all_messages().collect();
    eprintln!("cold-rotate over {} message types", descs.len());
    c.bench_function("decode_empty_rotating_all_types", |b| {
        let mut i = 0usize;
        b.iter(|| {
            let desc = descs[i % descs.len()].clone();
            i += 1;
            DynamicMessage::decode(desc, black_box(&[][..])).unwrap()
        })
    });
}

fn route_field_read(c: &mut Criterion) {
    let bytes = scalars_sample().encode_to_vec();
    let complex_bytes = complex_sample().encode_to_vec();
    let desc = test_file_descriptor()
        .get_message_by_name("test.Scalars")
        .unwrap();
    let dynamic = DynamicMessage::decode(desc.clone(), bytes.as_slice()).unwrap();

    c.bench_function("route_read_cached_field", |b| {
        let field = desc.get_field_by_name("int32").unwrap();
        b.iter(|| {
            let value = dynamic.get_field(&field);
            black_box(value.as_ref().as_i32())
        })
    });
    c.bench_function("route_read_field_by_name", |b| {
        b.iter(|| {
            let field = dynamic.descriptor().get_field_by_name("int32").unwrap();
            let value = dynamic.get_field(&field);
            black_box(value.as_ref().as_i32())
        })
    });
    c.bench_function("route_read_nested_field", |b| {
        let complex_desc = test_file_descriptor()
            .get_message_by_name("test.ComplexType")
            .unwrap();
        let nested_desc = complex_desc.get_field_by_name("nested").unwrap();
        let inner_field = test_file_descriptor()
            .get_message_by_name("test.Scalars")
            .unwrap()
            .get_field_by_name("int32")
            .unwrap();
        let msg = DynamicMessage::decode(complex_desc, complex_bytes.as_slice()).unwrap();
        b.iter(|| {
            let nested = msg.get_field(&nested_desc);
            let Value::Message(inner) = nested.as_ref() else {
                unreachable!()
            };
            let value = inner.get_field(&inner_field);
            black_box(value.as_ref().as_i32())
        })
    });
}

fn json_complex(c: &mut Criterion) {
    use serde_json::de::Deserializer as JsonDe;
    use serde_json::Serializer as JsonSer;

    let desc = test_file_descriptor()
        .get_message_by_name("test.ComplexType")
        .unwrap();
    let dynamic = complex_sample().transcode_to_dynamic();

    let mut ser = JsonSer::new(Vec::new());
    dynamic
        .serialize_with_options(&mut ser, &SerializeOptions::new())
        .unwrap();
    let json_bytes = ser.into_inner();

    c.bench_function("json_serialize_complex", |b| {
        b.iter(|| {
            let mut ser = JsonSer::new(Vec::new());
            dynamic
                .serialize_with_options(&mut ser, &SerializeOptions::new())
                .unwrap();
            black_box(ser.into_inner())
        })
    });
    c.bench_function("json_deserialize_complex", |b| {
        b.iter(|| {
            let mut de = JsonDe::from_slice(black_box(json_bytes.as_slice()));
            let msg = DynamicMessage::deserialize_with_options(
                desc.clone(),
                &mut de,
                &DeserializeOptions::new(),
            )
            .unwrap();
            de.end().unwrap();
            black_box(msg)
        })
    });
}

fn encode_complex_mt(c: &mut Criterion, threads: usize, per_batch: usize, reuse: bool) {
    let desc = test_file_descriptor()
        .get_message_by_name("test.ComplexType")
        .unwrap();
    let bytes = complex_sample().encode_to_vec();
    let dynamic = DynamicMessage::decode(desc, bytes.as_slice()).unwrap();
    let suffix = if reuse { "_reuse_buf" } else { "" };

    c.bench_function(
        &format!("encode_complex_dynamic_mt{threads}{suffix}"),
        |b| {
            b.iter_custom(|iters| {
                let start = Barrier::new(threads + 1);
                let done = Barrier::new(threads + 1);
                std::thread::scope(|s| {
                    for _ in 0..threads {
                        let (start, done) = (&start, &done);
                        let msg = &dynamic;
                        s.spawn(move || {
                            let mut buf = Vec::new();
                            for _ in 0..iters {
                                start.wait();
                                for _ in 0..per_batch {
                                    if reuse {
                                        buf.clear();
                                        msg.encode(&mut buf).unwrap();
                                        black_box(buf.len());
                                    } else {
                                        // Allocating path: fresh output buffer per
                                        // message, like encode_to_vec. Contrasts
                                        // allocator churn vs buffer reuse.
                                        black_box(msg.encode_to_vec());
                                    }
                                }
                                done.wait();
                            }
                        });
                    }
                    let t0 = Instant::now();
                    for _ in 0..iters {
                        start.wait();
                        done.wait();
                    }
                    t0.elapsed()
                })
            })
        },
    );
}

fn encode_complex_mt_benches(c: &mut Criterion) {
    encode_complex_mt(c, 1, 200, false);
    encode_complex_mt(c, 4, 200, false);
    encode_complex_mt(c, 16, 200, false);
    encode_complex_mt(c, 1, 200, true);
    encode_complex_mt(c, 4, 200, true);
    encode_complex_mt(c, 16, 200, true);
}

/// Pin the calling thread to one logical processor. Windows enumerates HT
/// siblings as adjacent pairs (2i, 2i+1), so even ids are distinct physical
/// cores on this box.
#[cfg(windows)]
fn pin_to_core(logical: usize) {
    unsafe {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentThread() -> *mut core::ffi::c_void;
            fn SetThreadAffinityMask(
                hThread: *mut core::ffi::c_void,
                dwThreadAffinityMask: usize,
            ) -> usize;
        }
        let mask = 1usize << logical;
        let result = SetThreadAffinityMask(GetCurrentThread(), mask);
        assert!(result != 0, "affinity failed for logical core {logical}");
    }
}

#[cfg(not(windows))]
fn pin_to_core(_logical: usize) {}

fn encode_complex_mt_pinned(c: &mut Criterion, threads: usize, per_batch: usize) {
    let desc = test_file_descriptor()
        .get_message_by_name("test.ComplexType")
        .unwrap();
    let bytes = complex_sample().encode_to_vec();
    let dynamic = DynamicMessage::decode(desc, bytes.as_slice()).unwrap();

    c.bench_function(&format!("encode_complex_dynamic_mt{threads}_pinned"), |b| {
        b.iter_custom(|iters| {
            let start = Barrier::new(threads + 1);
            let done = Barrier::new(threads + 1);
            std::thread::scope(|s| {
                for i in 0..threads {
                    let (start, done) = (&start, &done);
                    let msg = &dynamic;
                    s.spawn(move || {
                        pin_to_core(2 * i);
                        let mut buf = Vec::new();
                        for _ in 0..iters {
                            start.wait();
                            for _ in 0..per_batch {
                                buf.clear();
                                msg.encode(&mut buf).unwrap();
                                black_box(buf.len());
                            }
                            done.wait();
                        }
                    });
                }
                let t0 = Instant::now();
                for _ in 0..iters {
                    start.wait();
                    done.wait();
                }
                t0.elapsed()
            })
        })
    });
}

fn encode_complex_mt_pinned_benches(c: &mut Criterion) {
    encode_complex_mt_pinned(c, 8, 200);
    encode_complex_mt_pinned(c, 16, 200);
}

fn encode_static_mt(c: &mut Criterion, threads: usize, per_batch: usize) {
    let msg = complex_sample();
    c.bench_function(&format!("encode_complex_static_mt{threads}"), |b| {
        b.iter_custom(|iters| {
            let start = Barrier::new(threads + 1);
            let done = Barrier::new(threads + 1);
            std::thread::scope(|s| {
                for _ in 0..threads {
                    let (start, done) = (&start, &done);
                    let msg = &msg;
                    s.spawn(move || {
                        for _ in 0..iters {
                            start.wait();
                            for _ in 0..per_batch {
                                black_box(msg.encode_to_vec());
                            }
                            done.wait();
                        }
                    });
                }
                let t0 = Instant::now();
                for _ in 0..iters {
                    start.wait();
                    done.wait();
                }
                t0.elapsed()
            })
        })
    });
}

fn encode_static_mt_benches(c: &mut Criterion) {
    encode_static_mt(c, 1, 2000);
    encode_static_mt(c, 4, 2000);
    encode_static_mt(c, 16, 2000);
}

criterion_group!(
    proxy_benches,
    wkt_global_pool_init,
    pool_decode,
    lookup_by_name,
    decode_encode_scalars,
    decode_encode_complex,
    decode_encode_sparse,
    decode_unknown_fields,
    decode_unknown_decomposed,
    decode_cold_rotating,
    route_field_read,
    json_complex,
    encode_complex_mt_benches,
    encode_complex_mt_pinned_benches,
    encode_static_mt_benches,
);
criterion_main!(proxy_benches);
