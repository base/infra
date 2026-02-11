//! Utility helpers for the RPC Collector.

use alloy_primitives::{Address, U256, utils::Unit};

/// OP Stack deposit (system) transaction type (`0x7E`).
///
/// Transactions with this type are injected by the sequencer and should
/// generally be excluded from user-facing metrics (priority fees, L2 fees, etc.).
pub const DEPOSIT_TX_TYPE: u8 = 126;

/// Convert a wei amount to the given [`Unit`] as an `f64`.
///
/// Uses direct numeric division — no string formatting round-trip.
/// For very large values this may lose precision, but it is sufficient
/// for metric reporting.
pub fn wei_to_unit(wei: U256, unit: Unit) -> f64 {
    let divisor = 10_f64.powi(unit.get() as i32);
    u128::try_from(wei).unwrap_or(u128::MAX) as f64 / divisor
}

/// Parsed contract-balance call triple (`metric|address|calldata`).
#[derive(Debug, Clone)]
pub struct ContractBalanceCall {
    pub metric: String,
    pub address: Address,
    pub calldata: Vec<u8>,
}

/// Parse `metric|address|calldata` triples from a string slice.
pub fn parse_contract_balance_calls(raw: &[String]) -> anyhow::Result<Vec<ContractBalanceCall>> {
    raw.iter()
        .map(|v| {
            let parts: Vec<&str> = v.split('|').collect();
            if parts.len() != 3 {
                anyhow::bail!("invalid contract balance call flag: {v}");
            }
            Ok(ContractBalanceCall {
                metric: parts[0].to_string(),
                address: parts[1].parse()?,
                calldata: alloy_primitives::hex::decode(parts[2])?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── wei_to_unit ──────────────────────────────────────────

    #[test]
    fn wei_to_unit_one_ether() {
        let one_eth = U256::from(1_000_000_000_000_000_000u128);
        let result = wei_to_unit(one_eth, Unit::ETHER);
        assert!((result - 1.0).abs() < 1e-12, "expected ~1.0, got {result}");
    }

    #[test]
    fn wei_to_unit_one_gwei() {
        let result = wei_to_unit(U256::from(1_000_000_000u128), Unit::GWEI);
        assert!((result - 1.0).abs() < 1e-12, "expected ~1.0, got {result}");
    }

    #[test]
    fn wei_to_unit_gwei_large() {
        // 100 Gwei = 100e9 wei
        let result = wei_to_unit(U256::from(100_000_000_000u128), Unit::GWEI);
        assert!((result - 100.0).abs() < 1e-9, "expected ~100.0, got {result}");
    }

    // ── parse_contract_balance_calls ─────────────────────────

    #[test]
    fn parse_contract_balance_calls_valid() {
        let input =
            vec!["my.metric|0x0000000000000000000000000000000000000001|deadbeef".to_string()];
        let result = parse_contract_balance_calls(&input).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].metric, "my.metric");
        assert_eq!(
            result[0].address,
            "0x0000000000000000000000000000000000000001".parse::<Address>().unwrap()
        );
        assert_eq!(result[0].calldata, vec![0xde, 0xad, 0xbe, 0xef]);
    }

    #[test]
    fn parse_contract_balance_calls_invalid_parts() {
        // Only two parts instead of three
        let input = vec!["my.metric|0x0000000000000000000000000000000000000001".to_string()];
        assert!(parse_contract_balance_calls(&input).is_err());
    }

    #[test]
    fn parse_contract_balance_calls_invalid_address() {
        let input = vec!["my.metric|not_an_address|deadbeef".to_string()];
        assert!(parse_contract_balance_calls(&input).is_err());
    }

    #[test]
    fn parse_contract_balance_calls_empty() {
        let input: Vec<String> = vec![];
        let result = parse_contract_balance_calls(&input).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn parse_contract_balance_calls_multiple() {
        let input = vec![
            "metric.a|0x0000000000000000000000000000000000000001|aa".to_string(),
            "metric.b|0x0000000000000000000000000000000000000002|bb".to_string(),
        ];
        let result = parse_contract_balance_calls(&input).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].metric, "metric.a");
        assert_eq!(result[1].metric, "metric.b");
    }
}
