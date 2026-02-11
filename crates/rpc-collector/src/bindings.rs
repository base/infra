//! Contract bindings via `alloy::sol!`.
//!
//! Only the functions/events we actually call are included.
//! Also re-exports well-known OP Stack predeploy addresses.

use alloy_primitives::{Address, address};
use alloy_sol_types::sol;

// ── OP Stack L2 Predeploy Addresses ─────────────────────────

/// `SequencerFeeVault` predeploy.
pub const SEQUENCER_FEE_VAULT: Address = address!("4200000000000000000000000000000000000011");
/// `BaseFeeVault` predeploy.
pub const BASE_FEE_VAULT: Address = address!("4200000000000000000000000000000000000019");
/// `L1FeeVault` predeploy.
pub const L1_FEE_VAULT: Address = address!("420000000000000000000000000000000000001A");
/// `L2ToL1MessagePasser` predeploy.
pub const L2_TO_L1_MSG_PASSER: Address = address!("4200000000000000000000000000000000000016");
/// `L2StandardBridge` predeploy.
pub const L2_STANDARD_BRIDGE: Address = address!("4200000000000000000000000000000000000010");
/// `L1Block` predeploy.
pub const L1_BLOCK: Address = address!("4200000000000000000000000000000000000015");
/// `GasPriceOracle` predeploy.
pub const GAS_PRICE_ORACLE: Address = address!("420000000000000000000000000000000000000F");

// ── GasPriceOracle (L2 predeploy 0x420000000000000000000000000000000000000F) ──

sol! {
    #[sol(rpc)]
    interface GasPriceOracle {
        function l1BaseFee() external view returns (uint256);
        function blobBaseFee() external view returns (uint256);
    }
}

// ── L2ToL1MessagePasser (L2 predeploy 0x4200000000000000000000000000000000000016) ──

sol! {
    #[sol(rpc)]
    interface L2ToL1MessagePasser {
        event MessagePassed(
            uint256 indexed nonce,
            address indexed sender,
            address indexed target,
            uint256 value,
            uint256 gasLimit,
            bytes data,
            bytes32 withdrawalHash
        );
    }
}

// ── L2StandardBridge (L2 predeploy 0x4200000000000000000000000000000000000010) ──

sol! {
    #[sol(rpc)]
    interface L2StandardBridge {
        event WithdrawalInitiated(
            address indexed l1Token,
            address indexed l2Token,
            address indexed from,
            address to,
            uint256 amount,
            bytes extraData
        );

        event DepositFinalized(
            address indexed l1Token,
            address indexed l2Token,
            address indexed from,
            address to,
            uint256 amount,
            bytes extraData
        );
    }
}

// ── L1StandardBridge ──

sol! {
    #[sol(rpc)]
    interface L1StandardBridge {
        event ETHBridgeInitiated(
            address indexed from,
            address indexed to,
            uint256 amount,
            bytes extraData
        );

        event ERC20BridgeInitiated(
            address indexed localToken,
            address indexed remoteToken,
            address indexed from,
            address to,
            uint256 amount,
            bytes extraData
        );

        event ETHWithdrawalFinalized(
            address indexed from,
            address indexed to,
            uint256 amount,
            bytes extraData
        );

        event ERC20WithdrawalFinalized(
            address indexed localToken,
            address indexed remoteToken,
            address indexed from,
            address to,
            uint256 amount,
            bytes extraData
        );
    }
}

// ── OptimismPortal ──

sol! {
    #[sol(rpc)]
    interface OptimismPortal {
        event TransactionDeposited(
            address indexed from,
            address indexed to,
            uint256 indexed version,
            bytes opaqueData
        );

        event WithdrawalFinalized(bytes32 indexed withdrawalHash, bool success);
    }
}

// ── L2OutputOracle (legacy) ──

sol! {
    #[sol(rpc)]
    interface L2OutputOracle {
        function latestBlockNumber() external view returns (uint256);
    }
}

// ── DisputeGameFactory ──

sol! {
    #[sol(rpc)]
    interface DisputeGameFactory {
        function gameCount() external view returns (uint256);

        function gameAtIndex(uint256 _index) external view returns (
            uint32 gameType,
            uint64 timestamp,
            address proxy
        );
    }
}

// ── FaultDisputeGame (proxied via DisputeGameFactory) ──

sol! {
    #[sol(rpc)]
    interface FaultDisputeGame {
        function l2BlockNumber() external view returns (uint256);
    }
}

// ── SystemConfig (L1) ──

sol! {
    #[sol(rpc)]
    interface SystemConfig {
        function eip1559Elasticity() external view returns (uint32);
        function eip1559Denominator() external view returns (uint32);
        function scalar() external view returns (uint256);
        function basefeeScalar() external view returns (uint32);
        function blobbasefeeScalar() external view returns (uint32);
    }
}

// ── L1Block (L2 predeploy 0x4200000000000000000000000000000000000015) ──

sol! {
    #[sol(rpc)]
    interface L1Block {
        function baseFeeScalar() external view returns (uint32);
        function blobBaseFeeScalar() external view returns (uint32);
        function daFootprintGasScalar() external view returns (uint16);
    }
}

// ── FeeDisburser (L2) ──

sol! {
    #[sol(rpc)]
    interface FeeDisburser {
        event FeesDisbursed(
            uint256 _disbursementTime,
            uint256 _paidToOptimism,
            uint256 _totalFeesDisbursed
        );
    }
}

// ── BalanceTracker (L1) ──

sol! {
    #[sol(rpc)]
    interface BalanceTracker {
        event ReceivedFunds(address indexed _sender, uint256 _amount);
        event SentProfit(address indexed _profitWallet, bool indexed _success, uint256 _balanceSent);
    }
}
