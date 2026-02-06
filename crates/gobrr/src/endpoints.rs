//! Multi-endpoint management for load distribution across multiple proxyd instances.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rand::Rng;

/// Distribution strategy for selecting endpoints
#[derive(Debug, Clone, Copy, Default)]
pub enum EndpointDistribution {
    /// Round-robin selection across endpoints
    #[default]
    RoundRobin,
    /// Random selection
    Random,
    /// Weighted selection based on endpoint weights
    Weighted,
}

impl std::str::FromStr for EndpointDistribution {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "round-robin" | "roundrobin" | "rr" => Ok(Self::RoundRobin),
            "random" | "rand" => Ok(Self::Random),
            "weighted" | "weight" => Ok(Self::Weighted),
            _ => Err(format!("Unknown distribution strategy: {s}")),
        }
    }
}

/// A single endpoint configuration
#[derive(Debug, Clone)]
pub struct Endpoint {
    /// The RPC URL
    pub url: String,
    /// Weight for weighted distribution (higher = more traffic)
    pub weight: u32,
    /// Optional name/label for the endpoint
    pub name: Option<String>,
}

impl Endpoint {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            weight: 1,
            name: None,
        }
    }

    pub fn with_weight(mut self, weight: u32) -> Self {
        self.weight = weight;
        self
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
}

/// Pool of endpoints with load distribution
#[derive(Debug)]
pub struct EndpointPool {
    endpoints: Vec<Endpoint>,
    distribution: EndpointDistribution,
    /// Counter for round-robin distribution
    counter: AtomicUsize,
    /// Cumulative weights for weighted distribution
    cumulative_weights: Vec<u32>,
    /// Total weight sum
    total_weight: u32,
}

impl EndpointPool {
    /// Creates a new endpoint pool from a list of endpoints
    pub fn new(endpoints: Vec<Endpoint>, distribution: EndpointDistribution) -> Self {
        assert!(!endpoints.is_empty(), "EndpointPool requires at least one endpoint");

        // Build cumulative weights for weighted selection
        let mut cumulative_weights = Vec::with_capacity(endpoints.len());
        let mut sum = 0u32;
        for ep in &endpoints {
            sum += ep.weight;
            cumulative_weights.push(sum);
        }

        Self {
            endpoints,
            distribution,
            counter: AtomicUsize::new(0),
            cumulative_weights,
            total_weight: sum,
        }
    }

    /// Creates a pool from a comma-separated list of URLs
    pub fn from_urls(urls: &str, distribution: EndpointDistribution) -> Self {
        let endpoints: Vec<Endpoint> = urls
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .enumerate()
            .map(|(i, url)| Endpoint::new(url).with_name(format!("endpoint-{i}")))
            .collect();

        Self::new(endpoints, distribution)
    }

    /// Creates a pool with a single endpoint
    pub fn single(url: impl Into<String>) -> Self {
        Self::new(vec![Endpoint::new(url)], EndpointDistribution::RoundRobin)
    }

    /// Returns the number of endpoints
    pub fn len(&self) -> usize {
        self.endpoints.len()
    }

    /// Returns true if there's only one endpoint
    pub fn is_single(&self) -> bool {
        self.endpoints.len() == 1
    }

    /// Returns true if pool is empty (should never happen after construction)
    pub fn is_empty(&self) -> bool {
        self.endpoints.is_empty()
    }

    /// Selects the next endpoint based on the distribution strategy
    pub fn select(&self) -> &Endpoint {
        match self.distribution {
            EndpointDistribution::RoundRobin => self.select_round_robin(),
            EndpointDistribution::Random => self.select_random(),
            EndpointDistribution::Weighted => self.select_weighted(),
        }
    }

    /// Returns the URL of the next selected endpoint
    pub fn select_url(&self) -> &str {
        &self.select().url
    }

    /// Round-robin selection
    fn select_round_robin(&self) -> &Endpoint {
        let idx = self.counter.fetch_add(1, Ordering::Relaxed) % self.endpoints.len();
        &self.endpoints[idx]
    }

    /// Random selection
    fn select_random(&self) -> &Endpoint {
        let idx = rand::thread_rng().gen_range(0..self.endpoints.len());
        &self.endpoints[idx]
    }

    /// Weighted random selection
    fn select_weighted(&self) -> &Endpoint {
        let mut rng = rand::thread_rng();
        let target = rng.gen_range(1..=self.total_weight);

        // Binary search for the endpoint
        let idx = self
            .cumulative_weights
            .binary_search_by(|&w| {
                if w < target {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                }
            })
            .unwrap_or_else(|i| i);

        &self.endpoints[idx]
    }

    /// Returns all endpoint URLs
    pub fn urls(&self) -> Vec<&str> {
        self.endpoints.iter().map(|e| e.url.as_str()).collect()
    }

    /// Returns iterator over all endpoints
    pub fn iter(&self) -> impl Iterator<Item = &Endpoint> {
        self.endpoints.iter()
    }
}

/// Thread-safe wrapper for sharing endpoint pool across tasks
pub type SharedEndpointPool = Arc<EndpointPool>;

/// Creates a shared endpoint pool
#[allow(dead_code)]
pub(crate) fn create_shared_pool(endpoints: Vec<Endpoint>, distribution: EndpointDistribution) -> SharedEndpointPool {
    Arc::new(EndpointPool::new(endpoints, distribution))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_endpoint() {
        let pool = EndpointPool::single("http://localhost:8545");
        assert_eq!(pool.len(), 1);
        assert!(pool.is_single());
        assert_eq!(pool.select_url(), "http://localhost:8545");
    }

    #[test]
    fn test_from_urls() {
        let pool = EndpointPool::from_urls(
            "http://a.com, http://b.com, http://c.com",
            EndpointDistribution::RoundRobin,
        );
        assert_eq!(pool.len(), 3);
    }

    #[test]
    fn test_round_robin() {
        let pool = EndpointPool::from_urls(
            "http://a.com,http://b.com",
            EndpointDistribution::RoundRobin,
        );

        assert_eq!(pool.select_url(), "http://a.com");
        assert_eq!(pool.select_url(), "http://b.com");
        assert_eq!(pool.select_url(), "http://a.com");
    }
}
