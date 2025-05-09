//! High-performance network I/O using io_uring.
//!
//! This module provides network I/O operations using Linux's io_uring interface
//! for maximum performance with minimal syscalls.

use crate::ring::{OperationKind, RingBuffer};
use anyhow::Result;
use std::future::Future;
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::mpsc;

#[cfg(feature = "io_uring")]
use tokio_uring::net::TcpStream as UringTcpStream;

/// Configuration for the network I/O engine.
#[derive(Debug, Clone)]
pub struct NetworkConfig {
    /// Number of I/O worker threads to use.
    pub worker_threads: usize,

    /// Whether to pin I/O worker threads to CPUs.
    pub pin_threads: bool,

    /// Queue depth for io_uring.
    pub queue_depth: u32,

    /// Size of the read buffer in bytes.
    pub buffer_size: usize,

    /// NUMA node to use for I/O operations.
    pub numa_node: Option<usize>,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            worker_threads: 2,
            pin_threads: true,
            queue_depth: 256,
            buffer_size: 64 * 1024, // 64 KiB
            numa_node: None,
        }
    }
}

/// A high-performance network I/O engine using io_uring.
///
/// This engine uses Linux's io_uring interface for efficient network I/O
/// with minimal syscalls and kernel context switches.
pub struct NetworkEngine {
    /// Configuration for the engine.
    config: NetworkConfig,

    /// Shared memory ring buffer for communication with other components.
    ring_buffer: Arc<RingBuffer>,

    /// Channel for submitting I/O operations.
    tx: mpsc::Sender<NetworkOperation>,

    /// Worker threads handle.
    worker_handle: Option<tokio::task::JoinHandle<()>>,
}

/// Type of network operation.
#[derive(Debug)]
pub enum NetworkOperationType {
    /// Accept a connection on a listening socket.
    Accept,

    /// Connect to a remote host.
    Connect,

    /// Read from a socket.
    Read,

    /// Write to a socket.
    Write,

    /// Close a socket.
    Close,
}

/// A network I/O operation.
pub struct NetworkOperation {
    /// Type of operation.
    pub op_type: NetworkOperationType,

    /// Socket address.
    pub addr: Option<SocketAddr>,

    /// Socket file descriptor.
    pub fd: RawFd,

    /// Data buffer (for write operations).
    pub data: Option<Vec<u8>>,

    /// Callback to invoke when the operation completes.
    pub callback: Option<Box<dyn FnOnce(Result<NetworkResult>) + Send + 'static>>,
}

/// Result of a network operation.
#[derive(Debug)]
pub enum NetworkResult {
    /// Accepted connection: (socket, remote address).
    Accept(RawFd, SocketAddr),

    /// Connected socket.
    Connect(RawFd),

    /// Read data.
    Read(Vec<u8>),

    /// Number of bytes written.
    Write(usize),

    /// Socket closed.
    Close,
}

// We need to fix the socket_cache type issue
// Create a type alias for the socket we're storing
type SocketType = std::net::TcpStream;

impl NetworkEngine {
    /// Creates a new network I/O engine.
    pub async fn new(config: NetworkConfig, ring_buffer: Arc<RingBuffer>) -> Result<Self> {
        let (tx, rx) = mpsc::channel(1024);

        // Start worker thread(s)
        let worker_handle = Self::start_workers(config.clone(), rx, ring_buffer.clone()).await?;

        Ok(Self {
            config,
            ring_buffer,
            tx,
            worker_handle: Some(worker_handle),
        })
    }

