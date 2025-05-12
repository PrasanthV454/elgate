//! Network I/O example for Elgate.
//!
//! This example demonstrates how to use the network I/O engine for
//! high-performance socket operations.

use elgate_core::net::io_uring::{NetworkConfig, NetworkEngine};
use elgate_core::ring::{RingBuffer, RingBufferOptions};
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Use tokio-uring runtime instead of tokio runtime
    tokio_uring::start(async {
        run_example().await.unwrap();
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
    let net_engine = NetworkEngine::new(config, ring.clone()).await?;

    // -----------------------------------------------------------
    // Example 1: Client connecting to an echo server
    // -----------------------------------------------------------

    // Start an echo server in a separate thread
    let server_addr = "127.0.0.1:0".parse::<SocketAddr>().unwrap();
    println!("Starting echo server at {}", server_addr);

    let listener = TcpListener::bind(server_addr)?;
    let actual_server_addr = listener.local_addr()?;
    println!("Server actually listening on {}", actual_server_addr);

    let server_thread = thread::spawn(move || {
        println!("Server listening...");

        let (mut stream, client_addr) = listener.accept().unwrap();
        println!("Server accepted connection from {}", client_addr);

        // Read data from client
        let mut buffer = [0u8; 1024];
        let n = stream.read(&mut buffer).unwrap();
        println!("Server received {} bytes", n);

        // Echo data back
        stream.write_all(&buffer[0..n]).unwrap();
        println!("Server echoed data back");
    });

    // Give the server time to start
    thread::sleep(Duration::from_millis(100));

    // Connect to the server
    println!("Connecting to echo server...");
    let server_addr = actual_server_addr;
    let socket_fd = net_engine.connect(server_addr).await?;
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
    server_thread.join().unwrap();

    // -----------------------------------------------------------
    // Example 2: Server accepting connections
    // -----------------------------------------------------------

    println!("\nStarting server example...");

    // Create a TCP listener
    let server_addr = "127.0.0.1:0".parse::<SocketAddr>().unwrap();
    println!("Creating TCP listener at {}", server_addr);
    let listener = TcpListener::bind(server_addr)?;
    let actual_server_addr = listener.local_addr()?;
    println!("Server actually listening on {}", actual_server_addr);
    let listener_fd = unsafe { std::os::unix::io::IntoRawFd::into_raw_fd(listener) };

    // Start a client in a separate thread
    println!("Starting client thread...");
    let client_thread = thread::spawn(move || {
        // Give the server time to start accepting
        thread::sleep(Duration::from_millis(100));

        println!("Client connecting to server...");
        let mut stream = TcpStream::connect(actual_server_addr).unwrap();
        println!("Client connected");

        // Send a message
        let message = "Hello from client!";
        println!("Client sending: {}", message);
        stream.write_all(message.as_bytes()).unwrap();

        // Read response
        let mut buffer = [0u8; 1024];
        let n = stream.read(&mut buffer).unwrap();
        println!(
            "Client received: {}",
            String::from_utf8_lossy(&buffer[0..n])
        );
    });

    // Accept connection using the engine
    println!("Server accepting connections...");
    let (client_fd, addr) = net_engine.accept(listener_fd).await?;
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
    client_thread.join().unwrap();

    // Clean up
    println!("Cleaning up...");
    let _ = fs::remove_file(ring_path);

    println!("Network I/O example completed successfully!");
    Ok(())
}
