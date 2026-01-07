//! Mock Ethereum node implementation.
//!
//! This module provides a simulated Ethereum node that responds to JSON-RPC
//! requests with configurable latency, health status, and block progression.

mod handlers;
mod server;
mod state;

pub(crate) use server::{MockNode, MockNodeConfig};