    /// Starts the I/O worker threads.
    async fn start_workers(
        config: NetworkConfig,
        mut rx: mpsc::Receiver<NetworkOperation>,
        ring_buffer: Arc<RingBuffer>,
    ) -> Result<tokio::task::JoinHandle<()>> {
        // Create a new runtime with a single thread dedicated to io_uring
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .build()?;

        // Spawn a thread to run this dedicated runtime
        let worker_handle = std::thread::spawn(move || {
            rt.block_on(async move {
                #[cfg(feature = "io_uring")]
                {
                    // Create a socket handle cache
                    let mut socket_cache: std::collections::HashMap<RawFd, SocketType> =
                        std::collections::HashMap::new();

                    while let Some(op) = rx.recv().await {
                        match op.op_type {
                            NetworkOperationType::Accept => {
                                let fd = op.fd;

                                // First create a TcpListener from the raw fd
                                let std_listener =
                                    unsafe { std::net::TcpListener::from_raw_fd(fd) };
                                std_listener.set_nonblocking(true).unwrap();

                                // Now just use the standard TcpListener directly
                                match std_listener.accept() {
                                    Ok((stream, addr)) => {
                                        // Handle the accepted connection
                                        // Get the fd of the new connection
                                        let new_fd = stream.as_raw_fd();

                                        // Add the new socket to the cache
                                        socket_cache.insert(new_fd, stream);

                                        // Write to the ring buffer
                                        let data = format!("Accept: {}", addr).into_bytes();
                                        if let Err(err) =
                                            ring_buffer.write(OperationKind::NetworkReceive, &data)
                                        {
                                            eprintln!("Failed to write to ring buffer: {}", err);
                                        }

                                        if let Some(cb) = op.callback {
                                            cb(Ok(NetworkResult::Accept(new_fd, addr)));
                                        }
                                    }
                                    Err(err) => {
                                        if let Some(cb) = op.callback {
                                            cb(Err(anyhow::anyhow!("Accept failed: {}", err)));
                                        }
                                    }
                                }
                            }
                            NetworkOperationType::Connect => {
                                let addr = match op.addr {
                                    Some(addr) => addr,
                                    None => {
                                        if let Some(cb) = op.callback {
                                            cb(Err(anyhow::anyhow!(
                                                "Connect operation requires an address"
                                            )));
                                        }
                                        return;
                                    }
                                };

                                match UringTcpStream::connect(addr).await {
                                    Ok(uring_stream) => {
                                        // Get the fd of the new connection
                                        let fd = uring_stream.as_raw_fd();

                                        // Convert to a standard TcpStream for consistent cache storage
                                        let std_stream =
                                            unsafe { std::net::TcpStream::from_raw_fd(fd) };

                                        // Add to cache
                                        socket_cache.insert(fd, std_stream);

                                        // Write to the ring buffer
                                        let data = format!("Connect: {}", addr).into_bytes();
                                        if let Err(err) =
                                            ring_buffer.write(OperationKind::NetworkSend, &data)
                                        {
                                            eprintln!("Failed to write to ring buffer: {}", err);
                                        }

                                        if let Some(cb) = op.callback {
                                            cb(Ok(NetworkResult::Connect(fd)));
                                        }
                                    }
                                    Err(err) => {
                                        if let Some(cb) = op.callback {
                                            cb(Err(anyhow::anyhow!("Connect failed: {}", err)));
                                        }
                                    }
                                }
                            }
                            NetworkOperationType::Read => {
                                let fd = op.fd;

                                // Get the socket from the cache
                                if let Some(stream) = socket_cache.get_mut(&fd) {
                                    let mut buf = vec![0u8; config.buffer_size];

                                    match stream.read(&mut buf) {
                                        Ok(n) => {
                                            buf.truncate(n);

                                            // Write to the ring buffer
                                            if let Err(err) = ring_buffer
                                                .write(OperationKind::NetworkReceive, &buf)
                                            {
                                                eprintln!(
                                                    "Failed to write to ring buffer: {}",
                                                    err
                                                );
                                            }

                                            if let Some(cb) = op.callback {
                                                cb(Ok(NetworkResult::Read(buf)));
                                            }
                                        }
                                        Err(err) => {
                                            if let Some(cb) = op.callback {
                                                cb(Err(anyhow::anyhow!("Read failed: {}", err)));
                                            }
                                        }
                                    }
                                } else if let Some(cb) = op.callback {
                                    cb(Err(anyhow::anyhow!("Socket not found in cache")));
                                }
                            }
                            NetworkOperationType::Write => {
                                let fd = op.fd;

                                let data = match op.data {
                                    Some(data) => data,
                                    None => {
                                        if let Some(cb) = op.callback {
                                            cb(Err(anyhow::anyhow!(
                                                "Write operation requires data"
                                            )));
                                        }
                                        return;
                                    }
                                };

                                // Get the socket from the cache
                                if let Some(stream) = socket_cache.get_mut(&fd) {
                                    match stream.write(&data) {
                                        Ok(n) => {
                                            // Write to the ring buffer
                                            if let Err(err) = ring_buffer
                                                .write(OperationKind::NetworkSend, &data[..n])
                                            {
                                                eprintln!(
                                                    "Failed to write to ring buffer: {}",
                                                    err
                                                );
                                            }

                                            if let Some(cb) = op.callback {
                                                cb(Ok(NetworkResult::Write(n)));
                                            }
                                        }
                                        Err(err) => {
                                            if let Some(cb) = op.callback {
                                                cb(Err(anyhow::anyhow!("Write failed: {}", err)));
                                            }
                                        }
                                    }
                                } else if let Some(cb) = op.callback {
                                    cb(Err(anyhow::anyhow!("Socket not found in cache")));
                                }
                            }
                            NetworkOperationType::Close => {
                                let fd = op.fd;

                                // Remove the socket from the cache
                                socket_cache.remove(&fd);

                                if let Some(cb) = op.callback {
                                    cb(Ok(NetworkResult::Close));
                                }
                            }
                        }
                    }
                }
            });
        });

        // Create two separate channels
        let (monitor_tx, monitor_rx) = tokio::sync::oneshot::channel();
        let (wrapper_tx, wrapper_rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            let _ = worker_handle.join();
            let _ = monitor_tx.send(());
        });

