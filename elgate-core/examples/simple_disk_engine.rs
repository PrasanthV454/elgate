//! A simplified disk engine for examples
//!
//! This version avoids the thread context issues by using tokio-uring directly
//! without spawning additional worker threads.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use tokio_uring::fs::File;

/// A simplified disk engine that works directly with tokio-uring
/// without creating separate worker threads
pub struct SimpleDiskEngine {
    // Cache open file handles to prevent Bad file descriptor errors
    files: Mutex<HashMap<String, File>>,
}

impl SimpleDiskEngine {
    /// Create a new instance of the simple disk engine
    pub fn new() -> Self {
        Self {
            files: Mutex::new(HashMap::new()),
        }
    }

    /// Read from a file at the specified offset
    pub async fn read_file<P: AsRef<Path>>(
        &self,
        path: P,
        offset: u64,
    ) -> std::io::Result<Vec<u8>> {
        // Get or open the file
        let path_str = path.as_ref().to_string_lossy().to_string();
        let file = self.get_or_open_file(&path_str).await?;

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
        // Get or create the file
        let path_str = path.as_ref().to_string_lossy().to_string();
        let file = self.get_or_create_file(&path_str).await?;

        // Write the data
        let (res, _) = file.write_at(data, offset).await;
        res?;

        Ok(())
    }

    /// Sync a file to disk
    pub async fn sync_file<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        let path_str = path.as_ref().to_string_lossy().to_string();
        let file = self.get_or_open_file(&path_str).await?;
        file.sync_all().await?;
        Ok(())
    }

    /// Close a file (no-op in this simple version)
    pub async fn close_file<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        // Actually close the file by removing it from our cache
        let path_str = path.as_ref().to_string_lossy().to_string();
        let mut files = self.files.lock().unwrap();
        files.remove(&path_str);
        Ok(())
    }

    /// Helper to get or open a file
    async fn get_or_open_file(&self, path: &str) -> std::io::Result<File> {
        let mut files = self.files.lock().unwrap();

        if !files.contains_key(path) {
            let file = File::open(path).await?;
            files.insert(path.to_string(), file);
        }

        // tokio-uring::fs::File doesn't have try_clone, so we need to open a new one
        // This is safe since the file is already open in our cache
        File::open(path).await
    }

    /// Helper to get or create a file
    async fn get_or_create_file(&self, path: &str) -> std::io::Result<File> {
        let mut files = self.files.lock().unwrap();

        if !files.contains_key(path) {
            let file = match File::open(path).await {
                Ok(file) => file,
                Err(_) => File::create(path).await?,
            };
            files.insert(path.to_string(), file);
        }

        // Instead of cloning the file (which tokio-uring doesn't support),
        // open a new handle to the same file
        match File::open(path).await {
            Ok(file) => Ok(file),
            Err(_) => File::create(path).await, // Fallback if file was removed
        }
    }
}
