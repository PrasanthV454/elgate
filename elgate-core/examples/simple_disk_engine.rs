//! A simplified disk engine for examples
//!
//! This version avoids the thread context issues by using tokio-uring directly
//! without spawning additional worker threads.

use std::path::Path;
use tokio_uring::fs::File;

/// A simplified disk engine that works directly with tokio-uring
/// without creating separate worker threads
pub struct SimpleDiskEngine;

impl SimpleDiskEngine {
    /// Create a new instance of the simple disk engine
    pub fn new() -> Self {
        Self
    }

    /// Read from a file at the specified offset
    pub async fn read_file<P: AsRef<Path>>(
        &self,
        path: P,
        offset: u64,
    ) -> std::io::Result<Vec<u8>> {
        // Open the file directly
        let file = File::open(path).await?;

        // First read a small amount to determine file size
        let size_buf = vec![0u8; 1];
        let (res, _) = file.read_at(size_buf, offset).await;
        res?; // Check for errors

        // For simplicity, just read a fixed size
        let buf_size = 4096;
        let buf = vec![0u8; buf_size];

        // Read the data
        let (res, buf) = file.read_at(buf, offset).await;
        let bytes_read = res?;

        Ok(buf[..bytes_read].to_vec())
    }

    /// Write to a file at the specified offset
    pub async fn write_file<P: AsRef<Path>>(
        &self,
        path: P,
        offset: u64,
        data: Vec<u8>,
    ) -> std::io::Result<()> {
        // Use regular std::fs to ensure the file exists first
        if !std::path::Path::new(path.as_ref()).exists() {
            std::fs::File::create(path.as_ref())?;
        }

        // Now open with tokio-uring
        let file = File::open(path).await?;

        // Write the data
        let (res, _) = file.write_at(data, offset).await;
        res?;

        Ok(())
    }

    /// Sync a file to disk
    pub async fn sync_file<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        let file = File::open(path).await?;
        file.sync_all().await?;
        Ok(())
    }

    /// Close a file (no-op in this simple version)
    pub async fn close_file<P: AsRef<Path>>(&self, _path: P) -> std::io::Result<()> {
        // No-op in this implementation
        Ok(())
    }
}
