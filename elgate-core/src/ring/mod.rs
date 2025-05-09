//! Shared memory ring buffer for inter-process communication.
//!
//! This module implements a lock-free, multi-producer, single-consumer
//! ring buffer located in `/dev/shm/elgate_ring` for high-performance
//! communication between processes.

mod buffer;
mod header;
mod slot;

pub use buffer::{RingBuffer, RingBufferOptions};
pub use header::RingHeader;
pub use slot::{Operation, OperationKind, Slot, SlotMetadata, SlotStatus};

use anyhow::{Context, Result};
use std::path::PathBuf;

/// Default path for the shared memory ring buffer.
pub const DEFAULT_RING_PATH: &str = "/dev/shm/elgate_ring";

/// Default size of the ring buffer in bytes.
pub const DEFAULT_RING_SIZE: usize = 4 * 1024 * 1024; // 4 MiB

/// Default size of a slot in the ring buffer.
pub const DEFAULT_SLOT_SIZE: usize = 4 * 1024; // 4 KiB

/// Creates a new ring buffer with default settings.
///
/// This is a convenience function that creates a ring buffer with
/// default settings at the default location.
pub fn create_ring_buffer() -> Result<RingBuffer> {
    let options = RingBufferOptions::default();
    RingBuffer::create(options).context("Failed to create ring buffer with default options")
}

/// Opens an existing ring buffer with default settings.
///
/// This is a convenience function that opens an existing ring buffer
/// at the default location.
pub fn open_ring_buffer() -> Result<RingBuffer> {
    let options = RingBufferOptions::default();
    RingBuffer::open(options).context("Failed to open ring buffer with default options")
}

/// Returns the default path for the ring buffer.
pub fn get_default_ring_path() -> PathBuf {
    PathBuf::from(DEFAULT_RING_PATH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_create_and_open_ring_buffer() {
        // Use a test-specific path to avoid conflicts
        let test_path = "/tmp/elgate_test_ring";

        // Clean up any previous test run
        let _ = fs::remove_file(test_path);

        // Create options with test path
        let options = RingBufferOptions {
            path: PathBuf::from(test_path),
            size: DEFAULT_RING_SIZE,
            slot_size: DEFAULT_SLOT_SIZE,
        };

        // Create a new ring buffer
        let ring = RingBuffer::create(options.clone()).unwrap();

        // Write some data
        let data = b"test data";
        ring.write(OperationKind::DiskWrite, data).unwrap();

        // Open the existing ring buffer
        let ring2 = RingBuffer::open(options).unwrap();

        // Read the data back
        let (op, read_data) = ring2.read().unwrap().unwrap();
        assert_eq!(op, OperationKind::DiskWrite);
        assert_eq!(read_data, data);

        // Clean up
        drop(ring);
        drop(ring2);
        let _ = fs::remove_file(test_path);
    }
}
