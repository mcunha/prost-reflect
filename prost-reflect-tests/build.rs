use std::io;

fn main() -> io::Result<()> {
    let mut config = prost_build::Config::new();
    config
        .type_attribute(".test.Scalars", "#[cfg_attr(test, derive(::proptest_derive::Arbitrary))]")
        .type_attribute(".test.ScalarArrays", "#[cfg_attr(test, derive(::proptest_derive::Arbitrary))]")
        .type_attribute(".test.ComplexType", "#[cfg_attr(test, derive(::proptest_derive::Arbitrary))]")
        .type_attribute(".test.EnumCarrier", "#[cfg_attr(test, derive(::proptest_derive::Arbitrary))]")
        .type_attribute(".test.WellKnownTypes", "#[cfg_attr(test, derive(::proptest_derive::Arbitrary))]")
        .field_attribute(
            ".test.WellKnownTypes.timestamp",
            "#[cfg_attr(test, proptest(strategy = \"::proptest::option::of(crate::arbitrary::timestamp())\"))]",
        )
        .field_attribute(
            ".test.WellKnownTypes.duration",
            "#[cfg_attr(test, proptest(strategy = \"::proptest::option::of(crate::arbitrary::duration())\"))]",
        )
        .field_attribute(
            ".test.WellKnownTypes.struct",
            "#[cfg_attr(test, proptest(strategy = \"::proptest::option::of(crate::arbitrary::struct_())\"))]",
        )
        .field_attribute(
            ".test.WellKnownTypes.list",
            "#[cfg_attr(test, proptest(strategy = \"::proptest::option::of(crate::arbitrary::list())\"))]",
        )
        .field_attribute(
            ".test.WellKnownTypes.mask",
            "#[cfg_attr(test, proptest(strategy = \"::proptest::option::of(crate::arbitrary::mask())\"))]",
        )
        .field_attribute(
            ".test.WellKnownTypes.empty",
            "#[cfg_attr(test, proptest(strategy = \"::proptest::option::of(::proptest::strategy::Just(()))\"))]",
        )
        .field_attribute(".test.WellKnownTypes.null", "#[cfg_attr(test, proptest(value= \"0\"))]");

    prost_reflect_build::Builder::new()
        .file_descriptor_set_bytes("crate::DESCRIPTOR_POOL_BYTES")
        .compile_protos_with_config(
            config,
            &[
                "src/test.proto",
                "src/test2.proto",
                "src/desc.proto",
                "src/desc2.proto",
                "src/desc_no_package.proto",
                "src/imports.proto",
                "src/ext.proto",
                "src/options.proto",
            ],
            &["src/"],
        )?;

    // Real-payload benchmark corpus (google protobuf benchmarks, v21.12;
    // see benches/corpus/README.md): compiled into its own descriptor
    // set so the test pool stays untouched. Each generated file is
    // included under its own submodule in `corpus` (generated structs
    // are top-level, and GoogleMessage1 exists in both syntaxes).
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    let corpus_config = prost_build::Config::new();
    // gogo.proto is an import-only include (etcd raft options); it is not
    // compiled, and its options are ignored by prost (wire-identical).
    prost_reflect_build::Builder::new()
        .file_descriptor_set_bytes("crate::CORPUS_POOL_BYTES")
        .file_descriptor_set_path(format!("{out_dir}/corpus_descriptor_set.bin"))
        .compile_protos_with_config(
            corpus_config,
            &[
                // google protobuf benchmark corpus (v21.12) - relative to
                // the benches/corpus/ include
                "benchmarks.proto",
                "benchmark_message1_proto2.proto",
                "benchmark_message1_proto3.proto",
                "benchmark_message2.proto",
                // non-Google real-world schemas (Apache-2.0, see README),
                // each relative to its own include root so protoc sees one
                // canonical name per file
                "grpc/health/v1/health.proto",
                "etcd/raftpb/raft.proto",
                "opentelemetry/proto/common/v1/common.proto",
                "opentelemetry/proto/resource/v1/resource.proto",
                "opentelemetry/proto/metrics/v1/metrics.proto",
                "types.proto",
                "remote.proto",
            ],
            &[
                "benches/corpus/",
                "benches/corpus/non_google/",
                "benches/corpus/non_google/otel/",
                "benches/corpus/non_google/prom/prompb/",
            ],
        )?;
    Ok(())
}
