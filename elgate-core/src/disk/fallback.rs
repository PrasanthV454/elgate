//! Standard file I/O fallback implementation.
//!
//! This module provides a fallback disk I/O implementation using
//! standard Rust file I/O for platforms without io_uring support.

use crate::ring::{OperationKind, RingBuffer};
use anyhow::{Context, Result};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Configuration for the disk I/O engine.
#[derive(Debug, Clone)]
pub struct DiskConfig {
    /// Number of I/O worker threads to use.
    pub worker_threads: usize,
    
    /// Whether to pin I/O worker threads to CPUs.
    pub pin_threads: bool,
    
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
            buffer_size: 128 * 1024, // 128 KiB
            numa_node: None,
        }
    }
}

/// A disk I/O engine using standard Rust file I/O.
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
#[derive(Debug)]
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

impl DiskEngine {
    /// Creates a new disk I/O engine.
    pub async fn new(config: DiskConfig, ring_buffer: Arc<RingBuffer>) -> Result<Self> {
        let (tx, rx) = mpsc::channel(config.worker_threads * 32);
        
        let engine = Self {
            config,
            ring_buffer,
            tx,
            worker_handle: None,
        };
        
        // Start worker threads
        let worker_handle = engine.start_workers(rx).await?;
        
        Ok(Self {
            worker_handle: Some(worker_handle),
            ..engine
        })
    }
    
    /// Starts worker threads for processing I/O operations.
    async fn start_workers(
        &self,
        mut rx: mpsc::Receiver<DiskOperation>,
    ) -> Result<tokio::task::JoinHandle<()>> {
        let worker_handle = tokio::spawn(async move {
            // Process operations from the channel
            while let Some(op) = rx.recv().await {
                match op.op_type {
                    DiskOperationType::Read => {
                        // Open the file
                        let result = match File::open(&op.path) {
                            Ok(mut file) => {
                                // Seek to the offset
                                if let Err(e) = file.seek(SeekFrom::Start(op.offset)) {
                                    Err(anyhow::anyhow!("Failed to seek: {}", e))
                                } else {
                                    // Read the data
                                    let mut buffer = vec![0u8; 4096]; // Fixed size for now
                                    match file.read(&mut buffer) {
                                        Ok(n) => {
                                            buffer.truncate(n);
                                            Ok(buffer)
                                        }
                                        Err(e) => Err(anyhow::anyhow!("Failed to read: {}", e)),
                                    }
                                }
                            }
                            Err(e) => Err(anyhow::anyhow!("Failed to open file: {}", e)),
                        };
                        
                        // Call the callback
                        if let Some(callback) = op.callback {
                            callback(result);
                        }
                    }
                    DiskOperationType::Write => {
                        let result = match OpenOptions::new().write(true).create(true).open(&op.path) {
                            Ok(mut file) => {
                                // Seek to the offset
                                if let Err(e) = file.seek(SeekFrom::Start(op.offset)) {
                                    Err(anyhow::anyhow!("Failed to seek: {}", e))
                                } else if let Some(data) = op.data {
                                    // Write the data
                                    match file.write_all(&data) {
                                        Ok(_) => Ok(vec![]),
                                        Err(e) => Err(anyhow::anyhow!("Failed to write: {}", e)),
                                    }
                                } else {
                                    Err(anyhow::anyhow!("No data provided for write operation"))
                                }
                            }
                            Err(e) => Err(anyhow::anyhow!("Failed to open file: {}", e)),
                        };
                        
                        // Call the callback
                        if let Some(callback) = op.callback {
                            callback(result);
                        }
                    }
                    DiskOperationType::Sync => {
                        let result = match File::open(&op.path) {
                            Ok(file) => match file.sync_all() {
                                Ok(_) => Ok(vec![]),
                                Err(e) => Err(anyhow::anyhow!("Failed to sync: {}", e)),
                            },
                            Err(e) => Err(anyhow::anyhow!("Failed to open file: {}", e)),
                        };
                        
                        // Call the callback
                        if let Some(callback) = op.callback {
                            callback(result);
                        }
                    }
                    DiskOperationType::Close => {
                        // No-op for the fallback implementation (Rust closes files automatically)
                        if let Some(callback) = op.callback {
                            callback(Ok(vec![]));
                        }
                    }
                }
            }
        });
        
        Ok(worker_handle)
    }
    
    /// Reads data from a file.
    pub async fn read_file<P: AsRef<Path>>(&self, path: P, offset: u64) -> Result<Vec<u8>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        
        self.tx.send(DiskOperation {
            op_type: DiskOperationType::Read,
            path: path.as_ref().to_path_buf(),
            offset,
            data: None,
            callback: Some(Box::new(move |result| {
                let _ = tx.send(result);
            })),
        }).await.context("Failed to send read operation")?;
        