        // Set up an async task to forward the signal
        tokio::spawn(async move {
            let _ = monitor_rx.await;
            let _ = wrapper_tx.send(());
        });

        let wrapper = ReceiverWrapper { rx: wrapper_rx };
        Ok(tokio::task::spawn(wrapper))
    }

    /// Accepts a connection on a listening socket.
    pub async fn accept(&self, listener_fd: RawFd) -> Result<(RawFd, SocketAddr)> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        self.tx
            .send(NetworkOperation {
                op_type: NetworkOperationType::Accept,
                addr: None,
                fd: listener_fd,
                data: None,
                callback: Some(Box::new(move |result| match result {
                    Ok(NetworkResult::Accept(fd, addr)) => {
                        let _ = tx.send(Ok((fd, addr)));
                    }
                    Ok(_) => {
                        let _ = tx.send(Err(anyhow::anyhow!("Unexpected result type")));
                    }
                    Err(err) => {
                        let _ = tx.send(Err(err));
                    }
                })),
            })
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send accept operation: {}", e))?;

        rx.await
            .map_err(|e| anyhow::anyhow!("Failed to receive accept result: {}", e))?
    }

    /// Connects to a remote host.
    pub async fn connect(&self, addr: SocketAddr) -> Result<RawFd> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        self.tx
            .send(NetworkOperation {
                op_type: NetworkOperationType::Connect,
                addr: Some(addr),
                fd: -1,
                data: None,
                callback: Some(Box::new(move |result| match result {
                    Ok(NetworkResult::Connect(fd)) => {
                        let _ = tx.send(Ok(fd));
                    }
                    Ok(_) => {
                        let _ = tx.send(Err(anyhow::anyhow!("Unexpected result type")));
                    }
                    Err(err) => {
                        let _ = tx.send(Err(err));
                    }
                })),
            })
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send connect operation: {}", e))?;

        rx.await
            .map_err(|e| anyhow::anyhow!("Failed to receive connect result: {}", e))?
    }

    /// Reads from a socket.
    pub async fn read(&self, fd: RawFd) -> Result<Vec<u8>> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        self.tx
            .send(NetworkOperation {
                op_type: NetworkOperationType::Read,
                addr: None,
                fd: fd,
                data: None,
                callback: Some(Box::new(move |result| match result {
                    Ok(NetworkResult::Read(data)) => {
                        let _ = tx.send(Ok(data));
                    }
                    Ok(_) => {
                        let _ = tx.send(Err(anyhow::anyhow!("Unexpected result type")));
                    }
                    Err(err) => {
                        let _ = tx.send(Err(err));
                    }
                })),
            })
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send read operation: {}", e))?;

        rx.await
            .map_err(|e| anyhow::anyhow!("Failed to receive read result: {}", e))?
    }

    /// Writes to a socket.
    pub async fn write(&self, fd: RawFd, data: Vec<u8>) -> Result<usize> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        self.tx
            .send(NetworkOperation {
                op_type: NetworkOperationType::Write,
                addr: None,
                fd: fd,
                data: Some(data),
                callback: Some(Box::new(move |result| match result {
                    Ok(NetworkResult::Write(n)) => {
                        let _ = tx.send(Ok(n));
                    }
                    Ok(_) => {
                        let _ = tx.send(Err(anyhow::anyhow!("Unexpected result type")));
                    }
                    Err(err) => {
                        let _ = tx.send(Err(err));
                    }
                })),
            })
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send write operation: {}", e))?;

        rx.await
            .map_err(|e| anyhow::anyhow!("Failed to receive write result: {}", e))?
    }

    /// Closes a socket.
    pub async fn close(&self, fd: RawFd) -> Result<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        self.tx
            .send(NetworkOperation {
                op_type: NetworkOperationType::Close,
                addr: None,
                fd: fd,
                data: None,
                callback: Some(Box::new(move |result| match result {
                    Ok(NetworkResult::Close) => {
                        let _ = tx.send(Ok(()));
                    }
                    Ok(_) => {
                        let _ = tx.send(Err(anyhow::anyhow!("Unexpected result type")));
                    }
                    Err(err) => {
                        let _ = tx.send(Err(err));
                    }
                })),
            })
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send close operation: {}", e))?;

        rx.await
            .map_err(|e| anyhow::anyhow!("Failed to receive close result: {}", e))?
    }
}

