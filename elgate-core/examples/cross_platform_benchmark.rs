//! Cross-platform benchmark example
//!
//! This example compares performance between io_uring and traditional I/O implementations,
//! running both implementations to provide performance comparisons.

use elgate_core::arch::cpu_info::CpuInfo;
use elgate_core::disk::io_uring::{DiskConfig, DiskEngine};
use elgate_core::net::io_uring::{NetworkConfig, NetworkEngine};
use elgate_core::ring::{RingBuffer, RingBufferOptions};
use std::alloc::{alloc, Layout};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::{AsRawFd, IntoRawFd};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::ptr;
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

const TEST_FILE_PATH: &str = "/tmp/elgate_benchmark_test_file";
const RING_PATH: &str = "/tmp/elgate_benchmark_ring";
const FILE_SIZE: usize = 1 * 1024 * 1024; // 1 MiB
const CHUNK_SIZE: usize = 128 * 1024; // 128 KiB - larger chunks for faster processing
const ITERATIONS: usize = 2;

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

    // Try to drop caches for accurate benchmarking
    println!("Attempting to drop filesystem caches...");
    if try_drop_caches() {
        println!("Successfully dropped caches");
    } else {
        println!("Could not drop caches (requires root). Results may be affected by caching");
    }

    // Create ring buffer for io_uring benchmarks
    let ring_options = RingBufferOptions {
        path: PathBuf::from(RING_PATH),
        size: 4 * 1024 * 1024, // 4 MiB
        slot_size: 64 * 1024,  // 64 KiB (larger for performance)
    };

    println!("Creating shared memory ring buffer...");
    let ring = Arc::new(RingBuffer::create(ring_options)?);

    // Print build configuration
    #[cfg(feature = "io_uring")]
    println!("io_uring feature is enabled - will benchmark both implementations");
    #[cfg(not(feature = "io_uring"))]
    println!("io_uring feature is disabled - will only benchmark traditional I/O");

    // Initialize results table
    println!("\n=== BENCHMARK RESULTS ===");
    println!("Operation          | Traditional I/O | io_uring     | Speedup");
    println!("------------------ | --------------- | ------------ | -------");

    // Run traditional disk benchmarks first
    let trad_read = run_traditional_read_benchmark(TEST_FILE_PATH, CHUNK_SIZE, actual_iterations)?;
    try_drop_caches();

    // Always create disk engine for io_uring benchmarks
    let disk_config = DiskConfig {
        #[cfg(feature = "io_uring")]
        queue_depth: 128,
        buffer_size: 128 * 1024, // 128 KiB
    };

    let disk_engine = DiskEngine::new(disk_config, ring.clone()).await?;

    // Run io_uring read benchmark
    let iouring_read =
        run_read_benchmark(&disk_engine, TEST_FILE_PATH, CHUNK_SIZE, actual_iterations).await?;
    try_drop_caches();

    // Calculate and print speedup
    let speedup = if trad_read > 0.0 {
        iouring_read / trad_read
    } else {
        0.0
    };
    println!(
        "Sequential Read    | {:.2} MB/s      | {:.2} MB/s   | {:.2}x",
        trad_read, iouring_read, speedup
    );

    // Run traditional write benchmark
    let trad_write =
        run_traditional_write_benchmark(TEST_FILE_PATH, CHUNK_SIZE, actual_iterations)?;

    // Run io_uring write benchmark
    let iouring_write =
        run_write_benchmark(&disk_engine, TEST_FILE_PATH, CHUNK_SIZE, actual_iterations).await?;

    // Calculate and print speedup
    let speedup = if trad_write > 0.0 {
        iouring_write / trad_write
    } else {
        0.0
    };
    println!(
        "Sequential Write   | {:.2} MB/s      | {:.2} MB/s   | {:.2}x",
        trad_write, iouring_write, speedup
    );

    // Run traditional random access benchmark
    let trad_random =
        run_traditional_random_benchmark(TEST_FILE_PATH, CHUNK_SIZE, actual_iterations)?;

    // Run io_uring random access benchmark
    let iouring_random =
        run_random_access_benchmark(&disk_engine, TEST_FILE_PATH, CHUNK_SIZE, actual_iterations)
            .await?;

    // Calculate and print speedup
    let speedup = if trad_random > 0.0 {
        iouring_random / trad_random
    } else {
        0.0
    };
    println!(
        "Random Access      | {:.2} MB/s      | {:.2} MB/s   | {:.2}x",
        trad_random, iouring_random, speedup
    );

    // Run traditional network benchmark
    let trad_network = run_traditional_network_benchmark(actual_iterations)?;

    // Create a network engine for io_uring network benchmark
    let net_config = NetworkConfig {
        #[cfg(feature = "io_uring")]
        queue_depth: 32,
        buffer_size: 64 * 1024, // 64 KiB
    };

    let net_engine = NetworkEngine::new(net_config, ring.clone()).await?;

    // Run io_uring network benchmark
    let iouring_network = run_network_throughput_benchmark(&net_engine, actual_iterations).await?;

    // Calculate and print speedup
    let speedup = if trad_network > 0.0 {
        iouring_network / trad_network
    } else {
        0.0
    };
    println!(
        "Network Throughput | {:.2} MB/s      | {:.2} MB/s   | {:.2}x",
        trad_network, iouring_network, speedup
    );

    // Clean up
    println!("\nCleaning up...");
    let _ = std::fs::remove_file(TEST_FILE_PATH);
    let _ = std::fs::remove_file(RING_PATH);

    println!("Benchmark completed successfully!");
    Ok(())
}

