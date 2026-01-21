//! HTTP API server for sidecrushd health endpoints.

pub mod server;

pub use server::{create_router, AppState};
