//! Ring buffer implementation using shared memory.
//!
//! This module implements a lock-free, multi-producer, single-consumer
//! ring buffer using shared memory for high-performance IPC.

use crate::ring::{
    header::RingHeader,
    slot::{OperationKind, Slot, SlotStatus},
    DEFAULT_RING_PATH, DEFAULT_RING_SIZE, DEFAULT_SLOT_SIZE,
};

use crate::arch::cpu_info::CpuInfo;
use anyhow::{Context, Result};
use memmap2::{MmapMut, MmapOptions};
use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;
use uuid::Uuid;

/// Options for creating or opening a ring buffer.
#[derive(Debug, Clone)]
pub struct RingBufferOptions {
    /// Path to the shared memory file.
    pub path: PathBuf,

    /// Total size of the ring buffer in bytes.
    pub size: usize,

    /// Size of each slot in bytes.
    pub slot_size: usize,
}

impl Default for RingBufferOptions {
    fn default() -> Self {
        Self {
            path: PathBuf::from(DEFAULT_RING_PATH),
            size: DEFAULT_RING_SIZE,
            slot_size: DEFAULT_SLOT_SIZE,
        }
    }
}

/// A shared memory ring buffer for inter-process communication.
pub struct RingBuffer {
    /// Memory-mapped region for the ring buffer.
    mmap: MmapMut,

    /// Header of the ring buffer.
    header: *mut RingHeader,

    /// Path to the shared memory file.
    path: PathBuf,

    /// Size of the ring buffer in bytes.
    size: usize,

    /// Size of each slot in bytes.
    slot_size: usize,

    /// Whether this instance is a producer.
    is_producer: bool,
}

// Safety: The RingBuffer can be safely sent between threads
// because access to the shared memory is synchronized using
// atomic operations in the header and slots.
unsafe impl Send for RingBuffer {}
unsafe impl Sync for RingBuffer {}

impl RingBuffer {
    /// Creates a new ring buffer.
    ///
    /// This creates a new shared memory region for the ring buffer
    /// and initializes the header and slots.
    pub fn create(options: RingBufferOptions) -> Result<Self> {
        // Create the shared memory file
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o666) // Readable/writable by all users
            .open(&options.path)
            .context("Failed to create/open shared memory file")?;

        // Resize the file to the desired size
        file.set_len(options.size as u64)
            .context("Failed to resize shared memory file")?;

        // Memory-map the file
        let mut mmap = unsafe {
            MmapOptions::new()
                .map_mut(&file)
                .context("Failed to memory-map shared memory file")?
        };

        // Initialize the header
        let header_ptr = mmap.as_mut_ptr() as *mut RingHeader;
        unsafe {
            std::ptr::write(header_ptr, RingHeader::new(options.size, options.slot_size));

            // Register as a producer
            (*header_ptr).register_producer();
        }

        // Initialize slots
        let slot_count = unsafe { (*header_ptr).slot_count.load(Ordering::Relaxed) };
        for i in 0..slot_count {
            let slot = unsafe { Self::get_slot(&mmap, header_ptr, i, options.slot_size) };
            slot.reset();
        }