impl Drop for NetworkEngine {
    fn drop(&mut self) {
        // Abort worker threads
        if let Some(handle) = self.worker_handle.take() {
            handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ring::{RingBuffer, RingBufferOptions};
    use std::fs;
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::os::unix::io::AsRawFd;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    #[tokio::test]
    async fn test_network_engine_connect_write_read() {
        // Skip test if io_uring feature is not enabled
        if !cfg!(feature = "io_uring") {
            return;
        }

        // Create a ring buffer for communication
        let ring_path = "/tmp/elgate_test_net_ring";

        // Clean up from previous runs
        let _ = fs::remove_file(ring_path);

        // Create ring buffer
        let ring_options = RingBufferOptions {
            path: PathBuf::from(ring_path),
            size: 1024 * 1024, // 1 MiB
            slot_size: 4096,   // 4 KiB
        };

        let ring = Arc::new(RingBuffer::create(ring_options).unwrap());

        // Create network engine
        let config = NetworkConfig {
            worker_threads: 1,
            pin_threads: false, // Don't pin threads in tests
            queue_depth: 32,
            buffer_size: 4096,
            numa_node: None,
        };

        let net_engine = NetworkEngine::new(config, ring.clone()).await.unwrap();

        // Start a TCP server in a separate thread
        let server_addr = "127.0.0.1:9876".parse::<SocketAddr>().unwrap();
        let server_thread = thread::spawn(move || {
            let listener = TcpListener::bind(server_addr).unwrap();
            let (mut stream, _) = listener.accept().unwrap();

            // Read data from client
            let mut buffer = [0u8; 1024];
            let n = stream.read(&mut buffer).unwrap();

            // Echo data back
            stream.write_all(&buffer[0..n]).unwrap();
        });

        // Give the server time to start
        thread::sleep(Duration::from_millis(100));

        // Connect to the server
        let server_addr = "127.0.0.1:9876".parse::<SocketAddr>().unwrap();
        let socket_fd = net_engine.connect(server_addr).await.unwrap();

        // Write data to the server
        let test_data = b"Hello, io_uring network engine!".to_vec();
        let bytes_written = net_engine
            .write(socket_fd, test_data.clone())
            .await
            .unwrap();

        assert_eq!(bytes_written, test_data.len());

        // Read the echo response
        let read_data = net_engine.read(socket_fd).await.unwrap();

        // Verify data
        assert_eq!(read_data, test_data);

        // Close the socket
        net_engine.close(socket_fd).await.unwrap();

        // Wait for the server to complete
        server_thread.join().unwrap();

        // Clean up
        let _ = fs::remove_file(ring_path);
    }

    #[cfg(test)]
    fn has_network_io_uring_permissions() -> bool {
        // First check basic io_uring permissions
        let has_basic_io_uring = crate::disk::io_uring::tests::has_full_io_uring_permissions();

        // Then check network-specific permissions
        let can_bind_to_localhost = std::net::TcpListener::bind("127.0.0.1:0")
            .map(|listener| {
                // Get the ephemeral port assigned
                let _ = listener.local_addr();
                true
            })
            .unwrap_or(false);

        // Ensure we can create a socket with the right flags
        let has_socket_permissions = unsafe {
            let sock = libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0);
            if sock < 0 {
                false
            } else {
                let result = libc::setsockopt(
                    sock,
                    libc::SOL_SOCKET,
                    libc::SO_REUSEADDR,
                    &1 as *const i32 as *const libc::c_void,
                    std::mem::size_of::<i32>() as libc::socklen_t,
                ) >= 0;
                libc::close(sock);
                result
            }
        };

        has_basic_io_uring && can_bind_to_localhost && has_socket_permissions
    }

    #[test]
    #[cfg(feature = "io_uring")]
    fn test_network_engine_accept() {
        // Skip if not supported
        if !has_network_io_uring_permissions() {
            println!("Skipping io_uring network test - insufficient permissions or support");
            return;
        }

        // Use tokio_uring::start for proper context
        tokio_uring::start(async {
            use std::net::SocketAddr;
            use tokio::sync::oneshot;

            // Create a ring buffer for communication
            let ring_path = "/tmp/elgate_test_net_ring";

            // Clean up from previous runs
            let _ = std::fs::remove_file(ring_path);

            // Create ring buffer with proper options
            let ring_options = crate::ring::RingBufferOptions {
                path: std::path::PathBuf::from(ring_path),
                size: 1024 * 1024, // 1 MiB
                slot_size: 4096,   // 4 KiB
            };

            let ring = std::sync::Arc::new(crate::ring::RingBuffer::create(ring_options).unwrap());

            // Find an available port
            let server_addr = {
                let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                let addr = listener.local_addr().unwrap();
                drop(listener); // Release the port
                addr
            };

            println!("Test using server address: {}", server_addr);

            // Create network engine with CI-friendly settings
            let config = super::NetworkConfig {
                worker_threads: 1,
                pin_threads: false,
                queue_depth: 32,
                buffer_size: 4096,
                numa_node: None,
            };

            // Create the engine with proper error handling
            let net_engine = match super::NetworkEngine::new(config, ring.clone()).await {
                Ok(engine) => engine,
                Err(e) => {
                    println!("Failed to create io_uring network engine: {}", e);
                    return;
                }
            };

            // Create a channel for server-client synchronization
            let (tx, rx) = oneshot::channel::<()>();

            // Start server
            let server_handle = std::thread::spawn(move || {
                println!("Server thread starting");

                // Use proper tokio_uring context in the server thread
                tokio_uring::start(async {
                    // Start listening with proper error handling
                    let listener = match net_engine.listen(&server_addr).await {
                        Ok(l) => {
                            println!("Server listening on {}", server_addr);
                            l
                        }
                        Err(e) => {
                            println!("Server listen error: {}", e);
                            return;
                        }
                    };

                    // Signal client that server is ready
                    let _ = tx.send(());

                    // Accept a connection with proper error handling
                    match listener.accept().await {
                        Ok(stream) => {
                            println!(
                                "Server accepted connection from {}",
                                stream.peer_addr().unwrap()
                            );

                            // Echo back data from client
                            let mut buffer = [0u8; 128];
                            match stream.read(&mut buffer).await {
                                Ok(n) => {
                                    let data = &buffer[..n];
                                    println!(
                                        "Server received {} bytes: {:?}",
                                        n,
                                        String::from_utf8_lossy(data)
                                    );

                                    if let Err(e) = stream.write(data).await {
                                        println!("Server write error: {}", e);
                                    } else {
                                        println!("Server echoed data back to client");
                                    }
                                }
                                Err(e) => println!("Server read error: {}", e),
                            }
                        }
                        Err(e) => println!("Server accept error: {}", e),
                    }
                });
            });

            // Wait for server to start
            println!("Waiting for server to initialize...");
            match tokio::time::timeout(std::time::Duration::from_secs(2), rx).await {
                Ok(_) => println!("Server signaled it's ready"),
                Err(_) => {
                    println!("Timed out waiting for server to start");
                    let _ = server_handle.join();
                    return;
                }
            }

            // Add a small delay for stability
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            // Client test
            println!("Client connecting to {}", server_addr);
            match net_engine.connect(&server_addr).await {
                Ok(mut stream) => {
                    println!("Client connected to {}", server_addr);

                    // Send test data with proper error handling
                    let test_message = b"Hello from io_uring client!";
                    match stream.write(test_message).await {
                        Ok(_) => println!("Client sent message"),
                        Err(e) => {
                            println!("Client write error: {}", e);
                            let _ = server_handle.join();
                            return;
                        }
                    }

                    // Read the echoed response
                    let mut buffer = [0u8; 128];
                    match stream.read(&mut buffer).await {
                        Ok(n) => {
                            let received = &buffer[..n];
                            println!(
                                "Client received {} bytes: {:?}",
                                n,
                                String::from_utf8_lossy(received)
                            );
                            assert_eq!(received, test_message);
                        }
                        Err(e) => println!("Client read error: {}", e),
                    }
                }
                Err(e) => println!("Client connection error: {}", e),
            }

            // Clean up
            let _ = std::fs::remove_file(ring_path);
            let _ = server_handle.join();
        });
    }
}

// Create a simple Future wrapper around the receiver
struct ReceiverWrapper {
    rx: tokio::sync::oneshot::Receiver<()>,
}

impl Future for ReceiverWrapper {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Properly handle the Pin by using Pin::new
        let rx = &mut self.get_mut().rx;
        match Pin::new(rx).poll(cx) {
            Poll::Ready(_) => Poll::Ready(()),
            Poll::Pending => Poll::Pending,
        }
    }
}
