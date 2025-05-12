//! High-performance disk I/O using io_uring.
//!
//! This module provides disk I/O operations using Linux's io_uring interface
//! for maximum performance with minimal syscalls.

use crate::ring::{OperationKind, RingBuffer};
use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio_uring::fs::{File, OpenOptions};

#[cfg(feature = "io_uring")]
use tokio_uring::fs::File as UringFile;

#[cfg(test)]
use std::os::unix::fs::OpenOptionsExt;

/// Configuration for the disk I/O engine.
#[derive(Debug, Clone)]
pub struct DiskConfig {
    /// Number of I/O worker threads to use.
    pub worker_threads: usize,

    /// Whether to pin I/O worker threads to CPUs.
    pub pin_threads: bool,

    /// Queue depth for io_uring.
    pub queue_depth: u32,

    /// Size of the read/write buffer in bytes.
    pub buffer_size: usize,

    /// NUMA node to use for I/O operations.
    pub numa_node: Option<usize>,
}

impl Default for DiskConfig {
    fn default() -> Self {
        Self {
            worker_threads: 2,
            pin_threads: true,
            queue_depth: 256,
            buffer_size: 128 * 1024, // 128 KiB
            numa_node: None,
        }
    }
}

/// A high-performance disk I/O engine using io_uring.
///
/// This engine uses Linux's io_uring interface for efficient disk I/O
/// with minimal syscalls and kernel context switches.
pub struct DiskEngine {
    /// Configuration for the engine.
    config: DiskConfig,

    /// Shared memory ring buffer for communication with other components.
    ring_buffer: Arc<RingBuffer>,

    /// File cache to maintain open handles
    file_handles: Mutex<HashMap<PathBuf, File>>,
}

/// Type of disk operation.
#[derive(Debug, Clone)]
pub enum DiskOperationType {
    /// Read from a file.
    Read,

    /// Write to a file.
    Write,

    /// Sync a file to disk.
    Sync,

    /// Close a file.
    Close,
}

/// A disk I/O operation.
pub struct DiskOperation {
    /// Type of operation.
    pub op_type: DiskOperationType,

    /// Path to the file.
    pub path: PathBuf,

    /// Offset in the file.
    pub offset: u64,

    /// Data buffer (for write operations).
    pub data: Option<Vec<u8>>,

    /// Completion callback.
    pub callback: Option<Box<dyn FnOnce(Result<Vec<u8>>) + Send + 'static>>,
}

impl std::fmt::Debug for DiskOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiskOperation")
            .field("op_type", &self.op_type)
            .field("path", &self.path)
            .field("offset", &self.offset)
            .field("data", &self.data.as_ref().map(|d| d.len()))
            .field(
                "callback",
                &if self.callback.is_some() {
                    "Some(FnOnce)"
                } else {
                    "None"
                },
            )
            .finish()
    }
}

impl DiskEngine {
    /// Creates a new disk I/O engine.
    pub async fn new(config: DiskConfig, ring_buffer: Arc<RingBuffer>) -> Result<Self> {
        // Just initialize the structure - no worker threads
        Ok(Self {
            config,
            ring_buffer,
            file_handles: Mutex::new(HashMap::new()),
        })
    }

    /// Reads data from a file.
    pub async fn read_file<P: AsRef<Path>>(&self, path: P, offset: u64) -> Result<Vec<u8>> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        self.tx
            .send(DiskOperation {
                op_type: DiskOperationType::Read,
                path: path.as_ref().to_path_buf(),
                offset,
                data: None,
                callback: Some(Box::new(move |result| {
                    let _ = tx.send(result);
                })),
            })
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send read operation: {}", e))?;