// Add a function to create aligned buffers for O_DIRECT
fn create_aligned_buffer(size: usize) -> Vec<u8> {
    // O_DIRECT typically requires 512-byte alignment
    const ALIGNMENT: usize = 512;

    // Create an aligned buffer using the allocator
    let layout = Layout::from_size_align(size, ALIGNMENT).expect("Invalid buffer layout");

    let mut vec = Vec::with_capacity(size);
    unsafe {
        let ptr = alloc(layout);
        vec.set_len(size);
        ptr::copy_nonoverlapping(ptr, vec.as_mut_ptr(), size);
    }

    vec
}

// Modify the read function to use aligned buffers with proper fallback
fn run_traditional_read_benchmark(
    path: &str,
    chunk_size: usize,
    iterations: usize,
) -> Result<f64, Box<dyn std::error::Error>> {
    let mut throughputs = Vec::new();

    for i in 0..iterations {
        println!("  Traditional Read: Iteration {}/{}", i + 1, iterations);
        let start = Instant::now();
        let file_size = std::fs::metadata(path)?.len() as usize;
        let file_size = std::cmp::min(file_size, 1 * 1024 * 1024); // Limit to 1MB max
        let mut total_read = 0;

        // Add progress reporting
        let total_chunks = (file_size + chunk_size - 1) / chunk_size;
        println!(
            "    Reading {} chunks of {} bytes each ({}KB total)",
            total_chunks,
            chunk_size,
            file_size / 1024
        );

        // Try O_DIRECT first, if it fails, fall back to standard I/O
        let mut file = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECT)
            .open(path)
        {
            Ok(f) => {
                println!("    Using O_DIRECT for reading");
                f
            }
            Err(e) => {
                println!("    O_DIRECT not supported: {}. Using standard file API", e);
                std::fs::File::open(path)?
            }
        };

        // Create an aligned buffer for O_DIRECT
        let mut buffer = create_aligned_buffer(chunk_size);

        // Advise the kernel that we'll be reading the file sequentially
        #[cfg(target_os = "linux")]
        unsafe {
            libc::posix_fadvise(
                file.as_raw_fd(),
                0,
                0,
                libc::POSIX_FADV_SEQUENTIAL | libc::POSIX_FADV_NOREUSE,
            );
        }

        // Try single read first
        println!("    Attempting to read entire file at once...");
        file.seek(SeekFrom::Start(0))?;
        match file.read(&mut buffer) {
            Ok(n) => {
                total_read = n;
                println!("    Successfully read {} bytes at once", n);
            }
            Err(e) => {
                println!("    Full read failed: {}, falling back to chunked reads", e);

                // Read in chunks
                for offset in (0..file_size).step_by(chunk_size) {
                    file.seek(SeekFrom::Start(offset as u64))?;
                    match file.read(&mut buffer) {
                        Ok(n) => total_read += n,
                        Err(e) => {
                            println!("    Read error at offset {}: {}", offset, e);
                            continue;
                        }
                    }
                }
            }
        }

        let elapsed = start.elapsed();
        let throughput = total_read as f64 / (1024.0 * 1024.0) / elapsed.as_secs_f64();
        throughputs.push(throughput);
        println!(
            "    Read {} KiB in {:.2?} ({:.2} MB/s)",
            total_read / 1024,
            elapsed,
            throughput
        );
    }

    Ok(throughputs.iter().sum::<f64>() / throughputs.len() as f64)
}

