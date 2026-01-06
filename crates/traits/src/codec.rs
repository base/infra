//! Codec traits for encoding and decoding with configuration.

use bytes::Bytes;
use derive_more::{Debug, Display, Error};

/// Error type for codec operations.
#[derive(Debug, Display, Error)]
#[display("codec error: {_0}")]
#[error(ignore)]
pub struct CodecError(pub String);

impl CodecError {
    /// Create a new codec error.
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

/// Configuration for codec operations.
///
/// This trait allows codecs to be configured with limits and options.
pub trait CodecConfig: Clone + Send + Sync + 'static {
    /// Maximum size in bytes for encoded data.
    fn max_size(&self) -> usize;

    /// Maximum nesting depth for nested structures.
    fn max_depth(&self) -> usize;

    /// Whether to allow batch requests.
    fn allow_batch(&self) -> bool;

    /// Maximum number of requests in a batch.
    fn max_batch_size(&self) -> usize;
}

/// Default codec configuration.
#[derive(Clone, Debug)]
pub struct DefaultCodecConfig {
    max_size: usize,
    max_depth: usize,
    allow_batch: bool,
    max_batch_size: usize,
}

impl DefaultCodecConfig {
    /// Create a new default codec configuration.
    pub const fn new() -> Self {
        Self {
            max_size: 1024 * 1024, // 1 MB
            max_depth: 32,
            allow_batch: true,
            max_batch_size: 100,
        }
    }

    /// Set the maximum size.
    pub const fn with_max_size(mut self, size: usize) -> Self {
        self.max_size = size;
        self
    }

    /// Set the maximum depth.
    pub const fn with_max_depth(mut self, depth: usize) -> Self {
        self.max_depth = depth;
        self
    }

    /// Set whether to allow batch requests.
    pub const fn with_allow_batch(mut self, allow: bool) -> Self {
        self.allow_batch = allow;
        self
    }

    /// Set the maximum batch size.
    pub const fn with_max_batch_size(mut self, size: usize) -> Self {
        self.max_batch_size = size;
        self
    }
}

impl Default for DefaultCodecConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl CodecConfig for DefaultCodecConfig {
    fn max_size(&self) -> usize {
        self.max_size
    }

    fn max_depth(&self) -> usize {
        self.max_depth
    }

    fn allow_batch(&self) -> bool {
        self.allow_batch
    }

    fn max_batch_size(&self) -> usize {
        self.max_batch_size
    }
}

/// Trait for decoding bytes into a type with configuration.
pub trait Decode: Sized {
    /// Decode from bytes using the given configuration.
    fn decode<C: CodecConfig>(bytes: &[u8], config: &C) -> Result<Self, CodecError>;

    /// Decode from Bytes using the given configuration.
    fn decode_bytes<C: CodecConfig>(bytes: &Bytes, config: &C) -> Result<Self, CodecError> {
        Self::decode(bytes.as_ref(), config)
    }
}

/// Trait for encoding a type into bytes.
pub trait Encode {
    /// Encode into bytes.
    fn encode(&self) -> Result<Bytes, CodecError>;

    /// Encode into a `Vec<u8>`.
    fn encode_vec(&self) -> Result<Vec<u8>, CodecError> {
        self.encode().map(|b| b.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_codec_config() {
        let config = DefaultCodecConfig::new();
        assert_eq!(config.max_size(), 1024 * 1024);
        assert_eq!(config.max_depth(), 32);
        assert!(config.allow_batch());
        assert_eq!(config.max_batch_size(), 100);
    }

    #[test]
    fn test_codec_config_builder() {
        let config = DefaultCodecConfig::new()
            .with_max_size(512)
            .with_max_depth(16)
            .with_allow_batch(false)
            .with_max_batch_size(10);

        assert_eq!(config.max_size(), 512);
        assert_eq!(config.max_depth(), 16);
        assert!(!config.allow_batch());
        assert_eq!(config.max_batch_size(), 10);
    }

    #[test]
    fn test_codec_error_display() {
        let error = CodecError::new("invalid json");
        assert!(error.to_string().contains("invalid json"));
    }
}
