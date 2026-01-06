//! RPC method router for directing requests to backend groups.
//!
//! This module provides a [`MethodRouter`] that routes JSON-RPC requests to different
//! backend groups based on the method name. It supports:
//!
//! - Exact method name matching
//! - Prefix-based matching (e.g., "debug_" matches all debug methods)
//! - Blocking specific methods
//! - Default fallback routing
//!
//! # Example
//!
//! ```
//! use roxy_rpc::{MethodRouter, RouteTarget};
//!
//! let router = MethodRouter::new()
//!     .route("eth_sendRawTransaction", RouteTarget::Group("sequencer".into()))
//!     .route_prefix("debug_", RouteTarget::Group("debug".into()))
//!     .route_prefix("admin_", RouteTarget::Block)
//!     .fallback(RouteTarget::Group("default".into()));
//!
//! // Exact match takes precedence
//! assert!(matches!(
//!     router.resolve("eth_sendRawTransaction"),
//!     RouteTarget::Group(g) if g == "sequencer"
//! ));
//!
//! // Prefix match for debug methods
//! assert!(matches!(
//!     router.resolve("debug_traceCall"),
//!     RouteTarget::Group(g) if g == "debug"
//! ));
//!
//! // Admin methods are blocked
//! assert!(router.is_blocked("admin_addPeer"));
//!
//! // Unmatched methods use the default
//! assert!(matches!(
//!     router.resolve("eth_blockNumber"),
//!     RouteTarget::Group(g) if g == "default"
//! ));
//! ```

use std::collections::HashMap;

use derive_more::Debug;

/// Route configuration for RPC methods.
///
/// Defines where an RPC method should be routed or if it should be blocked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteTarget {
    /// Route to a specific backend group by name.
    Group(String),
    /// Block this method (return error to client).
    Block,
    /// Use the default backend group.
    Default,
}

impl RouteTarget {
    /// Creates a new [`RouteTarget::Group`] with the given group name.
    ///
    /// # Example
    ///
    /// ```
    /// use roxy_rpc::RouteTarget;
    ///
    /// let target = RouteTarget::group("sequencer");
    /// assert!(matches!(target, RouteTarget::Group(g) if g == "sequencer"));
    /// ```
    pub fn group(name: impl Into<String>) -> Self {
        Self::Group(name.into())
    }

    /// Returns `true` if this target blocks the method.
    pub const fn is_blocked(&self) -> bool {
        matches!(self, Self::Block)
    }

    /// Returns `true` if this target uses the default backend.
    pub const fn is_default(&self) -> bool {
        matches!(self, Self::Default)
    }

    /// Returns the group name if this is a [`RouteTarget::Group`].
    pub fn group_name(&self) -> Option<&str> {
        match self {
            Self::Group(name) => Some(name),
            _ => None,
        }
    }
}

/// RPC method router that directs requests to backend groups.
///
/// The router uses a priority-based matching system:
/// 1. Exact method name matches (highest priority)
/// 2. Prefix matches (checked in order of registration, longer prefixes first)
/// 3. Default target (fallback)
///
/// # Example
///
/// ```
/// use roxy_rpc::{MethodRouter, RouteTarget};
///
/// let router = MethodRouter::new()
///     .route("eth_call", RouteTarget::group("archive"))
///     .route_prefix("eth_", RouteTarget::group("default"))
///     .route_prefix("debug_", RouteTarget::group("debug"))
///     .fallback(RouteTarget::group("fallback"));
///
/// // eth_call has an exact match, so it goes to "archive"
/// assert_eq!(
///     router.resolve("eth_call").group_name(),
///     Some("archive")
/// );
///
/// // eth_blockNumber matches the "eth_" prefix
/// assert_eq!(
///     router.resolve("eth_blockNumber").group_name(),
///     Some("default")
/// );
/// ```
#[derive(Debug)]
pub struct MethodRouter {
    /// Exact method name -> target mappings.
    exact_routes: HashMap<String, RouteTarget>,
    /// Prefix -> target mappings (e.g., "debug_" -> Group("debug")).
    /// Sorted by prefix length (longest first) for correct matching.
    prefix_routes: Vec<(String, RouteTarget)>,
    /// Default target for unmatched methods.
    default_target: RouteTarget,
}

