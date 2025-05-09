//! Disk I/O engine.
//!
//! This module provides disk I/O operations using either io_uring (if available)
//! or a standard fallback implementation.

#[cfg(feature = "io_uring")]
pub mod io_uring;

#[cfg(not(feature = "io_uring"))]
pub mod fallback;

// Re-export based on feature flags for a unified API
#[cfg(feature = "io_uring")]
pub use io_uring::{DiskConfig, DiskEngine};

#[cfg(not(feature = "io_uring"))]
pub use fallback::{DiskConfig, DiskEngine};

// Add integration tests for both implementations

#[cfg(test)]
mod integration_tests {
    // Integration tests that work with either implementation
}
