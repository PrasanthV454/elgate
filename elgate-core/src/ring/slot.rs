//! Slot structure for the shared memory ring buffer.
//!
//! This module defines the slot structure that holds operations
//! and their metadata in the ring buffer.

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Status of a slot in the ring buffer.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotStatus {
    /// Slot is empty and available for writing.
    Empty = 0,

    /// Slot is being written to by a producer.
    Writing = 1,

    /// Slot is ready to be read by the consumer.
    Ready = 2,

    /// Slot is being read by the consumer.
    Reading = 3,

    /// Slot processing is complete and can be reused.
    Complete = 4,
}

/// Kind of operation stored in a slot.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationKind {
    /// No operation (empty slot).
    None = 0,

    /// Write to disk.
    DiskWrite = 1,

    /// Read from disk.
    DiskRead = 2,

    /// Send over network.
    NetworkSend = 3,

    /// Receive from network.
    NetworkReceive = 4,

    /// Close network connection.
    NetworkClose = 5,

    /// Custom operation type.
    Custom = 100,
}

/// Operation details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Operation {
    /// Kind of operation.
    pub kind: OperationKind,

    /// Size of the payload in bytes.
    pub size: usize,

    /// Operation ID (UUID).
    pub id: [u8; 16],

    /// Timestamp when the operation was created.
    pub timestamp: u64,
}

/// Metadata for a slot in the ring buffer.
///
/// This structure is placed at the beginning of each slot and
/// contains metadata about the operation stored in the slot.
#[repr(C, align(64))]
pub struct SlotMetadata {
    /// Status of the slot.
    pub status: AtomicU32,

    /// Kind of operation.
    pub operation: AtomicU32,

    /// Size of the payload in bytes.
    pub size: AtomicUsize,

    /// Operation ID (UUID).
    pub id: [AtomicU32; 4],

    /// Timestamp when the operation was created.
    pub timestamp: AtomicU64,

    /// Reserved for future use.
    pub _reserved: [AtomicU32; 2],
}

/// A slot in the ring buffer.
///
/// This represents a single slot in the ring buffer, containing
/// metadata and the payload data area.
pub struct Slot {
    /// Pointer to the slot's metadata.
    pub metadata: *mut SlotMetadata,

    /// Pointer to the payload data area.
    pub data: *mut u8,

    /// Size of the payload area in bytes.
    pub data_size: usize,
}

impl Slot {
    /// Creates a new slot from a raw memory pointer.
    ///
    /// # Safety
    ///
    /// This function is unsafe because it dereferences raw pointers.
    /// The caller must ensure that the pointer is valid and points
    /// to a valid memory region of sufficient size.
    pub unsafe fn from_ptr(ptr: *mut u8, slot_size: usize) -> Self {
        let metadata = ptr as *mut SlotMetadata;
        let data = ptr.add(std::mem::size_of::<SlotMetadata>());
        let data_size = slot_size - std::mem::size_of::<SlotMetadata>();

        Self {
            metadata,
            data,
            data_size,
        }
    }

    /// Gets the status of the slot.
    pub fn status(&self) -> SlotStatus {
        let status = unsafe { (*self.metadata).status.load(Ordering::Acquire) };
        match status {
            0 => SlotStatus::Empty,
            1 => SlotStatus::Writing,
            2 => SlotStatus::Ready,
            3 => SlotStatus::Reading,
            4 => SlotStatus::Complete,
            _ => SlotStatus::Empty,
        }
    }

    /// Sets the status of the slot.
    pub fn set_status(&self, status: SlotStatus) {
        unsafe {
            (*self.metadata)
                .status
                .store(status as u32, Ordering::Release);
        }
    }

    /// Gets the operation kind of the slot.
    pub fn operation(&self) -> OperationKind {
        let op = unsafe { (*self.metadata).operation.load(Ordering::Acquire) };
        match op {
            0 => OperationKind::None,
            1 => OperationKind::DiskWrite,
            2 => OperationKind::DiskRead,
            3 => OperationKind::NetworkSend,
            4 => OperationKind::NetworkReceive,
            5 => OperationKind::NetworkClose,
            100 => OperationKind::Custom,
            _ => OperationKind::None,
        }
    }

    /// Sets the operation kind of the slot.
    pub fn set_operation(&self, operation: OperationKind) {
        unsafe {
            (*self.metadata)
                .operation
                .store(operation as u32, Ordering::Release);
        }
    }

    /// Gets the size of the payload.
    pub fn size(&self) -> usize {
        unsafe { (*self.metadata).size.load(Ordering::Acquire) }
    }

    /// Sets the size of the payload.
    pub fn set_size(&self, size: usize) {
        unsafe {
            (*self.metadata).size.store(size, Ordering::Release);
        }
    }

    /// Gets the operation ID as a UUID.
    pub fn id(&self) -> Uuid {
        let parts = unsafe {
            [
                (*self.metadata).id[0].load(Ordering::Acquire),
                (*self.metadata).id[1].load(Ordering::Acquire),
                (*self.metadata).id[2].load(Ordering::Acquire),
                (*self.metadata).id[3].load(Ordering::Acquire),
            ]
        };

        let bytes = parts
            .iter()
            .flat_map(|p| p.to_be_bytes())
            .collect::<Vec<_>>();
        Uuid::from_slice(&bytes).unwrap_or_else(|_| Uuid::nil())
    }

