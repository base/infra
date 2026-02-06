use alloy_primitives::Address;
use alloy_signer_local::{LocalSigner, MnemonicBuilder, PrivateKeySigner, coins_bip39::English};
use anyhow::{Context, Result};

/// Derives `count` signers from the given mnemonic using BIP-44 path m/44'/60'/0'/0/i
/// Starting from index `offset` (i.e., derives indices offset..offset+count)
/// Skips any addresses in `skip_addresses` (e.g., the funder address)
pub(crate) fn derive_signers(
    mnemonic: &str,
    count: u32,
    offset: u32,
    skip_addresses: &[Address],
) -> Result<Vec<PrivateKeySigner>> {
    let mut signers = Vec::with_capacity(count as usize);
    let mut index = offset;

    while signers.len() < count as usize {
        let signer = MnemonicBuilder::<English>::default()
            .phrase(mnemonic)
            .index(index)?
            .build()
            .with_context(|| format!("Failed to derive signer at index {index}"))?;

        if skip_addresses.contains(&signer.address()) {
            tracing::warn!(index, address = %signer.address(), "Skipping derived address");
            index += 1;
            continue;
        }

        signers.push(signer);
        index += 1;
    }

    Ok(signers)
}

/// Parses a private key from hex string (with or without 0x prefix)
pub(crate) fn parse_funder_key(hex_key: &str) -> Result<PrivateKeySigner> {
    let key = hex_key.strip_prefix("0x").unwrap_or(hex_key);
    key.parse::<PrivateKeySigner>().context("Failed to parse funder private key")
}

/// Get addresses from signers
pub(crate) fn get_addresses(signers: &[PrivateKeySigner]) -> Vec<Address> {
    signers.iter().map(LocalSigner::address).collect()
}
