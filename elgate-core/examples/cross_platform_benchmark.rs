//! Cross-platform benchmark example
//!
//! This example compares performance between io_uring and fallback implementations,
//! running the appropriate implementation based on platform support.

use elgate_core::arch::cpu_info::CpuInfo;
use elgate_core::disk::io_uring::{DiskConfig, DiskEngine};
use elgate_core::net::io_uring::{NetworkConfig, NetworkEngine};
use elgate_core::ring::{RingBuffer, RingBufferOptions};
use std::env;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::io::IntoRawFd;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

const TEST_FILE_PATH: &str = "/tmp/elgate_benchmark_test_file";
const RING_PATH: &str = "/tmp/elgate_benchmark_ring";
const FILE_SIZE: usize = 10 * 1024 * 1024; // 10 MiB
const CHUNK_SIZE: usize = 32 * 1024; // 32 KiB
const ITERATIONS: usize = 3;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Use tokio_uring's runtime instead of tokio
    tokio_uring::start(async {
        if let Err(e) = run_benchmark().await {
            eprintln!("Benchmark error: {}", e);
        }
    });

    Ok(())
}

async fn run_benchmark() -> Result<(), Box<dyn std::error::Error>> {
    println!("Elgate Cross-Platform I/O Benchmark");
    println!("===================================");

    // Adjust parameters for CI
    let (actual_file_size, actual_iterations) = (FILE_SIZE, ITERATIONS);

    println!(
        "File size: {} MiB, Iterations: {}",
        actual_file_size / (1024 * 1024),
        actual_iterations
    );

    // Detect CPU topology
    let cpu_info = CpuInfo::detect();
    println!(
        "Running on {} logical cores, {} NUMA nodes",
        cpu_info.logical_cores(),
        cpu_info.numa_nodes().len()
    );

    // Clean up from previous runs
    let _ = std::fs::remove_file(TEST_FILE_PATH);
    let _ = std::fs::remove_file(RING_PATH);

    // Create a test file with random data
    create_test_file(TEST_FILE_PATH, actual_file_size)?;

    // Create ring buffer
    let ring_options = RingBufferOptions {
        path: PathBuf::from(RING_PATH),
        size: 4 * 1024 * 1024, // 4 MiB
        slot_size: 64 * 1024,  // 64 KiB (larger for performance)
    };

    println!("Creating shared memory ring buffer...");
    let ring = Arc::new(RingBuffer::create(ring_options)?);

    // Print build configuration
    #[cfg(feature = "io_uring")]
    println!("Using io_uring implementation");
    #[cfg(not(feature = "io_uring"))]
    println!("Using fallback implementation");

    // Run disk benchmarks
    println!("\n=== DISK I/O BENCHMARKS ===");

    // Create disk engine
    let disk_config = DiskConfig {
        #[cfg(feature = "io_uring")]
        queue_depth: 128,
        buffer_size: 128 * 1024, // 128 KiB
    };

    let disk_engine = DiskEngine::new(disk_config, ring.clone()).await?;

    // Run disk benchmarks
    println!("\nSequential Read Benchmark:");
    let read_throughput =
        run_read_benchmark(&disk_engine, TEST_FILE_PATH, CHUNK_SIZE, actual_iterations).await?;
    println!("Average read throughput: {:.2} MB/s", read_throughput);

    // Run write benchmark
    println!("\nSequential Write Benchmark:");
    let write_throughput =
        run_write_benchmark(&disk_engine, TEST_FILE_PATH, CHUNK_SIZE, actual_iterations).await?;
    println!("Average write throughput: {:.2} MB/s", write_throughput);

    // Run random access benchmark
    println!("\nRandom Access Benchmark:");
    let random_throughput =
        run_random_access_benchmark(&disk_engine, TEST_FILE_PATH, CHUNK_SIZE, actual_iterations)
            .await?;
    println!(
        "Average random access throughput: {:.2} MB/s",
        random_throughput
    );

    // Allow time for cleanup
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Run network benchmarks
    println!("\n=== NETWORK I/O BENCHMARKS ===");

    // Create a new network engine
    let net_config = NetworkConfig {
        #[cfg(feature = "io_uring")]
        queue_depth: 32,
        buffer_size: 64 * 1024, // 64 KiB
    };

    let net_engine = NetworkEngine::new(net_config, ring.clone()).await?;

    // Run network throughput benchmark
    println!("\nNetwork Throughput Benchmark:");
    let net_throughput = run_network_throughput_benchmark(&net_engine, 3).await?;
    println!("Average network throughput: {:.2} MB/s", net_throughput);

    // Clean up
    println!("\nCleaning up...");
    let _ = std::fs::remove_file(TEST_FILE_PATH);
    let _ = std::fs::remove_file(RING_PATH);

    println!("Benchmark completed successfully!");
    Ok(())
}

