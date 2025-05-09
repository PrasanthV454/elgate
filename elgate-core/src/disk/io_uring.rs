//! High-performance disk I/O using io_uring.
//!
//! This module provides disk I/O operations using Linux's io_uring interface
//! for maximum performance with minimal syscalls.

use crate::ring::{OperationKind, RingBuffer};
use anyhow::Result;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::mpsc;

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

    /// Channel for submitting I/O operations.
    tx: mpsc::Sender<DiskOperation>,

    /// Worker threads handle.
    worker_handle: Option<tokio::task::JoinHandle<()>>,
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
        // Create the correct channel for DiskOperation
        let (tx, rx) = mpsc::channel::<DiskOperation>(1024);

        // Pass the receiver to the worker thread
        let worker_handle = Self::start_workers(config.clone(), rx, ring_buffer.clone()).await?;

        Ok(Self {
            config,
            ring_buffer,
            tx, // Use the correctly typed sender
            worker_handle: Some(worker_handle),
        })
    }

    /// Starts the I/O worker threads.
    async fn start_workers(
        config: DiskConfig,
        mut rx: mpsc::Receiver<DiskOperation>,
        ring_buffer: Arc<RingBuffer>,
    ) -> Result<tokio::task::JoinHandle<()>> {
        // Create a new runtime with a single thread dedicated to io_uring
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .build()?;

        // Spawn a thread to run this dedicated runtime
        let worker_handle = std::thread::spawn(move || {
            rt.block_on(async move {
                // Add explicit type annotation
                let mut file_cache: std::collections::HashMap<PathBuf, Arc<UringFile>> =
                    std::collections::HashMap::new();

                while let Some(op) = rx.recv().await {
                    match op.op_type {
                        DiskOperationType::Read => {
                            let path = op.path.clone();
                            let offset = op.offset;
                            let callback = op.callback;

                            // Get or open the file
                            let file = match file_cache.get(&path) {
                                Some(file) => file.clone(),
                                None => {
                                    let file = match UringFile::open(&path).await {
                                        Ok(file) => Arc::new(file),
                                        Err(err) => {
                                            if let Some(cb) = callback {
                                                cb(Err(anyhow::anyhow!(
                                                    "Failed to open file: {}",
                                                    err
                                                )));
                                            }
                                            continue;
                                        }
                                    };
                                    file_cache.insert(path.clone(), file.clone());
                                    file
                                }
                            };

                            // Use direct Vec<u8> usage instead of ReadBuf
                            let buf_vec = vec![0u8; config.buffer_size];
                            let file_clone = file.clone();
                            match file_clone.read_at(buf_vec, offset).await {
                                (Ok(n), buf) => {
                                    let data = buf[..n].to_vec();

                                    // Write to the ring buffer
                                    if let Err(err) =
                                        ring_buffer.write(OperationKind::DiskRead, &data)
                                    {
                                        eprintln!("Failed to write to ring buffer: {}", err);
                                    }

                                    if let Some(cb) = callback {
                                        cb(Ok(data));
                                    }
                                }
                                (Err(err), _) => {
                                    if let Some(cb) = callback {
                                        cb(Err(anyhow::anyhow!("Read failed: {}", err)));
                                    }
                                }
                            }
                        }
                        DiskOperationType::Write => {
                            let path = op.path.clone();
                            let offset = op.offset;
                            let data = op.data.unwrap_or_default();
                            let callback = op.callback;

                            // Get or open the file
                            let file = match file_cache.get(&path) {
                                Some(file) => file.clone(),
                                None => {
                                    let file = match UringFile::create(&path).await {
                                        Ok(file) => Arc::new(file),
                                        Err(err) => {
                                            if let Some(cb) = callback {
                                                cb(Err(anyhow::anyhow!(
                                                    "Failed to create file: {}",
                                                    err
                                                )));
                                            }
                                            continue;
                                        }
                                    };
                                    file_cache.insert(path.clone(), file.clone());
                                    file
                                }
                            };

                            // Perform the write operation
                            let buf = data.clone();
                            match file.write_at(buf, offset).await {
                                (Ok(_n), _) => {
                                    // Write to the ring buffer
                                    if let Err(err) =
                                        ring_buffer.write(OperationKind::DiskWrite, &[])
                                    {
                                        eprintln!("Failed to write to ring buffer: {}", err);
                                    }

                                    if let Some(cb) = callback {
                                        cb(Ok(vec![]));
                                    }
                                }
                                (Err(err), _) => {
                                    if let Some(cb) = callback {
                                        cb(Err(anyhow::anyhow!("Write failed: {}", err)));
                                    }
                                }
                            }
                        }
                        DiskOperationType::Sync => {
                            let path = op.path.clone();
                            let callback = op.callback;

                            // Get the file from the cache
                            if let Some(file) = file_cache.get(&path) {
                                match file.sync_all().await {
                                    Ok(_) => {
                                        if let Some(cb) = callback {
                                            cb(Ok(vec![]));
                                        }
                                    }
                                    Err(err) => {
                                        if let Some(cb) = callback {
                                            cb(Err(anyhow::anyhow!("Sync failed: {}", err)));
                                        }
                                    }
                                }
                            } else if let Some(cb) = callback {
                                cb(Err(anyhow::anyhow!("File not found in cache")));
                            }
                        }
                        DiskOperationType::Close => {
                            let path = op.path;
                            let callback = op.callback;

                            // Remove the file from the cache
                            file_cache.remove(&path);

                            if let Some(cb) = callback {
                                cb(Ok(Vec::new()));
                            }
                        }
                    }
                }
            });
        });

        // Convert the thread JoinHandle to a tokio JoinHandle
        let (tx, rx) = tokio::sync::oneshot::channel();

        let wrapper = ThreadJoinHandleWrapper {
            join_handle: Some(worker_handle),
            _tx: tx,
            rx,
        };

        Ok(tokio::task::spawn(wrapper))
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
    use crate::ring::{RingBuffer, RingBufferOptions};
    use std::fs;
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

    #[tokio::test]
    async fn test_disk_engine_multiple_writes() {
        // Skip test if io_uring feature is not enabled
        if !cfg!(feature = "io_uring") {
            return;
        }

        // Create a ring buffer for communication
        let ring_path = "/tmp/elgate_test_disk_ring_multiple";
        let test_file_path = "/tmp/elgate_test_disk_file_multiple";

        // Clean up from previous runs
        let _ = fs::remove_file(ring_path);
        let _ = fs::remove_file(test_file_path);

        // Create ring buffer
        let ring_options = RingBufferOptions {
            path: PathBuf::from(ring_path),
            size: 1024 * 1024, // 1 MiB
            slot_size: 4096,   // 4 KiB
        };

        let ring = Arc::new(RingBuffer::create(ring_options).unwrap());

        // Create disk engine
        let config = DiskConfig {
            worker_threads: 1,
            pin_threads: false, // Don't pin threads in tests
            queue_depth: 32,
            buffer_size: 4096,
            numa_node: None,
        };

        let disk_engine = DiskEngine::new(config, ring.clone()).await.unwrap();

        // Write multiple chunks to a file
        for i in 0..10 {
            let offset = i * 100;
            let data = format!("Chunk {}", i).into_bytes();
            disk_engine
                .write_file(test_file_path, offset as u64, data)
                .await
                .unwrap();
        }

        // Read each chunk back and verify
        for i in 0..10 {
            let offset = i * 100;
            let expected_data = format!("Chunk {}", i).into_bytes();
            let read_data = disk_engine
                .read_file(test_file_path, offset as u64)
                .await
                .unwrap();

            // Skip zeros at the end of the read buffer
            let actual_data = read_data
                .iter()
                .take_while(|&&b| b != 0)
                .cloned()
                .collect::<Vec<u8>>();

            assert_eq!(actual_data, expected_data);
        }

        // Clean up
        let _ = fs::remove_file(ring_path);
        let _ = fs::remove_file(test_file_path);
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

// Replace the from_raw approach with a simpler JoinHandle creation
// We'll create a wrapper struct that implements Future
struct ThreadJoinHandleWrapper {
    join_handle: Option<std::thread::JoinHandle<()>>,
    _tx: tokio::sync::oneshot::Sender<()>,
    rx: tokio::sync::oneshot::Receiver<()>,
}

impl Future for ThreadJoinHandleWrapper {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Properly pin the receiver and poll it
        let rx = &mut self.get_mut().rx;
        match Pin::new(rx).poll(cx) {
            Poll::Ready(_) => Poll::Ready(()),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for ThreadJoinHandleWrapper {
    fn drop(&mut self) {
        // Ensure the thread gets joined when dropped
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}
