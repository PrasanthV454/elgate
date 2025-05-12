//! End-to-end example
//!
//! This example demonstrates a complete workflow involving:
//! 1. Reading a file from disk
//! 2. Sending it over the network
//! 3. Receiving it on the other end
//! 4. Writing it back to disk
//! All operations use Elgate components.

mod common;
use common::check_io_uring_full_support;
use elgate_core::arch::cpu_info::CpuInfo;
use elgate_core::disk::io_uring::{DiskConfig, DiskEngine};
use elgate_core::net::io_uring::{NetworkConfig, NetworkEngine};
use elgate_core::ring::{RingBuffer, RingBufferOptions};
use std::net::{SocketAddr, TcpListener};
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const TEST_FILE_PATH: &str = "/tmp/elgate_test_source_file";
const OUTPUT_FILE_PATH: &str = "/tmp/elgate_test_output_file";
const RING_PATH_SERVER: &str = "/tmp/elgate_server_ring";
const RING_PATH_CLIENT: &str = "/tmp/elgate_client_ring";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Skip if io_uring not supported
    if !check_io_uring_full_support() {
        println!("Skipping io_uring example - not fully supported in this environment");
        return Ok(());
    }

    println!("Elgate End-to-End Example");
    println!("========================");

    // Detect CPU topology
    let cpu_info = CpuInfo::detect();
    println!(
        "Running on {} logical cores, {} NUMA nodes",
        cpu_info.logical_cores(),
        cpu_info.numa_nodes().len()
    );

    // Clean up from previous runs
    cleanup()?;

    // Create a test file
    create_test_file(TEST_FILE_PATH, 1024 * 1024)?; // 1 MiB file
    println!("Created test file: {}", TEST_FILE_PATH);

    // Start server in a separate task
    let server_handle = tokio::spawn(async {
        if let Err(e) = run_server().await {
            eprintln!("Server error: {}", e);
        }
    });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Run client
    run_client().await?;

    // Wait for server to finish
    let _ = server_handle.await;

    // Verify files match
    verify_files(TEST_FILE_PATH, OUTPUT_FILE_PATH)?;

    // Clean up
    cleanup()?;

    println!("Example completed successfully!");
    Ok(())
}

fn cleanup() -> std::io::Result<()> {
    let _ = std::fs::remove_file(TEST_FILE_PATH);
    let _ = std::fs::remove_file(OUTPUT_FILE_PATH);
    let _ = std::fs::remove_file(RING_PATH_SERVER);
    let _ = std::fs::remove_file(RING_PATH_CLIENT);
    Ok(())
}

fn create_test_file(path: &str, size: usize) -> std::io::Result<()> {
    use std::io::Write;

    let mut file = std::fs::File::create(path)?;
    let mut data = Vec::with_capacity(size);

    // Create predictable pattern for verification
    for i in 0..size {
        data.push((i % 256) as u8);
    }

    file.write_all(&data)?;
    file.sync_all()?;
    Ok(())
}