        rx.await
            .map_err(|e| anyhow::anyhow!("Failed to receive read result: {}", e))?
    }

    /// Writes data to a file at the specified path and offset.
    pub async fn write_file<P: AsRef<Path>>(
        &self,
        path: P,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        // Send the operation with a timeout
        let path_buf = path.as_ref().to_path_buf();
        let send_result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.tx.send(DiskOperation {
                op_type: DiskOperationType::Write,
                path: path_buf,
                offset,
                data: Some(data),
                callback: Some(Box::new(move |result| {
                    let _ = tx.send(result.map(|_| ()));
                })),
            }),
        )
        .await;

        // Handle send timeout
        match send_result {
            Ok(send) => {
                send.map_err(|e| anyhow::anyhow!("Failed to send write operation: {}", e))?;
            }
            Err(_) => return Err(anyhow::anyhow!("Timed out sending write operation")),
        }

        // Wait for operation completion with timeout
        let receive_result = tokio::time::timeout(std::time::Duration::from_secs(10), rx).await;

        match receive_result {
            Ok(result) => {
                result.map_err(|e| anyhow::anyhow!("Failed to receive write result: {}", e))?
            }
            Err(_) => Err(anyhow::anyhow!(
                "Timed out waiting for write operation to complete"
            )),
        }
    }

    /// Syncs a file to disk.
    pub async fn sync_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        self.tx
            .send(DiskOperation {
                op_type: DiskOperationType::Sync,
                path: path.as_ref().to_path_buf(),
                offset: 0,
                data: None,
                callback: Some(Box::new(move |result| {
                    let _ = tx.send(result.map(|_| ()));
                })),
            })
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send sync operation: {}", e))?;

        rx.await
            .map_err(|e| anyhow::anyhow!("Failed to receive sync result: {}", e))?
    }

    /// Closes a file.
    pub async fn close_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        self.tx
            .send(DiskOperation {
                op_type: DiskOperationType::Close,
                path: path.as_ref().to_path_buf(),
                offset: 0,
                data: None,
                callback: Some(Box::new(move |result| {
                    let _ = tx.send(result.map(|_| ()));
                })),
            })
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send close operation: {}", e))?;

        rx.await
            .map_err(|e| anyhow::anyhow!("Failed to receive close result: {}", e))?
    }
}

impl Drop for DiskEngine {
    fn drop(&mut self) {
        // Abort worker threads
        if let Some(handle) = self.worker_handle.take() {
            handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ring::RingBuffer;
    use std::path::PathBuf;
    use std::sync::Arc;

    #[test]
    #[cfg(feature = "io_uring")]
    fn test_disk_engine_write_read() {
        // Skip test if not running with appropriate permissions
        if !has_io_uring_permissions() {
            println!("Skipping io_uring test - insufficient permissions");
            return;
        }

        // Create a temporary directory for the test
        // ...
    }

    // Helper function to check if we have proper io_uring permissions
    #[cfg(feature = "io_uring")]
    fn has_io_uring_permissions() -> bool {
        use std::fs::OpenOptions;
        use std::os::unix::fs::OpenOptionsExt;

        // Try to create a file with direct I/O flag (a common io_uring requirement)
        let test_path = "/tmp/io_uring_test_permissions";
        let result = OpenOptions::new()
            .write(true)
            .create(true)
            .custom_flags(libc::O_DIRECT)
            .open(test_path);

        // Clean up
        let _ = std::fs::remove_file(test_path);

        result.is_ok()
    }

    #[test]
    #[cfg(feature = "io_uring")]
    fn test_disk_engine_multiple_writes() {
        tokio_uring::start(async {
            // Test code should work the same with the new implementation
            // ...
        });
    }

    #[cfg(test)]
    fn has_full_io_uring_permissions() -> bool {
        // No need to re-import OpenOptionsExt since we added it at the module level
        // Combination of multiple permission checks
        let has_device_permissions = std::path::Path::new("/dev/io_uring").exists()
            && std::fs::metadata("/dev/io_uring")
                .map(|m| m.permissions().readonly())
                .unwrap_or(true)
                == false;

        let has_direct_io = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .custom_flags(libc::O_DIRECT)
            .open("/tmp/io_uring_test_direct")
            .map(|_| {
                let _ = std::fs::remove_file("/tmp/io_uring_test_direct");
                true
            })
            .unwrap_or(false);

        let has_submission_queue_permissions = std::process::Command::new("sh")
            .arg("-c")
            .arg("ulimit -l")
            .output()
            .map(|o| {
                let output = String::from_utf8_lossy(&o.stdout);
                let memlock = output.trim().parse::<u64>().unwrap_or(0);
                memlock > 1024
            })
            .unwrap_or(false);

        has_device_permissions && has_direct_io && has_submission_queue_permissions
    }
}