// Implement traditional write benchmark
fn run_traditional_write_benchmark(
    path: &str,
    chunk_size: usize,
    iterations: usize,
) -> Result<f64, Box<dyn std::error::Error>> {
    let mut throughputs = Vec::new();

    for i in 0..iterations {
        println!("  Traditional Write: Iteration {}/{}", i + 1, iterations);

        // Create aligned data for O_DIRECT
        let mut data = create_aligned_buffer(chunk_size);
        for i in 0..data.len() {
            data[i] = 0x42; // Fill with same test pattern
        }

        // Create a new file, try with O_DIRECT but fall back if needed
        let iter_path = format!("{}.{}", path, i);
        let mut file = match std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .custom_flags(libc::O_DIRECT)
            .open(&iter_path)
        {
            Ok(f) => {
                println!("    Using O_DIRECT for writing");
                f
            }
            Err(e) => {
                println!("    O_DIRECT not supported: {}. Using standard file API", e);
                std::fs::File::create(&iter_path)?
            }
        };

        for offset in (0..chunk_size).step_by(chunk_size) {
            file.seek(SeekFrom::Start(offset as u64))?;
            match file.write(&data) {
                Ok(n) => {
                    // No need to update total_written here, as we're writing the entire file
                }
                Err(e) => {
                    println!("    Write error at offset {}: {}", offset, e);
                    continue;
                }
            }
        }

        // Make sure data is committed to disk
        println!("    Syncing file to disk...");
        file.sync_all()?;

        let elapsed = start.elapsed();
        let throughput = chunk_size as f64 / (1024.0 * 1024.0) / elapsed.as_secs_f64();
        throughputs.push(throughput);
        println!(
            "    Wrote {} KiB in {:.2?} ({:.2} MB/s)",
            chunk_size / 1024,
            elapsed,
            throughput
        );

        // Clean up
        println!("    Cleaning up iteration file...");
        let _ = std::fs::remove_file(&iter_path);
    }

    Ok(throughputs.iter().sum::<f64>() / throughputs.len() as f64)
}

// Implement traditional random access benchmark
fn run_traditional_random_benchmark(
    path: &str,
    chunk_size: usize,
    iterations: usize,
) -> Result<f64, Box<dyn std::error::Error>> {
    let mut throughputs = Vec::new();

    for i in 0..iterations {
        println!(
            "  Traditional Random Access: Iteration {}/{}",
            i + 1,
            iterations
        );
        let start = Instant::now();
        let file_size = std::fs::metadata(path)?.len() as usize;
        let mut total_read = 0;

        // For random access, just use regular file I/O since O_DIRECT
        // causes too many complications with arbitrary offsets
        println!("    Using standard I/O for random access (O_DIRECT disabled)");
        let mut file = std::fs::File::open(path)?;

        let mut buffer = vec![0u8; chunk_size];

        for _ in 0..100 {
            let offset = rand::random::<u64>() % (file_size as u64);
            file.seek(SeekFrom::Start(offset))?;
            match file.read(&mut buffer) {
                Ok(n) => total_read += n,
                Err(e) => {
                    println!("    Read error at offset {}: {}", offset, e);
                    continue;
                }
            }
        }

        let elapsed = start.elapsed();
        let operations_per_second = 100 as f64 / elapsed.as_secs_f64();
        let throughput = total_read as f64 / (1024.0 * 1024.0) / elapsed.as_secs_f64();
        throughputs.push(throughput);
        println!(
            "    Performed 100 random reads in {:.2?} ({:.2} ops/s, {:.2} MB/s)",
            elapsed, operations_per_second, throughput
        );
    }

    Ok(throughputs.iter().sum::<f64>() / throughputs.len() as f64)
}