async fn run_server() -> Result<(), Box<dyn std::error::Error>> {
    println!("Starting server...");

    // Create ring buffer for server
    let ring_options = RingBufferOptions {
        path: PathBuf::from(RING_PATH_SERVER),
        size: 4 * 1024 * 1024, // 4 MiB
        slot_size: 64 * 1024,  // 64 KiB
    };

    let ring = Arc::new(RingBuffer::create(ring_options)?);

    // Create disk engine
    let disk_config = DiskConfig {
        worker_threads: 1,
        pin_threads: false, // Don't pin in example for compatibility
        buffer_size: 64 * 1024,
        numa_node: None,
        #[cfg(feature = "io_uring")]
        queue_depth: 32,
    };

    let disk_engine = DiskEngine::new(disk_config, ring.clone()).await?;

    // Create network engine
    let net_config = NetworkConfig {
        worker_threads: 1,
        pin_threads: false,
        buffer_size: 64 * 1024,
        numa_node: None,
        #[cfg(feature = "io_uring")]
        queue_depth: 32,
    };

    let net_engine = NetworkEngine::new(net_config, ring).await?;

    // Create a TCP listener
    let addr = "127.0.0.1:9876".parse::<SocketAddr>()?;
    let listener = TcpListener::bind(addr)?;
    listener.set_nonblocking(true)?;
    println!("Server listening on {}", addr);

    // Accept connection
    let fd = listener.as_raw_fd();
    let (client_fd, client_addr) = net_engine.accept(fd).await?;
    println!("Accepted connection from {}", client_addr);

    // Read data from client
    let mut all_data = Vec::new();
    loop {
        match net_engine.read(client_fd).await {
            Ok(data) if !data.is_empty() => {
                println!("Received {} bytes", data.len());
                all_data.extend_from_slice(&data);
            }
            Ok(_) => {
                // Empty read means client is done sending
                break;
            }
            Err(e) => {
                eprintln!("Read error: {}", e);
                break;
            }
        }

        // Small pause to avoid busy loop
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    println!("Received total of {} bytes", all_data.len());

    // Write data to output file
    disk_engine
        .write_file(OUTPUT_FILE_PATH, 0, all_data)
        .await?;
    println!("Wrote data to {}", OUTPUT_FILE_PATH);

    // Close connection
    net_engine.close(client_fd).await?;
    println!("Server completed successfully");

    Ok(())
}

async fn run_client() -> Result<(), Box<dyn std::error::Error>> {
    println!("Starting client...");

    // Create ring buffer for client
    let ring_options = RingBufferOptions {
        path: PathBuf::from(RING_PATH_CLIENT),
        size: 4 * 1024 * 1024, // 4 MiB
        slot_size: 64 * 1024,  // 64 KiB
    };

    let ring = Arc::new(RingBuffer::create(ring_options)?);

    // Create disk engine
    let disk_config = DiskConfig {
        worker_threads: 1,
        pin_threads: false,
        buffer_size: 64 * 1024,
        numa_node: None,
        #[cfg(feature = "io_uring")]
        queue_depth: 32,
    };

    let disk_engine = DiskEngine::new(disk_config, ring.clone()).await?;

    // Create network engine
    let net_config = NetworkConfig {
        worker_threads: 1,
        pin_threads: false,
        buffer_size: 64 * 1024,
        numa_node: None,
        #[cfg(feature = "io_uring")]
        queue_depth: 32,
    };

    let net_engine = NetworkEngine::new(net_config, ring).await?;

    // Connect to server
    let server_addr = "127.0.0.1:9876".parse::<SocketAddr>()?;
    let socket_fd = net_engine.connect(server_addr).await?;
    println!("Connected to server");

    // Read file from disk
    let file_data = disk_engine.read_file(TEST_FILE_PATH, 0).await?;
    println!("Read {} bytes from {}", file_data.len(), TEST_FILE_PATH);

    // Send file over network
    let chunk_size = 8192;
    for chunk in file_data.chunks(chunk_size) {
        let bytes_written = net_engine.write(socket_fd, chunk.to_vec()).await?;
        println!("Sent {} bytes", bytes_written);

        // Small pause to avoid flooding
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Close connection
    net_engine.close(socket_fd).await?;
    println!("Client completed successfully");

    Ok(())
}

fn verify_files(original: &str, copy: &str) -> std::io::Result<()> {
    use std::io::Read;

    let mut original_file = std::fs::File::open(original)?;
    let mut copy_file = std::fs::File::open(copy)?;

    let mut original_data = Vec::new();
    let mut copy_data = Vec::new();

    original_file.read_to_end(&mut original_data)?;
    copy_file.read_to_end(&mut copy_data)?;

    if original_data == copy_data {
        println!("Files match! Transfer was successful.");
        Ok(())
    } else {
        let msg = format!(
            "Files don't match! Original: {} bytes, Copy: {} bytes",
            original_data.len(),
            copy_data.len()
        );
        Err(std::io::Error::new(std::io::ErrorKind::Other, msg))
    }
}
