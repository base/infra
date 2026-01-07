//! Terminal output formatting helpers.

use std::{collections::HashMap, time::Duration};

// ANSI color codes
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const BLUE: &str = "\x1b[34m";
const MAGENTA: &str = "\x1b[35m";
const DIM: &str = "\x1b[2m";

/// Print the demo banner.
pub(crate) fn print_banner() {
    println!();
    println!("{CYAN}{BOLD}+=============================================+{RESET}");
    println!("{CYAN}{BOLD}|     Roxy Full Demo - JSON-RPC Proxy        |{RESET}");
    println!("{CYAN}{BOLD}+=============================================+{RESET}");
    println!();
}

/// Print a phase header.
pub(crate) fn print_phase(number: u32, total: u32, description: &str) {
    println!("{BOLD}[{number}/{total}] {description}...{RESET}");
}

/// Print a section header.
pub(crate) fn print_section(title: &str) {
    println!();
    println!("{CYAN}{BOLD}=== {title} ==={RESET}");
}

/// Print a success message with checkmark.
pub(crate) fn print_success(message: &str) {
    println!("  {GREEN}[OK]{RESET} {message}");
}

/// Print an info message.
pub(crate) fn print_info(message: &str) {
    println!("  {BLUE}[INFO]{RESET} {message}");
}

/// Print a warning message.
#[allow(dead_code)]
pub(crate) fn print_warning(message: &str) {
    println!("  {YELLOW}[WARN]{RESET} {message}");
}

/// Print an error message.
#[allow(dead_code)]
pub(crate) fn print_error(message: &str) {
    println!("  {RED}[ERROR]{RESET} {message}");
}

/// Print a node startup message.
pub(crate) fn print_node_started(name: &str, url: &str, latency_ms: u64) {
    let color = get_node_color(name);
    println!(
        "  {GREEN}[OK]{RESET} {color}{name}{RESET} at {url} {DIM}(latency: {latency_ms}ms){RESET}"
    );
}

/// Print which node served a request.
pub(crate) fn print_request_served(request_num: usize, node_name: &str, duration: Duration) {
    let color = get_node_color(node_name);
    let ms = duration.as_millis();
    println!("  Request {request_num}: served by {color}{node_name}{RESET} ({ms}ms)");
}

/// Print a cache hit/miss result.
pub(crate) fn print_cache_result(label: &str, duration: Duration, is_cached: bool) {
    let ms = duration.as_millis();
    let cache_label = if is_cached {
        format!("{GREEN}(cached){RESET}")
    } else {
        format!("{DIM}(backend){RESET}")
    };
    println!("  {label}: {ms}ms {cache_label}");
}

/// Print batch request results.
pub(crate) fn print_batch_result(method: &str, result: &str) {
    println!("  {DIM}[{method}]{RESET} -> {result}");
}

/// Print a failover action.
pub(crate) fn print_failover_action(action: &str) {
    println!("  {YELLOW}[ACTION]{RESET} {action}");
}

/// Print the distribution of requests across nodes.
pub(crate) fn print_distribution(counts: &HashMap<String, u64>) {
    println!();
    println!("  {BOLD}Distribution:{RESET}");

    let total: u64 = counts.values().sum();
    if total == 0 {
        println!("    No requests recorded");
        return;
    }

    // Sort by node name
    let mut sorted: Vec<_> = counts.iter().collect();
    sorted.sort_by_key(|(name, _)| *name);

    for (name, count) in sorted {
        let color = get_node_color(name);
        let percentage = (*count as f64 / total as f64) * 100.0;
        let bar_width = (percentage / 5.0) as usize; // 20 chars = 100%
        let bar: String = std::iter::repeat_n('█', bar_width).collect();
        println!("    {color}{name}{RESET}: {bar} ({count} requests, {percentage:.0}%)");
    }
}

/// Print the summary of node request counts.
pub(crate) fn print_summary(node_counts: &[(String, u64)]) {
    println!();
    println!("{BOLD}Summary:{RESET}");
    for (name, count) in node_counts {
        let color = get_node_color(name);
        println!("  {color}{name}{RESET}: {count} requests");
    }
}

/// Print demo complete message.
pub(crate) fn print_complete() {
    println!();
    println!("{GREEN}{BOLD}Demo complete!{RESET}");
    println!();
}

/// Get ANSI color code for a node name.
fn get_node_color(name: &str) -> &'static str {
    match name {
        n if n.contains('1') => RED,
        n if n.contains('2') => YELLOW,
        n if n.contains('3') => MAGENTA,
        _ => BLUE,
    }
}
