pub mod s3_poll;
pub mod volume_seq;

use bytes::Bytes;

use crate::chunk::ChunkKind;

/// Raw bytes of a single NEXRAD chunk together with its identified type.
/// Produced by the S3 poller and consumed by the volume assembly state machine.
pub struct ChunkEnvelope {
    pub kind: ChunkKind,
    pub raw_bytes: Bytes,
}
