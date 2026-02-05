//! HTTP API server for sidecrushd health endpoints.

pub mod server;

pub use server::{AppState, create_router};
