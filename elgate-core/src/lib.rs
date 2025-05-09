//! Elgate Core - High-performance I/O routing system
//!
//! This library provides the core functionality for the Elgate system.

/// Architecture detection and thread management
pub mod arch;

/// Shared memory ring buffer
pub mod ring;

/// Write-ahead logging
pub mod wal {
    // To be implemented
}

/// Disk I/O engine
pub mod disk {
    #[cfg(feature = "io_uring")]
    pub mod io_uring;

    #[cfg(not(feature = "io_uring"))]
    pub mod fallback {
        // Standard file I/O fallback (cross-platform)
    }
}

/// Network I/O engine
pub mod net {
    #[cfg(feature = "io_uring")]
    pub mod io_uring;

    #[cfg(not(feature = "io_uring"))]
    pub mod fallback {
        // Standard socket I/O fallback (cross-platform)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
