//! Parity gate over the real-payload corpus and the non-Google
//! real-schema shapes: dynamic decode -> re-encode must be semantically
//! identical to the static decode of the original bytes. This is the
//! real-data version of the synthetic roundtrip/parity props - the
//! google_message* payloads are real serialized bytes from the official
//! protobuf benchmark corpus (see benches/corpus/README.md), and the
//! non-Google shapes are faithful representatives of real schemas.

use prost::Message;
use prost_reflect::DynamicMessage;

use crate::corpus::benchmarks::proto2::{GoogleMessage1 as Gm1Proto2, GoogleMessage2};
use crate::corpus::benchmarks::proto3::GoogleMessage1 as Gm1Proto3;
use crate::corpus::benchmarks::BenchmarkDataset;
use crate::corpus::grpc::health::v1::{HealthCheckRequest, HealthCheckResponse};
use crate::corpus::opentelemetry::proto::metrics::v1::ResourceMetrics;
use crate::corpus::prometheus::WriteRequest;
use crate::corpus::raftpb::Message as RaftMessage;
use crate::corpus_file_descriptor;

fn semantic_gate<M: Message + PartialEq + Default + std::fmt::Debug>(
    pool: &prost_reflect::DescriptorPool,
    message_name: &str,
    bytes: &[u8],
    label: &str,
) {
    let original = M::decode(bytes).unwrap_or_else(|e| panic!("{label}: static decode: {e}"));
    let desc = pool
        .get_message_by_name(message_name)
        .unwrap_or_else(|| panic!("{label}: no descriptor {message_name}"));
    let dynamic = DynamicMessage::decode(desc, bytes)
        .unwrap_or_else(|e| panic!("{label}: dynamic decode: {e}"));
    let roundtrip = dynamic.encode_to_vec();
    let redecoded = M::decode(roundtrip.as_slice())
        .unwrap_or_else(|e| panic!("{label}: static decode of dynamic output: {e}"));
    assert_eq!(
        redecoded, original,
        "{label}: semantic drift through dynamic decode/encode"
    );
}

fn gate_dataset<M: Message + PartialEq + Default + std::fmt::Debug>(
    name: &str,
    data: &'static [u8],
) {
    let pool = corpus_file_descriptor();
    let dataset = BenchmarkDataset::decode(data).expect("dataset wrapper");
    let label = format!("dataset {name} ({})", dataset.message_name);
    assert_eq!(dataset.name, name, "{label}: wrapper name mismatch");
    assert!(!dataset.payload.is_empty(), "{label}: no payloads");
    for (i, payload) in dataset.payload.iter().enumerate() {
        semantic_gate::<M>(
            &pool,
            dataset.message_name.as_str(),
            payload,
            &format!("{label} payload {i}"),
        );
    }
    eprintln!(
        "gate {name}: {} payload(s), {} total bytes ok",
        dataset.payload.len(),
        dataset.payload.iter().map(Vec::len).sum::<usize>()
    );
}

#[test]
fn google_message1_proto2_real_payloads() {
    gate_dataset::<Gm1Proto2>(
        "google_message1_proto2",
        include_bytes!("../benches/corpus/dataset.google_message1_proto2.pb"),
    );
}

#[test]
fn google_message1_proto3_real_payloads() {
    gate_dataset::<Gm1Proto3>(
        "google_message1_proto3",
        include_bytes!("../benches/corpus/dataset.google_message1_proto3.pb"),
    );
}

#[test]
fn google_message2_real_payload() {
    gate_dataset::<GoogleMessage2>(
        "google_message2",
        include_bytes!("../benches/corpus/dataset.google_message2.pb"),
    );
}

#[test]
fn grpc_health_shapes() {
    let pool = corpus_file_descriptor();
    semantic_gate::<HealthCheckRequest>(
        &pool,
        "grpc.health.v1.HealthCheckRequest",
        &crate::corpus_shapes::health_request_bytes(),
        "health request",
    );
    semantic_gate::<HealthCheckResponse>(
        &pool,
        "grpc.health.v1.HealthCheckResponse",
        &crate::corpus_shapes::health_response_bytes(),
        "health response",
    );
}

#[test]
fn raft_message_shape() {
    let pool = corpus_file_descriptor();
    semantic_gate::<RaftMessage>(
        &pool,
        "raftpb.Message",
        &crate::corpus_shapes::raft_message_bytes(),
        "raft message",
    );
}

#[test]
fn otel_resource_metrics_shape() {
    let pool = corpus_file_descriptor();
    semantic_gate::<ResourceMetrics>(
        &pool,
        "opentelemetry.proto.metrics.v1.ResourceMetrics",
        &crate::corpus_shapes::otel_metrics_bytes(),
        "otel resource metrics",
    );
}

#[test]
fn prometheus_remote_write_shape() {
    let pool = corpus_file_descriptor();
    semantic_gate::<WriteRequest>(
        &pool,
        "prometheus.WriteRequest",
        &crate::corpus_shapes::prometheus_remote_write_bytes(),
        "prometheus remote write",
    );
}