        Ok(Self {
            mmap,
            header: header_ptr,
            path: options.path,
            size: options.size,
            slot_size: options.slot_size,
            is_producer: true,
        })
    }

    /// Opens an existing ring buffer.
    ///
    /// This opens an existing shared memory region for the ring buffer
    /// and validates the header.
    pub fn open(options: RingBufferOptions) -> Result<Self> {
        // Open the shared memory file
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&options.path)
            .context("Failed to open shared memory file")?;

        // Memory-map the file
        let mmap = unsafe {
            MmapOptions::new()
                .map_mut(&file)
                .context("Failed to memory-map shared memory file")?
        };

        // Validate the header
        let header_ptr = mmap.as_ptr() as *mut RingHeader;
        let is_valid = unsafe { (*header_ptr).validate() };
        if !is_valid {
            anyhow::bail!("Invalid ring buffer header");
        }

        // Check that size and slot_size match
        let size = unsafe { (*header_ptr).size.load(Ordering::Relaxed) };
        let slot_size = unsafe { (*header_ptr).slot_size.load(Ordering::Relaxed) };

        if size != options.size || slot_size != options.slot_size {
            // We'll just log a warning and continue, using the actual values
            eprintln!(
                "Warning: Ring buffer size or slot size mismatch. Expected: {}:{}, Actual: {}:{}",
                options.size, options.slot_size, size, slot_size
            );
        }

        Ok(Self {
            mmap,
            header: header_ptr,
            path: options.path,
            size,
            slot_size,
            is_producer: false,
        })
    }

    /// Writes data to the ring buffer.
    ///
    /// This writes data to the next available slot in the ring buffer
    /// and returns the slot index where the data was written.
    pub fn write(&self, kind: OperationKind, data: &[u8]) -> Result<usize> {
        if data.len() > self.max_payload_size() {
            anyhow::bail!("Data too large for slot");
        }

        let _header = unsafe { &*self.header };

        // Wait for an available slot
        let slot_idx = self.wait_for_slot()?;

        // Get the slot
        let slot = unsafe { Self::get_slot(&self.mmap, self.header, slot_idx, self.slot_size) };

        // Write data to the slot
        slot.set_operation(kind);
        slot.set_id(Uuid::new_v4());
        unsafe { slot.write_data(data).map_err(|e| anyhow::anyhow!(e))? };

        // Slot is now Ready (status set by write_data)

        Ok(slot_idx)
    }

    /// Reads data from the ring buffer.
    ///
    /// This reads data from the next available slot in the ring buffer
    /// and returns the operation kind and data.
    ///
    /// If no data is available, returns None.
    pub fn read(&self) -> Result<Option<(OperationKind, Vec<u8>)>> {
        let _header = unsafe { &*self.header };

        // Check if there's data to read
        let producer_idx = _header.producer_idx.load(Ordering::Acquire);
        let consumer_idx = _header.consumer_idx.load(Ordering::Acquire);

        if producer_idx == consumer_idx {
            // No new data
            return Ok(None);
        }

        // Get the next consumer index
        let slot_idx = _header.next_consumer_idx();

        // Get the slot
        let slot = unsafe { Self::get_slot(&self.mmap, self.header, slot_idx, self.slot_size) };

        // Read data if the slot is ready
        if slot.status() == SlotStatus::Ready {
            let kind = slot.operation();
            let data = unsafe { slot.read_data() };

            // Data was read successfully
            if let Some(data) = data {
                // Increment the completed operations counter
                (*_header).increment_completed();

                return Ok(Some((kind, data)));
            }
        }

        // Reset the slot for reuse
        slot.reset();

        // No data or invalid slot
        Ok(None)
    }

    /// Returns the maximum payload size that can be stored in a slot.
    ///
    /// This is the slot size minus the size of the slot metadata.
    pub fn max_payload_size(&self) -> usize {
        self.slot_size - std::mem::size_of::<crate::ring::slot::SlotMetadata>()
    }

    /// Waits for an available slot in the ring buffer.
    ///
    /// If the ring buffer is full, this will wait until a slot becomes
    /// available, with exponential backoff.
    fn wait_for_slot(&self) -> Result<usize> {
        let header = unsafe { &*self.header };
        let slot_count = header.slot_count.load(Ordering::Relaxed);

        let mut attempt = 0;
        let max_attempts = 10; // After this, we'll return an error
        let mut backoff_ms = 1;

        loop {
            // Get the next producer index
            let producer_idx = header.producer_idx.load(Ordering::Relaxed);
            let consumer_idx = header.consumer_idx.load(Ordering::Relaxed);

            // Check if the ring is full
            let available_slots = if producer_idx >= consumer_idx {
                slot_count - (producer_idx - consumer_idx)
            } else {
                consumer_idx - producer_idx
            };

            if available_slots > 0 {
                // Get the slot
                let slot_idx = header.next_producer_idx();
                let slot =
                    unsafe { Self::get_slot(&self.mmap, self.header, slot_idx, self.slot_size) };

                // Check if the slot is available
                if slot.status() == SlotStatus::Empty || slot.status() == SlotStatus::Complete {
                    // Reset the slot to clear any old data
                    slot.reset();
                    return Ok(slot_idx);
                }
            }

            // Ring is full or slot not available
            attempt += 1;
            if attempt > max_attempts {
                anyhow::bail!("Ring buffer full after {} attempts", max_attempts);
            }

            // Exponential backoff
            std::thread::sleep(Duration::from_millis(backoff_ms));
            backoff_ms = std::cmp::min(backoff_ms * 2, 100);
        }
    }

    /// Gets a slot from the ring buffer.
    ///
    /// # Safety
    ///
    /// This function is unsafe because it dereferences raw pointers.
    /// The caller must ensure that the header pointer is valid and
    /// the slot index is within bounds.
    unsafe fn get_slot(
        mmap: &MmapMut,
        header: *const RingHeader,
        idx: usize,
        slot_size: usize,
    ) -> Slot {
        let slot_count = (*header).slot_count.load(Ordering::Relaxed);
        assert!(idx < slot_count, "Slot index out of bounds");

        let header_size = std::mem::size_of::<RingHeader>();
        let slot_offset = header_size + idx * slot_size;

        // Use a raw pointer conversion that doesn't require mutable access
        let ptr = mmap.as_ptr().add(slot_offset) as *mut u8;
        Slot::from_ptr(ptr, slot_size)
    }

    /// Returns the path to the shared memory file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the size of the ring buffer in bytes.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Returns the size of each slot in bytes.
    pub fn slot_size(&self) -> usize {
        self.slot_size
    }

    /// Returns the number of slots in the ring buffer.
    pub fn slot_count(&self) -> usize {
        unsafe { (*self.header).slot_count.load(Ordering::Relaxed) }
    }

    /// Returns the number of active producers.
    pub fn active_producers(&self) -> usize {
        unsafe { (*self.header).active_producers.load(Ordering::Relaxed) }
    }

    /// Returns the number of completed operations.
    pub fn operations_completed(&self) -> usize {
        unsafe { (*self.header).operations_completed.load(Ordering::Relaxed) }
    }

    /// Returns the creation timestamp of the ring buffer.
    pub fn created_at(&self) -> u64 {
        unsafe { (*self.header).created_at.load(Ordering::Relaxed) }
    }

    /// Returns whether this instance is a producer.
    pub fn is_producer(&self) -> bool {
        self.is_producer
    }

    /// Registers this instance as a producer.
    ///
    /// This increments the active producer count in the header.
    pub fn register_as_producer(&mut self) {
        if !self.is_producer {
            unsafe {
                (*self.header).register_producer();
            }
            self.is_producer = true;
        }
    }

    /// Unregisters this instance as a producer.
    ///
    /// This decrements the active producer count in the header.
    pub fn unregister_as_producer(&mut self) {
        if self.is_producer {
            unsafe {
                (*self.header).unregister_producer();
            }
            self.is_producer = false;
        }
    }

    /// Creates a NUMA-aware ring buffer optimized for the given NUMA node.
    ///
    /// This creates a ring buffer with memory preferentially allocated on
    /// the specified NUMA node for optimal performance.
    pub fn create_numa_aware(options: RingBufferOptions, numa_node: usize) -> Result<Self> {
        // Get CPU info to check NUMA availability
        let cpu_info = CpuInfo::detect();

        if !cpu_info.is_numa_available() || numa_node >= cpu_info.numa_nodes().len() {
            // Fall back to regular creation if NUMA is not available or node is invalid
            return Self::create(options);
        }

        // Create the shared memory file
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o666)
            .open(&options.path)
            .context("Failed to create/open shared memory file")?;

        // Resize the file
        file.set_len(options.size as u64)
            .context("Failed to resize shared memory file")?;

        // Memory-map the file
        let mut mmap = unsafe {
            MmapOptions::new()
                .map_mut(&file)
                .context("Failed to memory-map shared memory file")?
        };

        // Conditionally compile this only on Linux
        #[cfg(target_os = "linux")]
        {
            // NUMA memory policy constants
            const MPOL_BIND: i32 = 2;
            const MPOL_MF_STRICT: i32 = 1;

            // Get the memory address and length
            let addr = mmap.as_mut_ptr() as *mut libc::c_void;
            let len = options.size;

            // Create a bitmask for the NUMA node
            let mask: u64 = 1 << numa_node;
            let max_node = 64; // Standard bitmask size

            // Apply mbind using the FFI interface
            let res = unsafe {
                libc::syscall(
                    libc::SYS_mbind,
                    addr,
                    len,
                    MPOL_BIND,
                    &mask as *const u64,
                    max_node,
                    MPOL_MF_STRICT,
                )
            };

            if res != 0 {
                let err = std::io::Error::last_os_error();
                tracing::warn!("Failed to bind memory to NUMA node {}: {}", numa_node, err);
            } else {
                tracing::debug!("Memory bound to NUMA node {}", numa_node);
            }
        }

        // Initialize the header
        let header_ptr = mmap.as_mut_ptr() as *mut RingHeader;
        unsafe {
            std::ptr::write(header_ptr, RingHeader::new(options.size, options.slot_size));

            // Add NUMA node information to the header
            (*header_ptr)
                .numa_node
                .store(numa_node as u32, Ordering::Relaxed);

            // Register as a producer
            (*header_ptr).register_producer();
        }

        // Initialize slots
        let slot_count = unsafe { (*header_ptr).slot_count.load(Ordering::Relaxed) };
        for i in 0..slot_count {
            let slot = unsafe { Self::get_slot(&mmap, header_ptr, i, options.slot_size) };
            slot.reset();
        }

        Ok(Self {
            mmap,
            header: header_ptr,
            path: options.path,
            size: options.size,
            slot_size: options.slot_size,
            is_producer: true,
        })
    }
}

