//! Allocation accounting for dynamic decode/encode: per-operation allocation
//! count, bytes, and peak live bytes. Uses a counting global allocator, so
//! this binary is for allocation data only — never timing. The counting
//! wrapper perturbs the timing suite, hence the separate target.
//!
//! Grounds two spec claims: per-message garbage volume (arena payoff) and the
//! input-copy penalty (zero-copy borrow decision in INPUT OWNERSHIP).
//!
//! Accounting invariants (read before extending):
//! - The counters are only meaningful when each measured phase is
//!   single-threaded and nothing allocated before a `reset()` is dropped
//!   inside the measured window; every phase in `main` obeys this. A
//!   threaded phase would mix other threads' allocations into per-op
//!   counts; dropping a pre-reset object mid-phase would drive LIVE
//!   negative and corrupt the PEAK delta.
//! - `reset()` zeroes LIVE while genuinely live baseline objects (the
//!   descriptor pool, fixtures) remain allocated, so reported peaks are
//!   per-phase deltas above that baseline, not absolute live bytes.
//! - PEAK cannot observe the transient double-live window inside a moving
//!   allocator realloc (old and new blocks alive during the copy), so peak
//!   live is a lower bound for phases whose buffers grow via realloc.

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};

use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage};
use prost_reflect_tests::proto::{ComplexType, Scalars};
use prost_reflect_tests::test_file_descriptor;

struct Counting;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        let live =
            LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed) + layout.size() as isize;
        PEAK.fetch_max(live as usize, Ordering::Relaxed);
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        System.dealloc(ptr, layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(new_size, Ordering::Relaxed);
        let delta = new_size as isize - layout.size() as isize;
        let live = LIVE.fetch_add(delta, Ordering::Relaxed) + delta;
        PEAK.fetch_max(live as usize, Ordering::Relaxed);
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

struct Snapshot {
    allocs: usize,
    bytes: usize,
    peak: usize,
}

fn snapshot() -> Snapshot {
    Snapshot {
        allocs: ALLOCS.load(Ordering::Relaxed),
        bytes: BYTES.load(Ordering::Relaxed),
        peak: PEAK.load(Ordering::Relaxed),
    }
}

fn reset() {
    ALLOCS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    LIVE.store(0, Ordering::Relaxed);
    PEAK.store(0, Ordering::Relaxed);
}

fn report(label: &str, iters: usize, after: Snapshot) {
    println!(
        "{label:<46} per-op: {:>6} allocs, {:>10} B allocated | peak live: {:>9} B",
        after.allocs / iters,
        after.bytes / iters,
        after.peak
    );
}

fn complex_sample() -> ComplexType {
    let scalars = Scalars {
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
    };
    ComplexType {
        string_map: std::collections::HashMap::from_iter([
            ("one".to_owned(), scalars.clone()),
            ("two".to_owned(), scalars.clone()),
        ]),
        int_map: std::collections::HashMap::from_iter([(1, scalars.clone())]),
        nested: Some(scalars),
        my_enum: vec![0, 1, 3],
        optional_enum: 1,
        enum_map: std::collections::HashMap::from_iter([(1, 3), (2, 0)]),
    }
}

fn main() {
    let desc = test_file_descriptor()
        .get_message_by_name("test.ComplexType")
        .unwrap();
    let bytes = complex_sample().encode_to_vec();
    let dynamic = DynamicMessage::decode(desc.clone(), bytes.as_slice()).unwrap();

    const ITERS: usize = 2_000;

    // Warm up (allocator, pool, caches) before snapshotting.
    for _ in 0..100 {
        black_box(DynamicMessage::decode(desc.clone(), bytes.as_slice()).unwrap());
    }
    reset();
    for _ in 0..ITERS {
        black_box(DynamicMessage::decode(desc.clone(), bytes.as_slice()).unwrap());
    }
    report("decode ComplexType (dynamic)", ITERS, snapshot());

    reset();
    for _ in 0..ITERS {
        black_box(dynamic.encode_to_vec());
    }
    report("encode ComplexType (dynamic)", ITERS, snapshot());

    let static_msg = complex_sample();
    reset();
    for _ in 0..ITERS {
        black_box(static_msg.encode_to_vec());
    }
    report("encode ComplexType (static)", ITERS, snapshot());

    // Zero-copy penalty: 1 MiB bytes field, decode from a borrowed slice.
    let big = Scalars {
        bytes: vec![0xAB; 1 << 20],
        ..Default::default()
    };
    let big_bytes = big.encode_to_vec();
    let scalars_desc = test_file_descriptor()
        .get_message_by_name("test.Scalars")
        .unwrap();
    for _ in 0..5 {
        black_box(DynamicMessage::decode(scalars_desc.clone(), big_bytes.as_slice()).unwrap());
    }
    reset();
    for _ in 0..ITERS {
        black_box(DynamicMessage::decode(scalars_desc.clone(), big_bytes.as_slice()).unwrap());
    }
    report("decode Scalars + 1 MiB bytes field", ITERS, snapshot());

    // Descriptor pool load accounting, for the ingest comparison.
    const FDS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/file_descriptor_set.bin"));
    for _ in 0..5 {
        black_box(DescriptorPool::decode(FDS).unwrap());
    }
    reset();
    for _ in 0..20 {
        black_box(DescriptorPool::decode(FDS).unwrap());
    }
    let after = snapshot();
    println!(
        "pool load (132 KB FDS)               per-op: {:>6} allocs, {:>10} B allocated | peak live: {:>9} B",
        after.allocs / 20,
        after.bytes / 20,
        after.peak
    );
}