impl MethodRouter {
    /// Creates a new [`MethodRouter`] with [`RouteTarget::Default`] as the default target.
    ///
    /// # Example
    ///
    /// ```
    /// use roxy_rpc::MethodRouter;
    ///
    /// let router = MethodRouter::new();
    /// assert!(router.resolve("any_method").is_default());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            exact_routes: HashMap::new(),
            prefix_routes: Vec::new(),
            default_target: RouteTarget::Default,
        }
    }

    /// Adds an exact method route.
    ///
    /// Exact routes take precedence over prefix routes and the default target.
    ///
    /// # Example
    ///
    /// ```
    /// use roxy_rpc::{MethodRouter, RouteTarget};
    ///
    /// let router = MethodRouter::new()
    ///     .route("eth_sendRawTransaction", RouteTarget::group("sequencer"));
    ///
    /// assert_eq!(
    ///     router.resolve("eth_sendRawTransaction").group_name(),
    ///     Some("sequencer")
    /// );
    /// ```
    #[must_use]
    pub fn route(mut self, method: &str, target: RouteTarget) -> Self {
        self.exact_routes.insert(method.to_owned(), target);
        self
    }

    /// Adds a prefix route.
    ///
    /// Prefix routes match any method that starts with the given prefix.
    /// When multiple prefixes match, the longest prefix wins.
    /// Exact routes always take precedence over prefix routes.
    ///
    /// # Example
    ///
    /// ```
    /// use roxy_rpc::{MethodRouter, RouteTarget};
    ///
    /// let router = MethodRouter::new()
    ///     .route_prefix("debug_", RouteTarget::group("debug"))
    ///     .route_prefix("debug_trace", RouteTarget::group("trace"));
    ///
    /// // "debug_traceCall" matches both prefixes, but "debug_trace" is longer
    /// assert_eq!(
    ///     router.resolve("debug_traceCall").group_name(),
    ///     Some("trace")
    /// );
    ///
    /// // "debug_getRawBlock" only matches "debug_"
    /// assert_eq!(
    ///     router.resolve("debug_getRawBlock").group_name(),
    ///     Some("debug")
    /// );
    /// ```
    #[must_use]
    pub fn route_prefix(mut self, prefix: &str, target: RouteTarget) -> Self {
        self.prefix_routes.push((prefix.to_owned(), target));
        // Sort by prefix length descending so longer prefixes are checked first
        self.prefix_routes.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        self
    }

    /// Sets the fallback target for methods that don't match any route.
    ///
    /// # Example
    ///
    /// ```
    /// use roxy_rpc::{MethodRouter, RouteTarget};
    ///
    /// let router = MethodRouter::new()
    ///     .fallback(RouteTarget::group("fallback"));
    ///
    /// assert_eq!(
    ///     router.resolve("unknown_method").group_name(),
    ///     Some("fallback")
    /// );
    /// ```
    #[must_use]
    pub fn fallback(mut self, target: RouteTarget) -> Self {
        self.default_target = target;
        self
    }

    /// Resolves which target a method should route to.
    ///
    /// Resolution priority:
    /// 1. Exact method name match
    /// 2. Longest matching prefix
    /// 3. Default target
    ///
    /// # Example
    ///
    /// ```
    /// use roxy_rpc::{MethodRouter, RouteTarget};
    ///
    /// let router = MethodRouter::new()
    ///     .route("eth_call", RouteTarget::group("archive"))
    ///     .route_prefix("eth_", RouteTarget::group("default"))
    ///     .fallback(RouteTarget::group("fallback"));
    ///
    /// // Exact match
    /// assert_eq!(router.resolve("eth_call").group_name(), Some("archive"));
    ///
    /// // Prefix match
    /// assert_eq!(router.resolve("eth_blockNumber").group_name(), Some("default"));
    ///
    /// // Default fallback
    /// assert_eq!(router.resolve("net_version").group_name(), Some("fallback"));
    /// ```
    pub fn resolve(&self, method: &str) -> &RouteTarget {
        // Check exact routes first
        if let Some(target) = self.exact_routes.get(method) {
            return target;
        }

        // Check prefix routes (already sorted by length, longest first)
        for (prefix, target) in &self.prefix_routes {
            if method.starts_with(prefix) {
                return target;
            }
        }

        // Return default target
        &self.default_target
    }

    /// Checks if a method is blocked.
    ///
    /// Returns `true` if the resolved target for this method is [`RouteTarget::Block`].
    ///
    /// # Example
    ///
    /// ```
    /// use roxy_rpc::{MethodRouter, RouteTarget};
    ///
    /// let router = MethodRouter::new()
    ///     .route("admin_addPeer", RouteTarget::Block)
    ///     .route_prefix("debug_", RouteTarget::Block);
    ///
    /// assert!(router.is_blocked("admin_addPeer"));
    /// assert!(router.is_blocked("debug_traceCall"));
    /// assert!(!router.is_blocked("eth_call"));
    /// ```
    pub fn is_blocked(&self, method: &str) -> bool {
        self.resolve(method).is_blocked()
    }

    /// Returns the number of exact routes configured.
    pub fn exact_route_count(&self) -> usize {
        self.exact_routes.len()
    }

    /// Returns the number of prefix routes configured.
    pub const fn prefix_route_count(&self) -> usize {
        self.prefix_routes.len()
    }

    /// Returns a reference to the default target.
    pub const fn default_target(&self) -> &RouteTarget {
        &self.default_target
    }
}

