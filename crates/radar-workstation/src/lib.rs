pub mod chunk;

pub use chunk::{decompress_chunk, detect_chunk_kind, ChunkError, ChunkKind};