    /// Sets the operation ID from a UUID.
    pub fn set_id(&self, id: Uuid) {
        let bytes = id.as_bytes();

        unsafe {
            (*self.metadata).id[0].store(
                u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
                Ordering::Release,
            );
            (*self.metadata).id[1].store(
                u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
                Ordering::Release,
            );
            (*self.metadata).id[2].store(
                u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
                Ordering::Release,
            );
            (*self.metadata).id[3].store(
                u32::from_be_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
                Ordering::Release,
            );
        }
    }

    /// Gets the operation timestamp.
    pub fn timestamp(&self) -> u64 {
        unsafe { (*self.metadata).timestamp.load(Ordering::Acquire) }
    }

    /// Sets the operation timestamp.
    pub fn set_timestamp(&self, timestamp: u64) {
        unsafe {
            (*self.metadata)
                .timestamp
                .store(timestamp, Ordering::Release);
        }
    }

    /// Gets the payload data as a slice.
    ///
    /// # Safety
    ///
    /// This function is unsafe because it creates a slice from a raw pointer.
    /// The caller must ensure that the pointer is valid and the memory
    /// region is of sufficient size.
    pub unsafe fn data_slice(&self, len: usize) -> &[u8] {
        std::slice::from_raw_parts(self.data, len)
    }

    /// Gets the payload data as a mutable slice.
    ///
    /// # Safety
    ///
    /// This function is unsafe because it creates a mutable slice from a raw pointer.
    /// The caller must ensure that the pointer is valid, the memory region is of
    /// sufficient size, and no other references to the memory exist.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn data_slice_mut(&self, len: usize) -> &mut [u8] {
        std::slice::from_raw_parts_mut(self.data, len)
    }

    /// Writes data to the slot.
    ///
    /// # Safety
    ///
    /// This function is unsafe because it writes to memory through a raw pointer.
    /// The caller must ensure that the pointer is valid and the memory region
    /// is of sufficient size.
    pub unsafe fn write_data(&self, data: &[u8]) -> Result<(), &'static str> {
        if data.len() > self.data_size {
            return Err("Data too large for slot");
        }

        self.set_status(SlotStatus::Writing);
        self.set_size(data.len());
        self.set_timestamp(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        );

        std::ptr::copy_nonoverlapping(data.as_ptr(), self.data, data.len());

        self.set_status(SlotStatus::Ready);
        Ok(())
    }

    /// Reads data from the slot.
    ///
    /// # Safety
    ///
    /// This function is unsafe because it reads memory through a raw pointer.
    /// The caller must ensure that the pointer is valid and the memory region
    /// is of sufficient size.
    pub unsafe fn read_data(&self) -> Option<Vec<u8>> {
        if self.status() != SlotStatus::Ready {
            return None;
        }

        self.set_status(SlotStatus::Reading);

        let size = self.size();
        if size > self.data_size {
            self.set_status(SlotStatus::Complete);
            return None;
        }

        let mut buffer = vec![0u8; size];
        std::ptr::copy_nonoverlapping(self.data, buffer.as_mut_ptr(), size);

        self.set_status(SlotStatus::Complete);

        Some(buffer)
    }

    /// Resets the slot to empty state.
    pub fn reset(&self) {
        self.set_status(SlotStatus::Empty);
        self.set_operation(OperationKind::None);
        self.set_size(0);
        self.set_id(Uuid::nil());
        self.set_timestamp(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::{alloc, Layout};

    #[test]
    fn test_slot_operations() {
        // Allocate memory for a slot
        let slot_size = 4096;
        let layout = Layout::from_size_align(slot_size, 64).unwrap();
        let ptr = unsafe { alloc(layout) };
        assert!(!ptr.is_null());

        // Create a slot from the memory
        let slot = unsafe { Slot::from_ptr(ptr, slot_size) };

        // Reset the slot
        slot.reset();

        // Check initial state
        assert_eq!(slot.status(), SlotStatus::Empty);
        assert_eq!(slot.operation(), OperationKind::None);
        assert_eq!(slot.size(), 0);
        assert_eq!(slot.id(), Uuid::nil());

        // Set operation details
        slot.set_operation(OperationKind::DiskWrite);
        let id = Uuid::new_v4();
        slot.set_id(id);

        // Check operation details
        assert_eq!(slot.operation(), OperationKind::DiskWrite);
        assert_eq!(slot.id(), id);

        // Write data
        let data = b"test data";
        unsafe { slot.write_data(data).unwrap() };

        // Check after writing
        assert_eq!(slot.status(), SlotStatus::Ready);
        assert_eq!(slot.size(), data.len());

        // Read data
        let read_data = unsafe { slot.read_data() }.unwrap();

        // Check after reading
        assert_eq!(read_data, data);
        assert_eq!(slot.status(), SlotStatus::Complete);

        // Reset
        slot.reset();
        assert_eq!(slot.status(), SlotStatus::Empty);

        // Clean up
        unsafe { std::alloc::dealloc(ptr, layout) };
    }
}
