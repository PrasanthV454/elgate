//! Network I/O example for Elgate.
//!
//! This example demonstrates how to use the network I/O engine for
//! high-performance socket operations.

use elgate_core::net::io_uring::{NetworkConfig, NetworkEngine};
use elgate_core::ring::{RingBuffer, RingBufferOptions};
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::os::unix::io::IntoRawFd;
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Use tokio-uring runtime instead of tokio runtime
    tokio_uring::start(async {
        if let Err(e) = run_example().await {
            eprintln!("Error in example: {}", e);
        }
    });

    Ok(())
}

async fn run_example() -> Result<(), Box<dyn std::error::Error>> {
    // Create a shared memory ring buffer
    let ring_path = "/tmp/elgate_example_net_ring";

    // Clean up from previous runs
    let _ = fs::remove_file(ring_path);

    // Create ring buffer
    let ring_options = RingBufferOptions {
        path: PathBuf::from(ring_path),
        size: 1024 * 1024, // 1 MiB
        slot_size: 4096,   // 4 KiB
    };

    println!("Creating shared memory ring buffer...");
    let ring = Arc::new(RingBuffer::create(ring_options)?);

    // Create network engine with simplified configuration
    let config = NetworkConfig {
        #[cfg(feature = "io_uring")]
        queue_depth: 32,
        buffer_size: 4096,
    };

    println!("Initializing network I/O engine...");
    let net_engine = NetworkEngine::new(config.clone(), ring.clone()).await?;

    // Run client example first
    println!("\n--- Running client example ---");
    if let Err(e) = run_client_example(&net_engine).await {
        eprintln!("Client example failed: {}", e);
    }

    // Wait a moment to ensure everything is cleaned up
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Create a new engine instance for the server example to avoid any resource conflicts
    println!("Reinitializing network engine for server example...");
    let net_engine_server = NetworkEngine::new(config.clone(), ring.clone()).await?;

    // Then run server example
    println!("\n--- Running server example ---");
    if let Err(e) = run_server_example(&net_engine_server).await {
        eprintln!("Server example failed: {}", e);
    }

    // Clean up
    println!("Cleaning up...");
    let _ = fs::remove_file(ring_path);

    println!("Network I/O example completed successfully!");
    Ok(())
}

async fn run_client_example(net_engine: &NetworkEngine) -> Result<(), Box<dyn std::error::Error>> {
    // Set up a channel for server-client communication
    let (server_ready_tx, server_ready_rx) = mpsc::channel();

    // Start an echo server in a separate thread
    let server_addr = "127.0.0.1:50000".parse::<SocketAddr>().unwrap();
    let listener = TcpListener::bind(server_addr)?;
    let actual_server_addr = listener.local_addr()?;
    println!("Starting echo server at {}", actual_server_addr);

    let server_thread = thread::spawn(move || {
        println!("Server listening...");

        // Signal that the server is ready to accept connections
        server_ready_tx.send(()).unwrap();

        match listener.accept() {
            Ok((mut stream, client_addr)) => {
                println!("Server accepted connection from {}", client_addr);

                // Read data from client
                let mut buffer = [0u8; 1024];
                match stream.read(&mut buffer) {
                    Ok(n) if n > 0 => {
                        println!("Server received {} bytes", n);

                        // Echo data back
                        if let Err(e) = stream.write_all(&buffer[0..n]) {
                            eprintln!("Server write error: {}", e);
                            return;
                        }
                        println!("Server echoed data back");
                    }
                    Ok(_) => println!("Client closed connection"),
                    Err(e) => eprintln!("Server read error: {}", e),
                }
            }
            Err(e) => eprintln!("Accept error: {}", e),
        }
    });

    // Wait for server to be ready
    server_ready_rx.recv().unwrap();
    println!("Server signaled it's ready");

    // Connect to the server
    println!("Connecting to echo server...");
    let socket_fd = net_engine.connect(actual_server_addr).await?;
    println!("Connected, socket fd: {}", socket_fd);

    // Write data to the server
    let message = "Hello, Elgate network engine!";
    println!("Sending message: {}", message);
    let bytes_written = net_engine
        .write(socket_fd, message.as_bytes().to_vec())
        .await?;
    println!("Sent {} bytes", bytes_written);

    // Read the echo response
    println!("Reading response...");
    let read_data = net_engine.read(socket_fd).await?;
    let response = String::from_utf8_lossy(&read_data);
    println!("Received response: {}", response);

    // Close the socket
    println!("Closing connection...");
    net_engine.close(socket_fd).await?;

    // Wait for the server to complete
    if let Err(e) = server_thread.join() {
        eprintln!("Server thread join error: {:?}", e);
    }

    Ok(())
}

