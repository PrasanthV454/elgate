//! Header for the shared memory ring buffer.
//!
//! This module defines the header structure for the ring buffer,
//! containing metadata and atomic counters for producers and consumers.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Size of the header padding in bytes.
/// This ensures proper alignment and cache line separation.
pub const HEADER_PADDING_SIZE: usize = 64;

/// Ring buffer header structure.
///
/// This structure is stored at the beginning of the shared memory region
/// and contains metadata and atomic counters for the ring buffer.
#[repr(C, align(64))]
pub struct RingHeader {
    /// Magic value to identify a valid ring buffer.
    pub magic: AtomicU64,
    
    /// Version of the ring buffer format.
    pub version: AtomicU64,
    
    /// Size of the entire ring buffer in bytes.
    pub size: AtomicUsize,
    
    /// Size of each slot in bytes.
    pub slot_size: AtomicUsize,
    
    /// Number of slots in the ring buffer.
    pub slot_count: AtomicUsize,
    
    /// Producer index (next slot to be written).
    pub producer_idx: AtomicUsize,
    
    /// Consumer index (next slot to be read).
    pub consumer_idx: AtomicUsize,
    
    /// Count of active producers.
    pub active_producers: AtomicUsize,
    
    /// Count of operations completed.
    pub operations_completed: AtomicUsize,
    
    /// Creation timestamp (seconds since epoch).
    pub created_at: AtomicU64,
    
    /// Padding to ensure the header is cache-aligned.
    /// This prevents false sharing between the header and the first slot.
    pub _padding: [u8; HEADER_PADDING_SIZE],
}

impl RingHeader {
    /// Magic value to identify a valid ring buffer.
    pub const MAGIC: u64 = 0x45_4C_47_41_54_45_52_42; // "ELGATERB" in hex
    
    /// Current version of the ring buffer format.
    pub const VERSION: u64 = 1;
    
    /// Creates a new header with the given parameters.
    pub fn new(size: usize, slot_size: usize) -> Self {
        let slot_count = (size - std::mem::size_of::<Self>()) / slot_size;
        
        Self {
            magic: AtomicU64::new(Self::MAGIC),
            version: AtomicU64::new(Self::VERSION),
            size: AtomicUsize::new(size),
            slot_size: AtomicUsize::new(slot_size),
            slot_count: AtomicUsize::new(slot_count),
            producer_idx: AtomicUsize::new(0),
            consumer_idx: AtomicUsize::new(0),
            active_producers: AtomicUsize::new(0),
            operations_completed: AtomicUsize::new(0),
            created_at: AtomicU64::new(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            ),
            _padding: [0; HEADER_PADDING_SIZE],
        }
    }
    
    /// Validates the header to ensure it's a valid ring buffer.
    pub fn validate(&self) -> bool {
        self.magic.load(Ordering::Relaxed) == Self::MAGIC &&
        self.version.load(Ordering::Relaxed) == Self::VERSION &&
        self.size.load(Ordering::Relaxed) > std::mem::size_of::<Self>() &&
        self.slot_size.load(Ordering::Relaxed) > 0 &&
        self.slot_count.load(Ordering::Relaxed) > 0
    }
    
    /// Gets the next producer index using compare-and-swap.
    pub fn next_producer_idx(&self) -> usize {
        let mut current = self.producer_idx.load(Ordering::Relaxed);
        let slot_count = self.slot_count.load(Ordering::Relaxed);
        
        loop {
            let next = (current + 1) % slot_count;
            match self.producer_idx.compare_exchange_weak(
                current,
                next,
                Ordering::SeqCst,
                Ordering::Relaxed,
            ) {
                Ok(_) => return current,
                Err(actual) => current = actual,
            }
        }
    }
    
    /// Gets the next consumer index using compare-and-swap.
    pub fn next_consumer_idx(&self) -> usize {
        let mut current = self.consumer_idx.load(Ordering::Relaxed);
        let slot_count = self.slot_count.load(Ordering::Relaxed);
        
        loop {
            let next = (current + 1) % slot_count;
            match self.consumer_idx.compare_exchange_weak(
                current,
                next,
                Ordering::SeqCst,
                Ordering::Relaxed,
            ) {
                Ok(_) => return current,
                Err(actual) => current = actual,
            }
        }
    }
    
    /// Increments the count of completed operations.
    pub fn increment_completed(&self) {
        self.operations_completed.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Registers a new producer.
    pub fn register_producer(&self) {
        self.active_producers.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Unregisters a producer.
    pub fn unregister_producer(&self) {
        self.active_producers.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_header_creation() {
        let size = 1024 * 1024;
        let slot_size = 4096;
        let header = RingHeader::new(size, slot_size);
        
        assert_eq!(header.magic.load(Ordering::Relaxed), RingHeader::MAGIC);
        assert_eq!(header.version.load(Ordering::Relaxed), RingHeader::VERSION);
        assert_eq!(header.size.load(Ordering::Relaxed), size);
        assert_eq!(header.slot_size.load(Ordering::Relaxed), slot_size);
        
        let expected_slot_count = (size - std::mem::size_of::<RingHeader>()) / slot_size;
        assert_eq!(header.slot_count.load(Ordering::Relaxed), expected_slot_count);
        
        assert!(header.validate());
    }
    
    #[test]
    fn test_producer_consumer_idx() {
        let header = RingHeader::new(1024 * 1024, 4096);
        let slot_count = header.slot_count.load(Ordering::Relaxed);
        
        // Test producer index increment
        for i in 0..slot_count * 2 {
            let idx = header.next_producer_idx();
            assert_eq!(idx, i % slot_count);
        }
        
        // Test consumer index increment
        for i in 0..slot_count * 2 {
            let idx = header.next_consumer_idx();
            assert_eq!(idx, i % slot_count);
        }
    }
}
