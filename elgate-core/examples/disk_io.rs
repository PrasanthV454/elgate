//! Disk I/O example for Elgate.
//!
//! This example demonstrates how to use the disk I/O engine for
//! high-performance file operations.

use elgate_core::disk::io_uring::{DiskConfig, DiskEngine};
use elgate_core::ring::{RingBuffer, RingBufferOptions};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create a shared memory ring buffer
    let ring_path = "/tmp/elgate_example_disk_ring";
    let test_file_path = "/tmp/elgate_example_disk_file";

    // Clean up from previous runs
    let _ = fs::remove_file(ring_path);
    let _ = fs::remove_file(test_file_path);

    // Create ring buffer
    let ring_options = RingBufferOptions {
        path: PathBuf::from(ring_path),
        size: 1024 * 1024, // 1 MiB
        slot_size: 4096,   // 4 KiB
    };

    println!("Creating shared memory ring buffer...");
    let ring = Arc::new(RingBuffer::create(ring_options)?);

    // Create disk engine
    let config = DiskConfig {
        worker_threads: 2,
        pin_threads: true,
        #[cfg(feature = "io_uring")]
        queue_depth: 32,
        buffer_size: 4096,
        numa_node: None,
    };

    println!("Initializing disk I/O engine...");
    let disk_engine = DiskEngine::new(config, ring.clone()).await?;

    // Write data to a file
    let content = "Hello, Elgate disk I/O engine!";
    println!("Writing to file: {}", test_file_path);
    disk_engine
        .write_file(test_file_path, 0, content.as_bytes().to_vec())
        .await?;

    // Read the data back
    println!("Reading from file...");
    let read_data = disk_engine.read_file(test_file_path, 0).await?;

    // Print the read data
    let read_str = String::from_utf8_lossy(&read_data);
    println!("Read: {}", read_str);

    // Write multiple chunks to demonstrate offset writing
    println!("Writing multiple chunks to file...");
    for i in 0..5 {
        let offset = 100 + (i * 50);
        let chunk = format!("Chunk {} at offset {}", i, offset);
        disk_engine
            .write_file(test_file_path, offset as u64, chunk.as_bytes().to_vec())
            .await?;
    }

    // Read back each chunk
    println!("Reading back chunks...");
    for i in 0..5 {
        let offset = 100 + (i * 50);
        let read_data = disk_engine.read_file(test_file_path, offset as u64).await?;
        let read_str = String::from_utf8_lossy(&read_data);
        println!("  Offset {}: {}", offset, read_str.trim_end_matches('\0'));
    }

    // Sync and close
    println!("Syncing file to disk...");
    disk_engine.sync_file(test_file_path).await?;

    println!("Closing file...");
    disk_engine.close_file(test_file_path).await?;

    // Clean up
    println!("Cleaning up...");
    let _ = fs::remove_file(ring_path);
    let _ = fs::remove_file(test_file_path);

    println!("Disk I/O example completed successfully!");
    Ok(())
}