fn create_test_file(path: &str, size: usize) -> std::io::Result<()> {
    use std::io::Write;

    let mut file = std::fs::File::create(path)?;
    let chunk_size = 1024 * 1024; // 1 MiB chunks for creation
    let mut remaining = size;

    // Generate some random data
    let mut buffer = vec![0u8; chunk_size];

    while remaining > 0 {
        let to_write = std::cmp::min(remaining, chunk_size);

        // Fill buffer with random data
        for i in 0..to_write {
            buffer[i] = rand::random::<u8>();
        }

        file.write_all(&buffer[0..to_write])?;
        remaining -= to_write;
    }

    file.sync_all()?;
    Ok(())
}

async fn run_read_benchmark(
    disk_engine: &DiskEngine,
    path: &str,
    chunk_size: usize,
    iterations: usize,
) -> Result<f64, Box<dyn std::error::Error>> {
    let mut throughputs = Vec::new();

    for i in 0..iterations {
        println!("  Iteration {}/{}", i + 1, iterations);
        let start = Instant::now();
        let file_size = std::fs::metadata(path)?.len() as usize;
        let mut total_read = 0;

        // Add progress reporting
        let total_chunks = (file_size + chunk_size - 1) / chunk_size;
        println!(
            "    Reading {} chunks of {} bytes each",
            total_chunks, chunk_size
        );
        let mut chunks_read = 0;

        for offset in (0..file_size).step_by(chunk_size) {
            // Print progress every few chunks
            chunks_read += 1;
            if chunks_read % 10 == 0 || chunks_read == total_chunks {
                println!("    Progress: {}/{} chunks", chunks_read, total_chunks);
            }

            // Add a timeout to each read operation
            let read_future = disk_engine.read_file(path, offset as u64);
            let timeout_duration = Duration::from_secs(10); // 10 second timeout

            let data = match tokio::time::timeout(timeout_duration, read_future).await {
                Ok(result) => result?,
                Err(_) => {
                    println!("    Read operation timed out at offset {}", offset);
                    // Continue with next chunk instead of hanging
                    continue;
                }
            };
            total_read += data.len();
        }

        let elapsed = start.elapsed();
        let throughput = total_read as f64 / (1024.0 * 1024.0) / elapsed.as_secs_f64();
        throughputs.push(throughput);
        println!(
            "    Read {} MiB in {:.2?} ({:.2} MB/s)",
            total_read / (1024 * 1024),
            elapsed,
            throughput
        );
    }

    Ok(throughputs.iter().sum::<f64>() / throughputs.len() as f64)
}

async fn run_write_benchmark(
    disk_engine: &DiskEngine,
    path: &str,
    chunk_size: usize,
    iterations: usize,
) -> Result<f64, Box<dyn std::error::Error>> {
    let mut throughputs = Vec::new();

    for i in 0..iterations {
        println!("  Iteration {}/{}", i + 1, iterations);
        // Create a new path for each iteration to avoid caching effects
        let iter_path = format!("{}.{}", path, i);
        let start = Instant::now();
        let file_size = std::fs::metadata(path)?.len() as usize;
        let mut total_written = 0;

        // Add progress reporting
        let total_chunks = (file_size + chunk_size - 1) / chunk_size;
        println!(
            "    Writing {} chunks of {} bytes each",
            total_chunks, chunk_size
        );
        let mut chunks_written = 0;

        // Generate data to write
        let data = vec![0x42; chunk_size]; // Fill with arbitrary data

        for offset in (0..file_size).step_by(chunk_size) {
            // Print progress every few chunks
            chunks_written += 1;
            if chunks_written % 10 == 0 || chunks_written == total_chunks {
                println!("    Progress: {}/{} chunks", chunks_written, total_chunks);
            }

            // Add a timeout to each write operation
            let write_future = disk_engine.write_file(&iter_path, offset as u64, data.clone());
            let timeout_duration = Duration::from_secs(10); // 10 second timeout

            match tokio::time::timeout(timeout_duration, write_future).await {
                Ok(result) => result?,
                Err(_) => {
                    println!("    Write operation timed out at offset {}", offset);
                    // Continue with next chunk instead of hanging
                    continue;
                }
            };
            total_written += chunk_size;
        }

        // Make sure data is committed to disk
        println!("    Syncing file to disk...");
        let sync_future = disk_engine.sync_file(&iter_path);
        match tokio::time::timeout(Duration::from_secs(5), sync_future).await {
            Ok(result) => result?,
            Err(_) => {
                println!("    Sync operation timed out");
            }
        };

        let elapsed = start.elapsed();
        let throughput = total_written as f64 / (1024.0 * 1024.0) / elapsed.as_secs_f64();
        throughputs.push(throughput);
        println!(
            "    Wrote {} MiB in {:.2?} ({:.2} MB/s)",
            total_written / (1024 * 1024),
            elapsed,
            throughput
        );

        // Clean up
        println!("    Cleaning up iteration file...");
        let _ = std::fs::remove_file(&iter_path);
    }

    Ok(throughputs.iter().sum::<f64>() / throughputs.len() as f64)
}