impl Drop for RingBuffer {
    fn drop(&mut self) {
        // If this instance is a producer, decrement the producer count
        if self.is_producer {
            self.unregister_as_producer();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_ring_buffer_creation() {
        // Use a test-specific path to avoid conflicts
        let test_path = "/tmp/elgate_test_ring_creation";

        // Clean up any previous test run
        let _ = fs::remove_file(test_path);

        // Create options with test path
        let options = RingBufferOptions {
            path: PathBuf::from(test_path),
            size: 1024 * 1024, // 1 MiB
            slot_size: 4096,   // 4 KiB
        };

        // Create a new ring buffer
        let ring = RingBuffer::create(options.clone()).unwrap();

        // Check the ring buffer properties
        assert_eq!(ring.size(), 1024 * 1024);
        assert_eq!(ring.slot_size(), 4096);
        assert!(ring.slot_count() > 0);
        assert_eq!(ring.active_producers(), 1);
        assert_eq!(ring.operations_completed(), 0);
        assert!(ring.is_producer());

        // Clean up
        drop(ring);
        let _ = fs::remove_file(test_path);
    }

    #[test]
    fn test_ring_buffer_write_read() {
        // Use a test-specific path to avoid conflicts
        let test_path = "/tmp/elgate_test_ring_write_read";

        // Clean up any previous test run
        let _ = fs::remove_file(test_path);

        // Create options with test path
        let options = RingBufferOptions {
            path: PathBuf::from(test_path),
            size: 1024 * 1024, // 1 MiB
            slot_size: 4096,   // 4 KiB
        };

        // Create a new ring buffer
        let ring = RingBuffer::create(options.clone()).unwrap();

        // Write data to the ring buffer
        let data = b"Hello, world!";
        let _slot_idx = ring.write(OperationKind::DiskWrite, data).unwrap();

        // Read data from the ring buffer
        let result = ring.read().unwrap();
        assert!(result.is_some());

        let (kind, read_data) = result.unwrap();
        assert_eq!(kind, OperationKind::DiskWrite);
        assert_eq!(read_data, data);

        // Check that operations completed was incremented
        assert_eq!(ring.operations_completed(), 1);

        // Clean up
        drop(ring);
        let _ = fs::remove_file(test_path);
    }

    #[test]
    fn test_ring_buffer_multiple_writes() {
        // Use a test-specific path to avoid conflicts
        let test_path = "/tmp/elgate_test_ring_multiple_writes";

        // Clean up any previous test run
        let _ = fs::remove_file(test_path);

        // Create options with test path
        let options = RingBufferOptions {
            path: PathBuf::from(test_path),
            size: 1024 * 1024, // 1 MiB
            slot_size: 4096,   // 4 KiB
        };

        // Create a new ring buffer
        let ring = RingBuffer::create(options.clone()).unwrap();

        // Write multiple operations
        for i in 0..10 {
            let data = format!("Operation {}", i).into_bytes();
            ring.write(OperationKind::DiskWrite, &data).unwrap();
        }

        // Read all operations
        for i in 0..10 {
            let result = ring.read().unwrap();
            assert!(result.is_some());

            let (kind, read_data) = result.unwrap();
            assert_eq!(kind, OperationKind::DiskWrite);
            assert_eq!(read_data, format!("Operation {}", i).into_bytes());
        }

        // No more operations should be available
        assert!(ring.read().unwrap().is_none());

        // Check that operations completed was incremented
        assert_eq!(ring.operations_completed(), 10);

        // Clean up
        drop(ring);
        let _ = fs::remove_file(test_path);
    }

    #[test]
    fn test_ring_buffer_numa_aware() {
        use crate::arch::cpu_info::CpuInfo;

        // Use a test-specific path to avoid conflicts
        let test_path = "/tmp/elgate_test_ring_numa";

        // Clean up any previous test run
        let _ = fs::remove_file(test_path);

        // Get CPU info to check NUMA availability
        let cpu_info = CpuInfo::detect();

        // Create options with test path
        let options = RingBufferOptions {
            path: PathBuf::from(test_path),
            size: 1024 * 1024, // 1 MiB
            slot_size: 4096,   // 4 KiB
        };

        // Create a NUMA-aware ring buffer
        // Use node 0 for testing, as it should be available on all systems
        let ring = RingBuffer::create_numa_aware(options.clone(), 0).unwrap();

        // Check the ring buffer properties
        assert_eq!(ring.size(), 1024 * 1024);
        assert_eq!(ring.slot_size(), 4096);
        assert!(ring.slot_count() > 0);
        assert_eq!(ring.active_producers(), 1);

        // If NUMA is available, check that the NUMA node is set correctly
        if cpu_info.is_numa_available() && cpu_info.numa_nodes().len() > 0 {
            let header = unsafe { &*ring.header };
            assert_eq!(header.numa_node(), 0);
        }

        // Test write and read on the NUMA-optimized buffer
        let data = b"NUMA-optimized data";
        let _slot_idx = ring.write(OperationKind::DiskWrite, data).unwrap();

        // Read the data back
        let result = ring.read().unwrap();
        assert!(result.is_some());

        let (kind, read_data) = result.unwrap();
        assert_eq!(kind, OperationKind::DiskWrite);
        assert_eq!(read_data, data);

        // Clean up
        drop(ring);
        let _ = fs::remove_file(test_path);
    }
}