impl Default for MethodRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[test]
    fn test_route_target_group() {
        let target = RouteTarget::group("test");
        assert_eq!(target, RouteTarget::Group("test".to_owned()));
        assert_eq!(target.group_name(), Some("test"));
        assert!(!target.is_blocked());
        assert!(!target.is_default());
    }

    #[test]
    fn test_route_target_block() {
        let target = RouteTarget::Block;
        assert!(target.is_blocked());
        assert!(!target.is_default());
        assert_eq!(target.group_name(), None);
    }

    #[test]
    fn test_route_target_default() {
        let target = RouteTarget::Default;
        assert!(target.is_default());
        assert!(!target.is_blocked());
        assert_eq!(target.group_name(), None);
    }

    #[test]
    fn test_method_router_new() {
        let router = MethodRouter::new();
        assert_eq!(router.exact_route_count(), 0);
        assert_eq!(router.prefix_route_count(), 0);
        assert!(router.default_target().is_default());
    }

    #[test]
    fn test_method_router_default_impl() {
        let router = MethodRouter::default();
        assert!(router.resolve("any_method").is_default());
    }

    #[rstest]
    #[case("eth_call", "archive")]
    #[case("eth_sendRawTransaction", "sequencer")]
    #[case("debug_traceCall", "debug")]
    fn test_exact_routes(#[case] method: &str, #[case] expected_group: &str) {
        let router = MethodRouter::new()
            .route("eth_call", RouteTarget::group("archive"))
            .route("eth_sendRawTransaction", RouteTarget::group("sequencer"))
            .route("debug_traceCall", RouteTarget::group("debug"));

        assert_eq!(router.resolve(method).group_name(), Some(expected_group));
    }

    #[rstest]
    #[case("eth_blockNumber", "eth")]
    #[case("eth_getBalance", "eth")]
    #[case("debug_getRawBlock", "debug")]
    #[case("debug_traceTransaction", "debug")]
    #[case("net_version", "net")]
    fn test_prefix_routes(#[case] method: &str, #[case] expected_group: &str) {
        let router = MethodRouter::new()
            .route_prefix("eth_", RouteTarget::group("eth"))
            .route_prefix("debug_", RouteTarget::group("debug"))
            .route_prefix("net_", RouteTarget::group("net"));

        assert_eq!(router.resolve(method).group_name(), Some(expected_group));
    }

    #[test]
    fn test_exact_route_takes_precedence_over_prefix() {
        let router = MethodRouter::new()
            .route("eth_call", RouteTarget::group("archive"))
            .route_prefix("eth_", RouteTarget::group("default"));

        // eth_call has an exact match, should go to "archive"
        assert_eq!(router.resolve("eth_call").group_name(), Some("archive"));

        // eth_blockNumber only matches prefix, should go to "default"
        assert_eq!(router.resolve("eth_blockNumber").group_name(), Some("default"));
    }

    #[test]
    fn test_longer_prefix_takes_precedence() {
        let router = MethodRouter::new()
            .route_prefix("debug_", RouteTarget::group("debug"))
            .route_prefix("debug_trace", RouteTarget::group("trace"));

        // debug_traceCall matches both, but "debug_trace" is longer
        assert_eq!(router.resolve("debug_traceCall").group_name(), Some("trace"));

        // debug_getRawBlock only matches "debug_"
        assert_eq!(router.resolve("debug_getRawBlock").group_name(), Some("debug"));
    }

    #[test]
    fn test_prefix_routes_sorted_by_length() {
        // Add shorter prefix first to ensure sorting works
        let router = MethodRouter::new()
            .route_prefix("d", RouteTarget::group("d"))
            .route_prefix("debug_trace", RouteTarget::group("trace"))
            .route_prefix("debug_", RouteTarget::group("debug"));

        // Should match "debug_trace" (longest match)
        assert_eq!(router.resolve("debug_traceCall").group_name(), Some("trace"));

        // Should match "debug_" (second longest)
        assert_eq!(router.resolve("debug_getRawBlock").group_name(), Some("debug"));

        // Should match "d" (shortest)
        assert_eq!(router.resolve("data_sync").group_name(), Some("d"));
    }

    #[test]
    fn test_default_target() {
        let router = MethodRouter::new()
            .route_prefix("eth_", RouteTarget::group("eth"))
            .fallback(RouteTarget::group("fallback"));

        // Methods that don't match any route use default
        assert_eq!(router.resolve("web3_clientVersion").group_name(), Some("fallback"));
        assert_eq!(router.resolve("unknown_method").group_name(), Some("fallback"));
    }

    #[rstest]
    #[case("admin_addPeer")]
    #[case("admin_removePeer")]
    #[case("admin_nodeInfo")]
    fn test_blocked_methods_via_exact_route(#[case] method: &str) {
        let router = MethodRouter::new()
            .route("admin_addPeer", RouteTarget::Block)
            .route("admin_removePeer", RouteTarget::Block)
            .route("admin_nodeInfo", RouteTarget::Block);

        assert!(router.is_blocked(method));
    }

    #[rstest]
    #[case("admin_addPeer")]
    #[case("admin_removePeer")]
    #[case("admin_nodeInfo")]
    fn test_blocked_methods_via_prefix_route(#[case] method: &str) {
        let router = MethodRouter::new().route_prefix("admin_", RouteTarget::Block);

        assert!(router.is_blocked(method));
    }

    #[test]
    fn test_non_blocked_methods() {
        let router = MethodRouter::new()
            .route_prefix("admin_", RouteTarget::Block)
            .route_prefix("eth_", RouteTarget::group("eth"));

        assert!(!router.is_blocked("eth_call"));
        assert!(!router.is_blocked("eth_blockNumber"));
        assert!(!router.is_blocked("debug_traceCall"));
    }

    #[test]
    fn test_complex_routing_scenario() {
        let router = MethodRouter::new()
            // Exact routes for high-priority methods
            .route("eth_sendRawTransaction", RouteTarget::group("sequencer"))
            .route("eth_call", RouteTarget::group("archive"))
            .route("eth_estimateGas", RouteTarget::group("archive"))
            // Prefix routes for method families
            .route_prefix("debug_trace", RouteTarget::group("trace"))
            .route_prefix("debug_", RouteTarget::group("debug"))
            .route_prefix("admin_", RouteTarget::Block)
            .route_prefix("eth_", RouteTarget::group("default"))
            // Fallback
            .fallback(RouteTarget::group("fallback"));

        // Exact matches
        assert_eq!(router.resolve("eth_sendRawTransaction").group_name(), Some("sequencer"));
        assert_eq!(router.resolve("eth_call").group_name(), Some("archive"));
        assert_eq!(router.resolve("eth_estimateGas").group_name(), Some("archive"));

        // Prefix matches with priority
        assert_eq!(router.resolve("debug_traceCall").group_name(), Some("trace"));
        assert_eq!(router.resolve("debug_traceTransaction").group_name(), Some("trace"));
        assert_eq!(router.resolve("debug_getRawBlock").group_name(), Some("debug"));

        // Blocked methods
        assert!(router.is_blocked("admin_addPeer"));
        assert!(router.is_blocked("admin_nodeInfo"));

        // Default eth_ prefix
        assert_eq!(router.resolve("eth_blockNumber").group_name(), Some("default"));
        assert_eq!(router.resolve("eth_getBalance").group_name(), Some("default"));

        // Fallback
        assert_eq!(router.resolve("web3_clientVersion").group_name(), Some("fallback"));
        assert_eq!(router.resolve("net_version").group_name(), Some("fallback"));
    }

    #[test]
    fn test_route_count() {
        let router = MethodRouter::new()
            .route("eth_call", RouteTarget::group("archive"))
            .route("eth_sendRawTransaction", RouteTarget::group("sequencer"))
            .route_prefix("debug_", RouteTarget::group("debug"))
            .route_prefix("admin_", RouteTarget::Block);

        assert_eq!(router.exact_route_count(), 2);
        assert_eq!(router.prefix_route_count(), 2);
    }

    #[test]
    fn test_empty_method_name() {
        let router = MethodRouter::new()
            .route_prefix("eth_", RouteTarget::group("eth"))
            .fallback(RouteTarget::group("fallback"));

        // Empty string should use default
        assert_eq!(router.resolve("").group_name(), Some("fallback"));
    }

    #[test]
    fn test_empty_prefix_matches_all() {
        let router = MethodRouter::new()
            .route_prefix("", RouteTarget::group("catch-all"))
            .fallback(RouteTarget::group("fallback"));

        // Empty prefix matches everything
        assert_eq!(router.resolve("eth_call").group_name(), Some("catch-all"));
        assert_eq!(router.resolve("anything").group_name(), Some("catch-all"));
    }

    #[rstest]
    #[case("eth_call")]
    #[case("ETH_CALL")]
    #[case("Eth_Call")]
    fn test_case_sensitivity(#[case] method: &str) {
        let router = MethodRouter::new()
            .route("eth_call", RouteTarget::group("exact"))
            .route_prefix("eth_", RouteTarget::group("prefix"))
            .fallback(RouteTarget::group("default"));

        // Only exact match for "eth_call", others go to default
        let expected = if method == "eth_call" { "exact" } else { "default" };
        assert_eq!(router.resolve(method).group_name(), Some(expected));
    }

    #[test]
    fn test_route_target_clone() {
        let target = RouteTarget::group("test");
        let cloned = target.clone();
        assert_eq!(target, cloned);
    }

    #[test]
    fn test_route_target_debug() {
        let target = RouteTarget::group("test");
        let debug_str = format!("{:?}", target);
        assert!(debug_str.contains("Group"));
        assert!(debug_str.contains("test"));
    }

    #[test]
    fn test_method_router_debug() {
        let router = MethodRouter::new()
            .route("eth_call", RouteTarget::group("test"))
            .route_prefix("debug_", RouteTarget::Block);

        let debug_str = format!("{:?}", router);
        assert!(debug_str.contains("MethodRouter"));
    }
}
