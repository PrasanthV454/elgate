//! Disk I/O example for Elgate.
//!
//! This example demonstrates how to use the disk I/O engine for
//! high-performance file operations.

mod common;
use common::check_io_uring_full_support;
mod simple_disk_engine;
use elgate_core::ring::{RingBuffer, RingBufferOptions};
use simple_disk_engine::SimpleDiskEngine;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Use tokio_uring's runtime instead of tokio
    tokio_uring::start(async {
        // Skip if io_uring not supported
        if !check_io_uring_full_support() {
            println!("Skipping io_uring example - not fully supported in this environment");
            return;
        }

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
        let ring = match RingBuffer::create(ring_options) {
            Ok(ring) => Arc::new(ring),
            Err(e) => {
                println!("Failed to create ring buffer: {}", e);
                return;
            }
        };

        // Use the simple disk engine
        let disk_engine = SimpleDiskEngine::new();

        // Write data to a file
        let content = "Hello, Elgate disk I/O engine!";
        println!("Writing to file: {}", test_file_path);
        if let Err(e) = disk_engine
            .write_file(test_file_path, 0, content.as_bytes().to_vec())
            .await
        {
            println!("Failed to write to file: {}", e);
            return;
        }

        // Read the data back
        println!("Reading from file...");
        let read_data = match disk_engine.read_file(test_file_path, 0).await {
            Ok(data) => data,
            Err(e) => {
                println!("Failed to read from file: {}", e);
                return;
            }
        };

        // Print the read data
        let read_str = String::from_utf8_lossy(&read_data);
        println!("Read: {}", read_str);

        // Write multiple chunks to demonstrate offset writing
        println!("Writing multiple chunks to file...");
        for i in 0..5 {
            let offset = 100 + (i * 50);
            let chunk = format!("Chunk {} at offset {}", i, offset);
            if let Err(e) = disk_engine
                .write_file(test_file_path, offset as u64, chunk.as_bytes().to_vec())
                .await
            {
                println!("Failed to write chunk {}: {}", i, e);
                return;
            }
        }

        // Read back each chunk
        println!("Reading back chunks...");
        for i in 0..5 {
            let offset = 100 + (i * 50);
            let read_data = match disk_engine.read_file(test_file_path, offset as u64).await {
                Ok(data) => data,
                Err(e) => {
                    println!("Failed to read chunk {}: {}", i, e);
                    return;
                }
            };
            let read_str = String::from_utf8_lossy(&read_data);
            println!("  Offset {}: {}", offset, read_str.trim_end_matches('\0'));
        }

        // Sync and close
        println!("Syncing file to disk...");
        if let Err(e) = disk_engine.sync_file(test_file_path).await {
            println!("Failed to sync file: {}", e);
            return;
        }

        println!("Closing file...");
        if let Err(e) = disk_engine.close_file(test_file_path).await {
            println!("Failed to close file: {}", e);
            return;
        }

        // Clean up
        println!("Cleaning up...");
        let _ = fs::remove_file(ring_path);
        let _ = fs::remove_file(test_file_path);

        println!("Disk I/O example completed successfully!");
    });

    Ok(())
}
