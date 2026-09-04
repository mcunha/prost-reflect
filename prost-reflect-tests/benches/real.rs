//! Real-workload benches: dynamic vs static decode/encode on real
//! payloads from the official protobuf benchmark corpus (google_message1
//! proto2/proto3, google_message2 - real serialized bytes) and
//! shape-faithful payloads of non-Google real-world schemas (grpc
//! health, etcd raft, OpenTelemetry metrics, Prometheus remote write).
//! See benches/corpus/README.md for provenance and the payload-realism
//! caveat (real bytes exist only for the Google corpus).

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use prost::Message;
use prost_reflect::DynamicMessage;

use prost_reflect_tests::corpus::benchmarks::proto2::{
    GoogleMessage1 as Gm1Proto2, GoogleMessage2,
};
use prost_reflect_tests::corpus::benchmarks::proto3::GoogleMessage1 as Gm1Proto3;
use prost_reflect_tests::corpus::benchmarks::BenchmarkDataset;
use prost_reflect_tests::corpus::grpc::health::v1::HealthCheckRequest;
use prost_reflect_tests::corpus::opentelemetry::proto::metrics::v1::ResourceMetrics;
use prost_reflect_tests::corpus::prometheus::WriteRequest;
use prost_reflect_tests::corpus::raftpb::Message as RaftMessage;
use prost_reflect_tests::corpus_file_descriptor;
use prost_reflect_tests::corpus_shapes;

/// First payload of a dataset file (the corpus stores payloads inside a
/// `BenchmarkDataset` wrapper); leaked so bench closures can borrow it
/// for the process lifetime. Each vendored dataset holds one payload.
fn dataset_payload(file: &'static [u8]) -> &'static [u8] {
    let dataset = BenchmarkDataset::decode(file).expect("dataset wrapper");
    assert_eq!(
        dataset.payload.len(),
        1,
        "vendored datasets hold one payload"
    );
    Box::leak(
        dataset
            .payload
            .into_iter()
            .next()
            .unwrap()
            .into_boxed_slice(),
    )
}

/// Registers decode/encode benches for one workload: dynamic vs static on
/// the same bytes. `desc_name` is the fully-qualified message name in the
/// corpus descriptor pool.
fn workload<S: Message + Clone + Default>(
    c: &mut Criterion,
    id: &str,
    desc_name: &str,
    bytes: &'static [u8],
) {
    let pool = corpus_file_descriptor();
    let desc = pool
        .get_message_by_name(desc_name)
        .unwrap_or_else(|| panic!("{id}: no descriptor {desc_name}"));
    eprintln!(
        "real bench {id}: {} payload bytes ({desc_name})",
        bytes.len()
    );

    c.bench_function(&format!("real_{id}_decode_dynamic"), |b| {
        let desc = desc.clone();
        b.iter(|| DynamicMessage::decode(desc.clone(), black_box(bytes)).unwrap())
    });
    c.bench_function(&format!("real_{id}_decode_static"), |b| {
        b.iter(|| S::decode(black_box(bytes)).unwrap())
    });

    let dynamic = DynamicMessage::decode(desc.clone(), bytes).expect("{id}: dynamic encode prep");
    c.bench_function(&format!("real_{id}_encode_dynamic"), |b| {
        b.iter(|| black_box(dynamic.encode_to_vec()))
    });
    let static_msg = S::decode(bytes).expect("{id}: static encode prep");
    c.bench_function(&format!("real_{id}_encode_static"), |b| {
        b.iter(|| black_box(static_msg.clone().encode_to_vec()))
    });
}

fn real_benches(c: &mut Criterion) {
    workload::<Gm1Proto2>(
        c,
        "gm1_proto2",
        "benchmarks.proto2.GoogleMessage1",
        dataset_payload(include_bytes!("corpus/dataset.google_message1_proto2.pb")),
    );
    workload::<Gm1Proto3>(
        c,
        "gm1_proto3",
        "benchmarks.proto3.GoogleMessage1",
        dataset_payload(include_bytes!("corpus/dataset.google_message1_proto3.pb")),
    );
    workload::<GoogleMessage2>(
        c,
        "gm2",
        "benchmarks.proto2.GoogleMessage2",
        dataset_payload(include_bytes!("corpus/dataset.google_message2.pb")),
    );
    workload::<HealthCheckRequest>(
        c,
        "health",
        "grpc.health.v1.HealthCheckRequest",
        Box::leak(corpus_shapes::health_request_bytes().into_boxed_slice()),
    );
    workload::<RaftMessage>(
        c,
        "raft",
        "raftpb.Message",
        Box::leak(corpus_shapes::raft_message_bytes().into_boxed_slice()),
    );
    workload::<ResourceMetrics>(
        c,
        "otel",
        "opentelemetry.proto.metrics.v1.ResourceMetrics",
        Box::leak(corpus_shapes::otel_metrics_bytes().into_boxed_slice()),
    );
    workload::<WriteRequest>(
        c,
        "prom",
        "prometheus.WriteRequest",
        Box::leak(corpus_shapes::prometheus_remote_write_bytes().into_boxed_slice()),
    );
}

criterion_group!(real, real_benches);
criterion_main!(real);
