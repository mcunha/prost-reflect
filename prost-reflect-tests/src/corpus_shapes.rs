//! Shape-faithful payload builders for the non-Google real-world schemas
//! (grpc health, etcd raft, OpenTelemetry metrics, Prometheus remote
//! write). The schemas are vendored real ones (see benches/corpus/README.md);
//! the payloads are generated representatives of realistic content - real
//! bytes only exist for the Google corpus datasets. Single source of truth
//! shared by the `real` bench and the real-corpus parity tests.

use prost::Message as _;

use crate::corpus::grpc::health::v1::{
    health_check_response::ServingStatus, HealthCheckRequest, HealthCheckResponse,
};
use crate::corpus::opentelemetry::proto::common::v1::{
    any_value::Value as AnyValueValue, AnyValue, InstrumentationScope, KeyValue,
};
use crate::corpus::opentelemetry::proto::metrics::v1::{
    metric::Data as MetricData, number_data_point::Value as NumberPointValue,
    AggregationTemporality, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, Sum,
};
use crate::corpus::opentelemetry::proto::resource::v1::Resource;
use crate::corpus::prometheus::{Label, Sample, TimeSeries, WriteRequest};
use crate::corpus::raftpb::{Entry, Message as RaftMessage, MessageType};

pub fn health_request_bytes() -> Vec<u8> {
    HealthCheckRequest {
        service: "cart.service".to_owned(),
    }
    .encode_to_vec()
}

pub fn health_response_bytes() -> Vec<u8> {
    HealthCheckResponse {
        status: ServingStatus::Serving as i32,
    }
    .encode_to_vec()
}

pub fn raft_message_bytes() -> Vec<u8> {
    RaftMessage {
        r#type: Some(MessageType::MsgApp as i32),
        to: Some(2),
        from: Some(1),
        term: Some(7),
        log_term: Some(6),
        index: Some(1024),
        entries: vec![
            Entry {
                term: Some(7),
                index: Some(1025),
                data: Some(b"set key=value".to_vec()),
                ..Default::default()
            },
            Entry {
                term: Some(7),
                index: Some(1026),
                data: Some(b"delete key".to_vec()),
                ..Default::default()
            },
        ],
        commit: Some(1026),
        ..Default::default()
    }
    .encode_to_vec()
}

pub fn otel_metrics_bytes() -> Vec<u8> {
    let kv = |key: &str, value: &str| KeyValue {
        key: key.to_owned(),
        value: Some(AnyValue {
            value: Some(AnyValueValue::StringValue(value.to_owned())),
        }),
        ..Default::default()
    };
    let attributes = vec![
        kv("service.name", "checkout"),
        kv("deployment.environment", "prod"),
        kv("http.route", "/api/v1/orders"),
    ];
    let point = NumberDataPoint {
        attributes: vec![kv("http.status_code", "200")],
        time_unix_nano: 1_700_000_000_000_000_000,
        start_time_unix_nano: 1_699_990_000_000_000_000,
        value: Some(NumberPointValue::AsDouble(0.042)),
        ..Default::default()
    };
    let sum = Sum {
        data_points: vec![point],
        aggregation_temporality: AggregationTemporality::Cumulative as i32,
        is_monotonic: true,
    };
    ResourceMetrics {
        resource: Some(Resource {
            attributes,
            ..Default::default()
        }),
        scope_metrics: vec![ScopeMetrics {
            scope: Some(InstrumentationScope {
                name: "tier1.http".to_owned(),
                version: "1.0.0".to_owned(),
                attributes: vec![kv("schema", "semconv")],
                ..Default::default()
            }),
            metrics: vec![Metric {
                name: "http.server.duration".to_owned(),
                description: "per-request latency".to_owned(),
                unit: "s".to_owned(),
                data: Some(MetricData::Sum(sum)),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    }
    .encode_to_vec()
}

/// Realistic remote-write batch: 40 series x 30 samples (plus labels),
/// the shape a metrics frontend proxies.
pub fn prometheus_remote_write_bytes() -> Vec<u8> {
    let mut timeseries = Vec::with_capacity(40);
    for i in 0..40 {
        let labels = vec![
            Label {
                name: "__name__".to_owned(),
                value: "http_requests_total{}".to_owned(),
            },
            Label {
                name: "instance".to_owned(),
                value: format!("host-{i:03}:9100"),
            },
            Label {
                name: "job".to_owned(),
                value: "gateway".to_owned(),
            },
        ];
        let samples: Vec<Sample> = (0..30)
            .map(|s| Sample {
                value: (i * 1000 + s) as f64 * 0.5,
                timestamp: 1_700_000_000_000 + s * 15_000,
            })
            .collect();
        timeseries.push(TimeSeries {
            labels,
            samples,
            ..Default::default()
        });
    }
    WriteRequest {
        timeseries,
        ..Default::default()
    }
    .encode_to_vec()
}
