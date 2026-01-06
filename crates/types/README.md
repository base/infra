# roxy-types

Core types for the Roxy RPC proxy.

## Overview

This crate re-exports well-tested types from the [alloy](https://github.com/alloy-rs/alloy) ecosystem and provides custom error types for the proxy.

## Re-exported Types

### From `alloy-json-rpc`

- `Request<Params>` - JSON-RPC 2.0 request
- `Response<Payload, ErrData>` - JSON-RPC 2.0 response
- `Id` - Request identifier (Number, String, or None)
- `ErrorPayload` - Error object with code, message, and optional data
- `RequestPacket` / `ResponsePacket` - Single or batch requests/responses

### From `alloy-primitives`

- `BlockNumber` - Block height (u64)
- `BlockHash` - Block hash (B256)
- `Address` - Ethereum address (20 bytes)
- `B256` - 32-byte fixed array

## Custom Types

- `RoxyError` - Proxy-specific error enum with rate limiting, backend, and cache errors