async fn run_random_access_benchmark(
    disk_engine: &DiskEngine,
    path: &str,
    chunk_size: usize,
    iterations: usize,
) -> Result<f64, Box<dyn std::error::Error>> {
    let mut throughputs = Vec::new();

    for i in 0..iterations {
        println!("  Iteration {}/{}", i + 1, iterations);
        let start = Instant::now();
        let file_size = std::fs::metadata(path)?.len() as usize;
        let mut total_read = 0;

        // Generate random offsets
        let max_offset = file_size.saturating_sub(chunk_size);
        let offsets: Vec<u64> = (0..100)
            .map(|_| rand::random::<u64>() % (max_offset as u64))
            .collect();

        println!("    Performing {} random reads", offsets.len());
        let mut reads_completed = 0;

        for &offset in &offsets {
            // Print progress
            reads_completed += 1;
            if reads_completed % 10 == 0 || reads_completed == offsets.len() {
                println!(
                    "    Progress: {}/{} random reads",
                    reads_completed,
                    offsets.len()
                );
            }

            // Add a timeout
            let read_future = disk_engine.read_file(path, offset);
            let timeout_duration = Duration::from_secs(5); // 5 second timeout

            let data = match tokio::time::timeout(timeout_duration, read_future).await {
                Ok(result) => result?,
                Err(_) => {
                    println!("    Random read operation timed out at offset {}", offset);
                    // Continue with next offset instead of hanging
                    continue;
                }
            };
            total_read += data.len();
        }

        let elapsed = start.elapsed();
        let operations_per_second = offsets.len() as f64 / elapsed.as_secs_f64();
        let throughput = total_read as f64 / (1024.0 * 1024.0) / elapsed.as_secs_f64();
        throughputs.push(throughput);
        println!(
            "    Performed {} random reads in {:.2?} ({:.2} ops/s, {:.2} MB/s)",
            offsets.len(),
            elapsed,
            operations_per_second,
            throughput
        );
    }

    Ok(throughputs.iter().sum::<f64>() / throughputs.len() as f64)
}

async fn run_network_throughput_benchmark(
    net_engine: &NetworkEngine,
    iterations: usize,
) -> Result<f64, Box<dyn std::error::Error>> {
    let mut throughputs = Vec::new();
    let data_size = 50 * 1024 * 1024; // 50 MiB
    let chunk_size = 64 * 1024; // 64 KiB per write

    for i in 0..iterations {
        println!("  Iteration {}/{}", i + 1, iterations);

        // Set up channels for coordination
        let (server_ready_tx, server_ready_rx) = mpsc::channel();

        // Start echo server in a separate thread
        let server_addr = "127.0.0.1:0".parse::<SocketAddr>().unwrap();
        let listener = TcpListener::bind(server_addr)?;
        let actual_server_addr = listener.local_addr()?;
        println!("  Starting echo server at {}", actual_server_addr);

        let server_thread = thread::spawn(move || {
            println!("  Server listening...");

            // Signal that we're ready
            server_ready_tx.send(()).unwrap();

            if let Ok((mut stream, client_addr)) = listener.accept() {
                println!("  Server accepted connection from {}", client_addr);

                // Echo back data in chunks
                let mut buffer = vec![0u8; chunk_size];
                let mut total_echoed = 0;

                loop {
                    match stream.read(&mut buffer) {
                        Ok(0) => break, // End of stream
                        Ok(n) => {
                            if let Err(e) = stream.write_all(&buffer[0..n]) {
                                eprintln!("  Server write error: {}", e);
                                break;
                            }
                            total_echoed += n;
                        }
                        Err(e) => {
                            eprintln!("  Server read error: {}", e);
                            break;
                        }
                    }
                }

                println!("  Server echoed {} bytes total", total_echoed);
            } else {
                eprintln!("  Server accept error");
            }
        });

        // Wait for server to be ready
        server_ready_rx.recv().unwrap();

        // Connect to the echo server
        let start = Instant::now();
        let socket_fd = net_engine.connect(actual_server_addr).await?;
        println!("  Connected to echo server");

        // Create test data
        let chunk = vec![0x42u8; chunk_size]; // Fill with arbitrary data
        let chunks_to_send = data_size / chunk_size;

        // Send data in chunks
        let mut total_sent = 0;
        let mut total_received = 0;

        for _ in 0..chunks_to_send {
            // Send chunk
            let bytes_written = net_engine.write(socket_fd, chunk.clone()).await?;
            total_sent += bytes_written;

            // Read echo response
            let data = net_engine.read(socket_fd).await?;
            total_received += data.len();
        }

        // Close connection
        net_engine.close(socket_fd).await?;

        // Calculate throughput (consider both directions)
        let elapsed = start.elapsed();
        let throughput =
            (total_sent + total_received) as f64 / (1024.0 * 1024.0) / elapsed.as_secs_f64();
        throughputs.push(throughput);

        println!(
            "    Transferred {} MiB in {:.2?} ({:.2} MB/s bidirectional)",
            (total_sent + total_received) / (1024 * 1024),
            elapsed,
            throughput
        );

        // Wait for server to complete
        if let Err(e) = server_thread.join() {
            eprintln!("  Server thread join error: {:?}", e);
        }

        // Short delay between iterations
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    Ok(throughputs.iter().sum::<f64>() / throughputs.len() as f64)
}