async fn run_server_example(net_engine: &NetworkEngine) -> Result<(), Box<dyn std::error::Error>> {
    // Set up a channel for client notification
    let (server_ready_tx, server_ready_rx) = mpsc::channel();
    // Set up a separate channel for the file descriptor
    let (fd_tx, fd_rx) = mpsc::channel();

    // Create a TCP listener
    let server_addr = "127.0.0.1:40000".parse::<SocketAddr>().unwrap();

    // Start a listener thread that will coordinate with the network engine
    let server_thread = thread::spawn(move || {
        // Create the listener in this thread
        match TcpListener::bind(server_addr) {
            Ok(listener) => {
                if let Ok(addr) = listener.local_addr() {
                    println!("Server listening on {}", addr);
                    // Send the address to the main thread
                    let _ = server_ready_tx.send(());

                    // Wait for the engine to be ready to accept
                    println!("Waiting for engine to be ready...");
                    if server_ready_rx.recv().is_ok() {
                        println!("Engine is ready, transferring listener fd");
                        // Transfer the listener to the engine
                        let listener_fd = unsafe { IntoRawFd::into_raw_fd(listener) };
                        let _ = fd_tx.send(listener_fd);
                    }
                }
            }
            Err(e) => {
                eprintln!("Error binding listener: {}", e);
            }
        }
    });

    // First wait for the notification that the server is listening
    if let Err(e) = server_ready_rx.recv() {
        return Err(format!("Server notification failed: {}", e).into());
    }

    // Signal that the engine is ready to accept
    println!("Engine ready to accept connections");
    server_ready_tx.send(()).unwrap();

    // Now receive the file descriptor
    let listener_fd = match fd_rx.recv() {
        Ok(fd) => fd,
        Err(e) => return Err(format!("Failed to get listener address: {}", e).into()),
    };

    // Start a client thread to connect to our server
    println!("Starting client thread...");
    let client_thread = thread::spawn(move || {
        // Small delay to ensure server is accepting
        thread::sleep(Duration::from_millis(100));

        println!("Client connecting to server...");
        match std::net::TcpStream::connect(server_addr) {
            Ok(mut stream) => {
                println!("Client connected");

                // Send a message
                let message = "Hello from client!";
                println!("Client sending: {}", message);
                if let Err(e) = stream.write_all(message.as_bytes()) {
                    eprintln!("Client write error: {}", e);
                    return;
                }

                // Read response
                let mut buffer = [0u8; 1024];
                match stream.read(&mut buffer) {
                    Ok(n) if n > 0 => {
                        println!(
                            "Client received: {}",
                            String::from_utf8_lossy(&buffer[0..n])
                        );
                    }
                    Ok(_) => println!("Server closed connection"),
                    Err(e) => eprintln!("Client read error: {}", e),
                }
            }
            Err(e) => eprintln!("Client connect error: {}", e),
        }
    });

    // Accept connection using the engine
    println!("Server accepting connections...");
    let (client_fd, addr) = match net_engine.accept(listener_fd).await {
        Ok(result) => result,
        Err(e) => {
            // Make sure client thread exits if we fail
            drop(server_ready_tx);
            return Err(e.into());
        }
    };
    println!("Accepted connection from {}", addr);

    // Read data from client
    println!("Reading data from client...");
    let client_data = net_engine.read(client_fd).await?;
    let client_message = String::from_utf8_lossy(&client_data);
    println!("Received: {}", client_message);

    // Send response
    let response = "Hello from server!";
    println!("Sending response: {}", response);
    net_engine
        .write(client_fd, response.as_bytes().to_vec())
        .await?;

    // Close the socket
    println!("Closing connection...");
    net_engine.close(client_fd).await?;

    // Wait for the client to complete
    if let Err(e) = server_thread.join() {
        eprintln!("Server thread join error: {:?}", e);
    }

    Ok(())
}
