use prost_reflect::{DescriptorPool, ReflectMessage};
use proto::Scalars;

#[cfg(test)]
mod arbitrary;
#[cfg(test)]
mod decode;
#[cfg(test)]
mod desc;
#[cfg(test)]
mod descriptor_api;
#[cfg(test)]
mod json;
#[cfg(test)]
mod real_corpus_gate;
#[cfg(test)]
mod text_format;

pub mod proto {
    #![allow(clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/test.rs"));
    include!(concat!(env!("OUT_DIR"), "/test2.rs"));

    pub mod options {
        include!(concat!(env!("OUT_DIR"), "/custom.options.rs"));
    }
}

pub mod corpus_shapes;

/// Real-payload benchmark corpus + non-Google real-world schemas (see
/// benches/corpus/README.md). prost emits leaf-relative cross-package
/// paths (e.g. `super::super::common::v1` from the metrics leaf), so
/// each generated file must be included at the leaf of ITS OWN package
/// chain; this module tree mirrors the proto packages exactly.
pub mod corpus {
    #![allow(clippy::all)]

    pub mod benchmarks {
        include!(concat!(env!("OUT_DIR"), "/benchmarks.rs"));
        pub mod proto2 {
            include!(concat!(env!("OUT_DIR"), "/benchmarks.proto2.rs"));
        }
        pub mod proto3 {
            include!(concat!(env!("OUT_DIR"), "/benchmarks.proto3.rs"));
        }
    }

    // grpc/health/v1
    pub mod grpc {
        pub mod health {
            pub mod v1 {
                include!(concat!(env!("OUT_DIR"), "/grpc.health.v1.rs"));
            }
        }
    }

    // raftpb
    pub mod raftpb {
        include!(concat!(env!("OUT_DIR"), "/raftpb.rs"));
    }

    // opentelemetry/proto/{common,resource,metrics}/v1
    pub mod opentelemetry {
        pub mod proto {
            pub mod common {
                pub mod v1 {
                    include!(concat!(
                        env!("OUT_DIR"),
                        "/opentelemetry.proto.common.v1.rs"
                    ));
                }
            }
            pub mod resource {
                pub mod v1 {
                    include!(concat!(
                        env!("OUT_DIR"),
                        "/opentelemetry.proto.resource.v1.rs"
                    ));
                }
            }
            pub mod metrics {
                pub mod v1 {
                    include!(concat!(
                        env!("OUT_DIR"),
                        "/opentelemetry.proto.metrics.v1.rs"
                    ));
                }
            }
        }
    }

    // prometheus (types.proto + remote.proto share the package)
    pub mod prometheus {
        include!(concat!(env!("OUT_DIR"), "/prometheus.rs"));
    }
}

const CORPUS_POOL_BYTES: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/corpus_descriptor_set.bin"));

pub fn corpus_file_descriptor() -> DescriptorPool {
    DescriptorPool::decode(CORPUS_POOL_BYTES).expect("corpus descriptor set must decode")
}

const DESCRIPTOR_POOL_BYTES: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/file_descriptor_set.bin"));

pub fn test_file_descriptor() -> DescriptorPool {
    // Ensure global pool is populated with test descriptors.
    let _ = Scalars::default().descriptor();

    DescriptorPool::global()
}
