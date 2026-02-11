//! RPC Collector — collects `OPStack` chain metrics from L1 and L2 RPC endpoints.

pub mod bindings;
pub mod metrics;
pub mod recorders;
pub mod services;
pub mod utils;

/// Test helpers shared across unit test modules.
#[cfg(test)]
pub(crate) mod test_helpers {
    use alloy_consensus::{Signed, TxEnvelope, TxLegacy, transaction::Recovered};
    use alloy_network_primitives::BlockTransactions;
    use alloy_primitives::{Address, B256, Bytes, Signature, TxKind, U256};
    use alloy_rpc_types_eth::{
        Block as RpcBlock, Header as RpcHeader, Transaction as RpcTransaction,
    };

    /// The concrete RPC block type used throughout the crate.
    pub(crate) type Block =
        RpcBlock<RpcTransaction<TxEnvelope>, RpcHeader<alloy_consensus::Header>>;
    /// The concrete RPC header type.
    #[allow(dead_code)]
    pub(crate) type Header = RpcHeader<alloy_consensus::Header>;
    /// The concrete RPC transaction type.
    pub(crate) type Transaction = RpcTransaction<TxEnvelope>;

    /// Build a minimal [`Block`] for testing with the given header fields.
    ///
    /// All other fields are left at their defaults.
    pub(crate) fn make_block(number: u64, hash: B256, parent_hash: B256) -> Block {
        let mut block: Block = Default::default();
        block.header.hash = hash;
        block.header.inner.number = number;
        block.header.inner.parent_hash = parent_hash;
        block
    }

    /// Build a minimal [`Block`] with the given transactions.
    pub(crate) fn make_block_with_txs(number: u64, txs: Vec<Transaction>) -> Block {
        let mut block: Block = Default::default();
        block.header.inner.number = number;
        block.transactions = BlockTransactions::Full(txs);
        block
    }

    /// Build a minimal RPC [`Header`] for testing.
    #[allow(dead_code)]
    pub(crate) fn make_header(number: u64, hash: B256, parent_hash: B256) -> Header {
        Header {
            hash,
            inner: alloy_consensus::Header { number, parent_hash, ..Default::default() },
            ..Default::default()
        }
    }

    /// Build a minimal RPC [`Transaction`] (legacy type) for testing.
    ///
    /// `from` is the sender, `to` is the recipient, and `input` is the calldata.
    pub(crate) fn make_legacy_tx(from: Address, to: Address, input: Bytes) -> Transaction {
        let tx_legacy = TxLegacy {
            chain_id: Some(1),
            nonce: 0,
            gas_price: 1_000_000_000,
            gas_limit: 21_000,
            to: TxKind::Call(to),
            value: U256::ZERO,
            input,
        };

        let fake_sig = Signature::new(U256::from(1u64), U256::from(2u64), false);
        let fake_hash = B256::default();
        let signed = Signed::new_unchecked(tx_legacy, fake_sig, fake_hash);
        let envelope = TxEnvelope::Legacy(signed);
        let recovered = Recovered::new_unchecked(envelope, from);

        RpcTransaction {
            inner: recovered,
            block_hash: None,
            block_number: None,
            transaction_index: None,
            effective_gas_price: None,
        }
    }
}
