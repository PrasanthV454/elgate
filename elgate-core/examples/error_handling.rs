//! Error handling example for Elgate.
//!
//! This example demonstrates how to properly handle errors
//! in the Elgate framework.

use elgate_core::disk::io_uring::{DiskConfig, DiskEngine};
use elgate_core::net::io_uring::{NetworkConfig, NetworkEngine};
use elgate_core::ring::{RingBuffer, RingBufferOptions};
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("Elgate Error Handling Examples");

    // Create a shared memory ring buffer
    let ring_path = "/tmp/elgate_example_error_ring";

    // Clean up from previous runs
    let _ = fs::remove_file(ring_path);

    // Create ring buffer
    let ring_options = RingBufferOptions {
        path: PathBuf::from(ring_path),
        size: 1024 * 1024, // 1 MiB
        slot_size: 4096,   // 4 KiB
    };

    println!("\nCreating shared memory ring buffer...");
    let ring = Arc::new(RingBuffer::create(ring_options)?);

    // --------------------------------------------------------
    // Example 1: Handling Disk I/O Errors
    // --------------------------------------------------------
    println!("\n=== Disk I/O Error Handling ===");

    // Create disk engine
    let disk_config = DiskConfig {
        worker_threads: 1,
        pin_threads: false,
        #[cfg(feature = "io_uring")]
        queue_depth: 32,
        buffer_size: 4096,
        numa_node: None,
    };

    println!("Initializing disk I/O engine...");
    let disk_engine = DiskEngine::new(disk_config, ring.clone()).await?;

    // Example 1: Try to read a non-existent file
    println!("\nAttempting to read a non-existent file...");
    let non_existent_file = "/tmp/this_file_does_not_exist_elgate_test";
    let _ = fs::remove_file(non_existent_file); // Ensure it doesn't exist

    match disk_engine.read_file(non_existent_file, 0).await {
        Ok(_) => println!("Unexpected success!"),
        Err(e) => {
            println!("Got expected error: {}", e);
            println!("Error chain:");
            let mut source = e.source();
            let mut indent = String::from("  ");
            while let Some(err) = source {
                println!("{}Caused by: {}", indent, err);
                source = err.source();
                indent = format!("{}  ", indent);
            }
        }
    }

    // Example 2: Try to write with invalid offset
    println!("\nAttempting to write with an extremely large offset...");
    let test_file = "/tmp/elgate_test_error_file";
    match disk_engine
        .write_file(test_file, u64::MAX - 10, b"test".to_vec())
        .await
    {
        Ok(_) => println!("Unexpected success!"),
        Err(e) => println!("Got expected error: {}", e),
    }

    // --------------------------------------------------------
    // Example 2: Handling Network I/O Errors
    // --------------------------------------------------------
    println!("\n=== Network I/O Error Handling ===");

    // Create network engine
    let net_config = NetworkConfig {
        worker_threads: 1,
        pin_threads: false,
        #[cfg(feature = "io_uring")]
        queue_depth: 32,
        buffer_size: 4096,
        numa_node: None,
    };

    println!("Initializing network I/O engine...");
    let net_engine = NetworkEngine::new(net_config, ring.clone()).await?;

    // Example 1: Try to connect to a non-existent server
    println!("\nAttempting to connect to a non-existent server...");
    let invalid_addr = "127.0.0.1:1".parse::<SocketAddr>()?; // Port 1 should not be listening

    match net_engine.connect(invalid_addr).await {
        Ok(_) => println!("Unexpected success!"),
        Err(e) => {
            println!("Got expected error: {}", e);
            println!("Error type: {}", std::any::type_name_of_val(&e));
        }
    }

    // Example 2: Try to read from an invalid file descriptor
    println!("\nAttempting to read from an invalid file descriptor...");
    let invalid_fd = 99999; // This FD should not exist

    match net_engine.read(invalid_fd).await {
        Ok(_) => println!("Unexpected success!"),
        Err(e) => println!("Got expected error: {}", e),
    }

    // --------------------------------------------------------
    // Example 3: Proper error propagation
    // --------------------------------------------------------
    println!("\n=== Error Propagation ===");

    // Use the ? operator for propagation
    println!("Demonstrating proper error propagation with the ? operator");

    let result = async {
        // This function will return an error
        let fd = net_engine.connect(invalid_addr).await?;
        println!("This won't be printed");
        Ok::<_, anyhow::Error>(fd)
    }
    .await;

    match result {
        Ok(_) => println!("Unexpected success!"),
        Err(e) => println!("Propagated error correctly: {}", e),
    }

    // Clean up
    println!("\nCleaning up...");
    let _ = fs::remove_file(ring_path);
    let _ = fs::remove_file(test_file);

    println!("\nError handling example completed successfully!");
    Ok(())
}
