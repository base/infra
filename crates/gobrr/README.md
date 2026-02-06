# gobrr

Ethereum/OP Stack load tester that derives wallets from a mnemonic, funds them, and spams RPC endpoints with transactions.

## Build

```bash
cargo build --release -p gobrr-bin
```

## Usage

```bash
./target/release/gobrr \
  --rpc "https://sepolia.base.org" \
  --mnemonic "your twelve word mnemonic phrase here" \
  --funder-key "0xYOUR_PRIVATE_KEY" \
  --funding-amount 10000000000000000 \
  --sender-count 10 \
  --duration 5m
```

## Key Options

| Option | Default | Description |
|--------|---------|-------------|
| `--rpc` | - | Single RPC endpoint |
| `--rpc-endpoints` | - | Comma-separated endpoints for load distribution |
| `--endpoint-distribution` | `round-robin` | `round-robin`, `random`, or `weighted` |
| `--network` | `custom` | `sepolia`, `sepolia-alpha`, or `custom` |
| `--mnemonic` | - | HD wallet mnemonic for sender accounts |
| `--funder-key` | - | Private key with ETH to fund senders |
| `--funding-amount` | - | Wei to fund each sender |
| `--sender-count` | `10` | Number of concurrent senders |
| `--in-flight-per-sender` | `16` | Max concurrent requests per sender |
| `--rpc-methods` | `eth_sendRawTransaction` | Comma-separated RPC methods |
| `--tx-percentage` | `80` | Percentage of requests that are transactions |
| `--replay-mode` | `none` | `none`, `same-tx`, or `same-method` |
| `--replay-percentage` | `0` | Percentage of requests that are replays |
| `--duration` | - | Test duration (e.g., `60s`, `5m`). Omit to run until Ctrl+C |

## Examples

```bash
# Multi-endpoint load test
./target/release/gobrr \
  --rpc-endpoints "https://rpc1.com,https://rpc2.com" \
  --endpoint-distribution round-robin \
  --mnemonic "..." --funder-key "0x..." --funding-amount 10000000000000000

# Mixed read/write load
./target/release/gobrr \
  --rpc "https://sepolia.base.org" \
  --rpc-methods "eth_sendRawTransaction,eth_getBalance,eth_blockNumber" \
  --tx-percentage 50 \
  --mnemonic "..." --funder-key "0x..." --funding-amount 10000000000000000

# Replay mode (test caching/deduplication)
./target/release/gobrr \
  --rpc "https://sepolia.base.org" \
  --replay-mode same-tx --replay-percentage 20 \
  --mnemonic "..." --funder-key "0x..." --funding-amount 10000000000000000
```
