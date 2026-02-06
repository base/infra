use alloy_primitives::Bytes;
use rand::Rng;

const SMALL_CALLDATA_SIZE: usize = 20;

/// Generates random calldata of the specified size.
/// Uses high-entropy random bytes that are uncompressible.
fn generate_calldata(size: usize) -> Bytes {
    let mut rng = rand::thread_rng();
    let data: Vec<u8> = (0..size).map(|_| rng.r#gen()).collect();
    Bytes::from(data)
}

/// Generates small calldata (20 bytes)
pub(crate) fn generate_small_calldata() -> Bytes {
    generate_calldata(SMALL_CALLDATA_SIZE)
}

/// Generates large calldata of the specified max size
pub(crate) fn generate_large_calldata(max_size: usize) -> Bytes {
    generate_calldata(max_size)
}

/// Decides whether to use large calldata based on the load percentage
pub(crate) fn should_use_large_calldata(load_percentage: u8) -> bool {
    let mut rng = rand::thread_rng();
    rng.gen_range(0..100) < load_percentage
}