        rx.await.context("Failed to receive read result")?
    }
    
    /// Writes data to a file.
    pub async fn write_file<P: AsRef<Path>>(&self, path: P, offset: u64, data: Vec<u8>) -> Result<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        
        self.tx.send(DiskOperation {
            op_type: DiskOperationType::Write,
            path: path.as_ref().to_path_buf(),
            offset,
            data: Some(data),
            callback: Some(Box::new(move |result| {
                let _ = tx.send(result.map(|_| ()));
            })),
        }).await.context("Failed to send write operation")?;
        
        rx.await.context("Failed to receive write result")?
    }
    
    /// Syncs a file to disk.
    pub async fn sync_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        
        self.tx.send(DiskOperation {
            op_type: DiskOperationType::Sync,
            path: path.as_ref().to_path_buf(),
            offset: 0,
            data: None,
            callback: Some(Box::new(move |result| {
                let _ = tx.send(result.map(|_| ()));
            })),
        }).await.context("Failed to send sync operation")?;
        
        rx.await.context("Failed to receive sync result")?
    }
    
    /// Closes a file.
    pub async fn close_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        
        self.tx.send(DiskOperation {
            op_type: DiskOperationType::Close,
            path: path.as_ref().to_path_buf(),
            offset: 0,
            data: None,
            callback: Some(Box::new(move |result| {
                let _ = tx.send(result.map(|_| ()));
            })),
        }).await.context("Failed to send close operation")?;
        
        rx.await.context("Failed to receive close result")?
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
    
    #[tokio::test]
    async fn test_fallback_disk_engine_write_read() {
        // Create a ring buffer for communication
        let ring_path = "/tmp/elgate_test_fallback_disk_ring";
        let test_file_path = "/tmp/elgate_test_fallback_disk_file";
        
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
            buffer_size: 4096,
            numa_node: None,
        };
        
        let disk_engine = DiskEngine::new(config, ring.clone()).await.unwrap();
        
        // Write data to a file
        let test_data = b"Hello, fallback disk engine!".to_vec();
        disk_engine.write_file(test_file_path, 0, test_data.clone()).await.unwrap();
        
        // Read the data back
        let read_data = disk_engine.read_file(test_file_path, 0).await.unwrap();
        
        // Verify data - only compare the written bytes, as fallback might return more data
        let read_data = read_data.iter().take(test_data.len()).cloned().collect::<Vec<u8>>();
        assert_eq!(read_data, test_data);
        
        // Sync and close
        disk_engine.sync_file(test_file_path).await.unwrap();
        disk_engine.close_file(test_file_path).await.unwrap();
        
        // Clean up
        let _ = fs::remove_file(ring_path);
        let _ = fs::remove_file(test_file_path);
    }
    
    #[tokio::test]
    async fn test_fallback_disk_engine_multiple_writes() {
        // Create a ring buffer for communication
        let ring_path = "/tmp/elgate_test_fallback_disk_ring_multiple";
        let test_file_path = "/tmp/elgate_test_fallback_disk_file_multiple";
        
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
            buffer_size: 4096,
            numa_node: None,
        };
        
        let disk_engine = DiskEngine::new(config, ring.clone()).await.unwrap();
        
        // Write multiple chunks to a file
        for i in 0..10 {
            let offset = i * 100;
            let data = format!("Chunk {}", i).into_bytes();
            disk_engine.write_file(test_file_path, offset as u64, data).await.unwrap();
        }
        
        // Read each chunk back and verify
        for i in 0..10 {
            let offset = i * 100;
            let expected_data = format!("Chunk {}", i).into_bytes();
            let read_data = disk_engine.read_file(test_file_path, offset as u64).await.unwrap();
            
            // Extract just the portion containing our data
            let actual_data = read_data.iter()
                .take(expected_data.len())
                .cloned()
                .collect::<Vec<u8>>();
            
            assert_eq!(actual_data, expected_data);
        }
        
        // Clean up
        let _ = fs::remove_file(ring_path);
        let _ = fs::remove_file(test_file_path);
    }
    
    #[tokio::test]
    async fn test_fallback_disk_engine_error_handling() {
        // Create a ring buffer for communication
        let ring_path = "/tmp/elgate_test_fallback_disk_ring_errors";
        
        // Clean up from previous runs
        let _ = fs::remove_file(ring_path);
        
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
            pin_threads: false,
            buffer_size: 4096,
            numa_node: None,
        };
        
        let disk_engine = DiskEngine::new(config, ring.clone()).await.unwrap();
        
        // Try to read a non-existent file
        let non_existent_file = "/tmp/this_file_does_not_exist_elgate_test";
        let _ = fs::remove_file(non_existent_file); // Ensure it doesn't exist
        
        let result = disk_engine.read_file(non_existent_file, 0).await;
        assert!(result.is_err(), "Reading non-existent file should fail");
        
        // Try to write with invalid offset (very large)
        let test_file = "/tmp/elgate_test_fallback_disk_file_errors";
        let result = disk_engine.write_file(test_file, u64::MAX - 10, b"test".to_vec()).await;
        assert!(result.is_err(), "Writing with invalid offset should fail");
        
        // Clean up
        let _ = fs::remove_file(ring_path);
        let _ = fs::remove_file(test_file);
    }
}
