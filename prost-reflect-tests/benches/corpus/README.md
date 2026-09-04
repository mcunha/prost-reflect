# Real-payload benchmark corpus (google/benchmarks datasets)

Vendored from the official protobuf benchmark corpus for real-workload
evidence: dynamic-vs-static and stock-vs-patched measurements on
anonymized real payloads, not synthetic shapes.

## Provenance

- Source: `protocolbuffers/protobuf`, tag **v21.12** (BSD-3-Clause),
  `benchmarks/datasets/`
- Files:
  - `benchmark_message1_proto2.proto` + `dataset.google_message1_proto2.pb`
    (package `benchmarks.proto2`, message `GoogleMessage1`)
  - `benchmark_message1_proto3.proto` + `dataset.google_message1_proto3.pb`
    (package `benchmarks.proto3`, message `GoogleMessage1`)
  - `benchmark_message2.proto` + `dataset.google_message2.pb`
    (package `benchmarks.proto2`, message `GoogleMessage2`)
  - `benchmarks.proto` (the `BenchmarkDataset` wrapper used by the corpus)
- The `.proto` files carry the full BSD-3 license text in their headers;
  the license applies to the `.pb` payload files identically (they are
  published under the same tag and license).

## Dataset format

Each `.pb` file is a serialized `BenchmarkDataset`:
`{ name, message_name, repeated bytes payload }`. Payloads are parsed
or serialized in sequence in a loop; a dataset may hold a single large
payload (as `google_message2` does: one 84,570-byte real message), so
benchmarks repeat that payload per iteration - the corpus README notes
single-payload repetition can be cache-friendlier than mixed data.

## Non-Google real-world schemas (Apache-2.0)

Shape-faithful payloads generated from real public schemas (payload
bytes are generated representatives, not recorded traffic; the schemas
and licenses are real):

| Domain | Source | Package / message |
|---|---|---|
| gRPC health check | `grpc/grpc-proto` master, `grpc/health/v1/health.proto` | `grpc.health.v1.HealthCheckRequest` |
| etcd raft | `etcd-io/raft` main, `raftpb/raft.proto` (imports `gogoproto/gogo.proto`, include-only; its options are ignored by prost - wire-identical) | `raftpb.Message` |
| OpenTelemetry | `open-telemetry/opentelemetry-proto` main, `opentelemetry/proto/{common,resource,metrics}/v1/` | `opentelemetry.proto.metrics.v1.ResourceMetrics` |
| Prometheus remote write | `prometheus/prometheus` main, `prompb/{types,remote}.proto` | `prometheus.WriteRequest` |

Builders: `prost-reflect-tests/src/corpus_shapes.rs` (single source of
truth for the `real` bench and the parity gates).

## Message inventory (benchmark payload character)

- `GoogleMessage1` proto2/proto3 (228 B payload): small, presence-rich
  proto2 vs no-presence proto3 variants of the same shape.
- `GoogleMessage2` (84,570 B payload): large real message, thousands of
  field occurrences incl. nested repeated messages - the pathological
  case for per-occurrence dynamic machinery.

Vendored under `prost-reflect-tests/benches/corpus/`; regenerable with:
`curl -LO https://raw.githubusercontent.com/protocolbuffers/protobuf/v21.12/benchmarks/datasets/...`