// Implement traditional network benchmark
fn run_traditional_network_benchmark(iterations: usize) -> Result<f64, Box<dyn std::error::Error>> {
    let mut throughputs = Vec::new();
    let data_size = 5 * 1024 * 1024; // 5 MiB
    let chunk_size = 64 * 1024; // 64 KiB

    for i in 0..iterations {
        println!("  Traditional Network: Iteration {}/{}", i + 1, iterations);

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
        let mut client = TcpStream::connect(actual_server_addr)?;
        println!("  Connected to echo server");

        // Set non-blocking mode (optional)
        client.set_nonblocking(false)?;

        // Create test data
        let chunk = vec![0x42u8; chunk_size]; // Fill with arbitrary data
        let chunks_to_send = data_size / chunk_size;

        // Send data in chunks
        let mut total_sent = 0;
        let mut total_received = 0;
        let mut buffer = vec![0u8; chunk_size];

        for _ in 0..chunks_to_send {
            // Send chunk
            match client.write_all(&chunk) {
                Ok(_) => {
                    total_sent += chunk.len();
                }
                Err(e) => {
                    println!("    Write error: {}", e);
                    break;
                }
            }

            // Read response
            match client.read(&mut buffer) {
                Ok(n) if n > 0 => {
                    total_received += n;
                }
                Ok(_) => break,
                Err(e) => {
                    println!("    Read error: {}", e);
                    break;
                }
            }
        }

        // Calculate throughput
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
        thread::sleep(Duration::from_millis(100));
    }

    Ok(throughputs.iter().sum::<f64>() / throughputs.len() as f64)
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
        let file_size = std::cmp::min(file_size, 1 * 1024 * 1024); // Limit to 1MB max
        let mut total_read = 0;

        // Add progress reporting
        let total_chunks = (file_size + chunk_size - 1) / chunk_size;
        println!(
            "    Reading {} chunks of {} bytes each ({}KB total)",
            total_chunks,
            chunk_size,
            file_size / 1024
        );
        let mut chunks_read = 0;

        // Read entire file at once for better performance
        println!("    Reading entire file at once...");
        let read_future = disk_engine.read_file(path, 0);
        let timeout_duration = Duration::from_secs(30); // 30 second timeout

        match tokio::time::timeout(timeout_duration, read_future).await {
            Ok(result) => {
                let data = result?;
                total_read = data.len();
                println!("    Successfully read {} bytes at once", total_read);
            }
            Err(_) => {
                println!("    Single read operation timed out, falling back to chunked reads");
                // Fall back to chunked reads
                for offset in (0..file_size).step_by(chunk_size) {
                    // Print progress every chunk
                    chunks_read += 1;
                    println!("    Reading chunk {}/{}", chunks_read, total_chunks);

                    // Add a timeout to each read operation
                    let read_future = disk_engine.read_file(path, offset as u64);
                    let timeout_duration = Duration::from_secs(10); // 10 second timeout

                    let data = match tokio::time::timeout(timeout_duration, read_future).await {
                        Ok(result) => result?,
                        Err(_) => {
                            println!("    Read operation timed out at offset {}", offset);
                            // Skip remaining chunks if we hit a timeout
                            println!("    Skipping remaining chunks due to timeout");
                            break;
                        }
                    };
                    total_read += data.len();
                }
            }
        }

        let elapsed = start.elapsed();
        let throughput = total_read as f64 / (1024.0 * 1024.0) / elapsed.as_secs_f64();
        throughputs.push(throughput);
        println!(
            "    Read {} KiB in {:.2?} ({:.2} MB/s)",
            total_read / 1024,
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

        // Try to batch read operations by opening file just once
        println!("    Opening file for random access reads...");
        // We'll use the path directly in the disk_engine.read_file calls

        // Use a longer timeout for the entire batch
        let timeout_duration = Duration::from_secs(30); // 30 second timeout

        // Initialize a vector to store futures
        let mut read_futures = Vec::with_capacity(offsets.len());

        for &offset in &offsets {
            // Queue up the read operation
            read_futures.push(disk_engine.read_file(path, offset));
        }

        // Process all reads with a single timeout for the batch
        println!("    Processing {} read operations...", read_futures.len());
        match tokio::time::timeout(timeout_duration, futures::future::join_all(read_futures)).await
        {
            Ok(results) => {
                for (i, result) in results.into_iter().enumerate() {
                    match result {
                        Ok(data) => total_read += data.len(),
                        Err(e) => println!("    Error reading at offset {}: {}", offsets[i], e),
                    }

                    // Track progress
                    reads_completed += 1;
                    if reads_completed % 10 == 0 || reads_completed == offsets.len() {
                        println!(
                            "    Completed {}/{} random reads",
                            reads_completed,
                            offsets.len()
                        );
                    }
                }
            }
            Err(_) => {
                println!(
                    "    Batch read operation timed out after {} seconds",
                    timeout_duration.as_secs()
                );
            }
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
    let data_size = 5 * 1024 * 1024; // 5 MiB - much smaller for CI
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

// Add a function to drop caches if possible
fn try_drop_caches() -> bool {
    #[cfg(target_os = "linux")]
    {
        // Check if we can drop caches (requires root or proper permissions)
        let output = Command::new("sh")
            .arg("-c")
            .arg("echo 3 | sudo -n tee /proc/sys/vm/drop_caches > /dev/null 2>&1")
            .output();

        match output {
            Ok(out) => out.status.success(),
            Err(_) => false,
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}
