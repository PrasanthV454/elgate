//! High-performance network I/O using io_uring.
//!
//! This module provides network I/O operations using Linux's io_uring interface
//! for maximum performance with minimal syscalls.

use crate::ring::{OperationKind, RingBuffer};
use anyhow::Result;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::sync::Arc;
use std::sync::Mutex;

#[cfg(feature = "io_uring")]
use tokio_uring::net::TcpStream as UringTcpStream;

#[cfg(test)]
use std::os::unix::fs::OpenOptionsExt;

/// Configuration for the network I/O engine.
///
/// This configuration is used for the direct tokio-uring implementation.
#[derive(Debug, Clone)]
pub struct NetworkConfig {
    /// Queue depth for io_uring.
    pub queue_depth: u32,

    /// Size of the read buffer in bytes.
    pub buffer_size: usize,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            queue_depth: 256,
            buffer_size: 64 * 1024, // 64 KiB
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

    /// Connection cache to maintain open handles
    connections: Mutex<HashMap<SocketAddr, tokio_uring::net::TcpStream>>,

    /// Listener cache to maintain open handles
    listeners: Mutex<HashMap<RawFd, tokio_uring::net::TcpListener>>,
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

impl NetworkEngine {
    /// Creates a new network I/O engine.
    pub async fn new(config: NetworkConfig, ring_buffer: Arc<RingBuffer>) -> Result<Self> {
        Ok(Self {
            config,
            ring_buffer,
            connections: Mutex::new(HashMap::new()),
            listeners: Mutex::new(HashMap::new()),
        })
    }

    /// Accepts a connection on a listening socket.
    pub async fn accept(&self, listener_fd: RawFd) -> Result<(RawFd, SocketAddr)> {
        // Get or create a listener from the raw file descriptor
        let listener = unsafe {
            let std_listener = std::net::TcpListener::from_raw_fd(listener_fd);
            let addr = std_listener.local_addr()?;
            let tokio_listener = tokio_uring::net::TcpListener::from(std_listener);

            // Store in our cache
            let mut listeners = self.listeners.lock().unwrap();
            listeners.insert(listener_fd, tokio_listener.clone());

            tokio_listener
        };

        // Accept connection directly with tokio_uring
        let (stream, addr) = listener
            .accept()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to accept connection: {}", e))?;

        // Get the file descriptor
        let fd = stream.as_raw_fd();

        // Store the connection
        let mut conns = self.connections.lock().unwrap();
        conns.insert(addr, stream);

        // Notify via ring buffer
        let _ = self
            .ring_buffer
            .write(OperationKind::NetworkReceive, addr.to_string().as_bytes());

        Ok((fd, addr))
    }

    /// Connects to a remote host.
    pub async fn connect(&self, addr: SocketAddr) -> Result<RawFd> {
        // Connect directly with tokio_uring
        let stream = tokio_uring::net::TcpStream::connect(addr)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to connect to {}: {}", addr, e))?;

        // Get the file descriptor
        let fd = stream.as_raw_fd();

        // Store the connection
        let mut conns = self.connections.lock().unwrap();
        conns.insert(addr, stream);

        // Notify via ring buffer
        let _ = self
            .ring_buffer
            .write(OperationKind::NetworkSend, addr.to_string().as_bytes());

        Ok(fd)
    }

    /// Reads data from a socket.
    pub async fn read(&self, fd: RawFd) -> Result<Vec<u8>> {
        // Find the connection in our cache
        let stream = {
            let conns = self.connections.lock().unwrap();
            let mut stream_opt = None;

            // Find the connection with this file descriptor
            for (addr, conn) in conns.iter() {
                if conn.as_raw_fd() == fd {
                    stream_opt = Some((addr.clone(), conn.clone()));
                    break;
                }
            }

            stream_opt.ok_or_else(|| anyhow::anyhow!("No connection found for fd {}", fd))?
        };

        // Allocate buffer
        let buf_size = self.config.buffer_size;
        let buf = vec![0u8; buf_size];

        // Read from socket
        let (res, buf) = stream.1.read(buf).await;
        let bytes_read = res.map_err(|e| anyhow::anyhow!("Failed to read from socket: {}", e))?;

        // Notify via ring buffer
        let _ = self.ring_buffer.write(
            OperationKind::NetworkReceive,
            stream.0.to_string().as_bytes(),
        );

        // Return read data
        Ok(buf[..bytes_read].to_vec())
    }

    /// Writes data to a socket.
    pub async fn write(&self, fd: RawFd, data: Vec<u8>) -> Result<usize> {
        // Find the connection in our cache
        let stream = {
            let conns = self.connections.lock().unwrap();
            let mut stream_opt = None;

            // Find the connection with this file descriptor
            for (addr, conn) in conns.iter() {
                if conn.as_raw_fd() == fd {
                    stream_opt = Some((addr.clone(), conn.clone()));
                    break;
                }
            }

            stream_opt.ok_or_else(|| anyhow::anyhow!("No connection found for fd {}", fd))?
        };

        // Write to socket
        let (res, _) = stream.1.write(data).await;
        let bytes_written = res.map_err(|e| anyhow::anyhow!("Failed to write to socket: {}", e))?;

        // Notify via ring buffer
        let _ = self
            .ring_buffer
            .write(OperationKind::NetworkSend, stream.0.to_string().as_bytes());

        // Return bytes written
        Ok(bytes_written)
    }

    /// Closes a socket.
    pub async fn close(&self, fd: RawFd) -> Result<()> {
        // Remove connection from cache
        let mut addr_to_remove = None;
        {
            let conns = self.connections.lock().unwrap();

            // Find the connection with this file descriptor
            for (addr, conn) in conns.iter() {
                if conn.as_raw_fd() == fd {
                    addr_to_remove = Some(addr.clone());
                    break;
                }
            }
        }

        // If found, remove it
        if let Some(addr) = addr_to_remove {
            let mut conns = self.connections.lock().unwrap();
            conns.remove(&addr);

            // Notify via ring buffer
            let _ = self
                .ring_buffer
                .write(OperationKind::NetworkClose, addr.to_string().as_bytes());
        }

        Ok(())
    }
}

impl Drop for NetworkEngine {
    fn drop(&mut self) {
        // Close all connections
        let mut conns = self.connections.lock().unwrap();
        conns.clear();

        // Close all listeners
        let mut listeners = self.listeners.lock().unwrap();
        listeners.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::io::AsRawFd;

    #[test]
    #[cfg(feature = "io_uring")]
    fn test_network_engine_connect_write_read() {
        // Skip if not supported
        if !has_network_io_uring_permissions() {
            println!(
                "Skipping io_uring network connect test - insufficient permissions or support"
            );
            return;
        }

        tokio_uring::start(async {
            // Create a ring buffer for communication
            let ring_path = "/tmp/elgate_test_net_ring_connect";

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

            // Create network engine with simplified config
            let config = NetworkConfig {
                queue_depth: 32,
                buffer_size: 4096,
            };

            let engine = NetworkEngine::new(config, ring.clone()).await.unwrap();

            // Start echo server using standard std library
            let (tx, rx) = tokio::sync::oneshot::channel::<()>();
            let server_handle = std::thread::spawn(move || {
                let listener = std::net::TcpListener::bind(server_addr).unwrap();
                println!("Server listening on {}", server_addr);

                // Signal client we're ready
                let _ = tx.send(());

                let (mut client, addr) = listener.accept().unwrap();
                println!("Server accepted connection from {}", addr);

                // Echo anything we receive
                let mut buffer = [0u8; 1024];
                let n = client.read(&mut buffer).unwrap();
                println!("Server received {} bytes", n);

                client.write_all(&buffer[..n]).unwrap();
                println!("Server echoed data back");
            });

            // Wait for server to start
            rx.await.unwrap();

            // Connect to server using our engine
            println!("Connecting to {}", server_addr);
            let fd = engine.connect(server_addr).await.unwrap();
            println!("Connected with fd={}", fd);

            // Write data using engine
            let test_data = b"Hello from io_uring!";
            let bytes_written = engine.write(fd, test_data.to_vec()).await.unwrap();
            println!("Wrote {} bytes", bytes_written);

            // Read response using engine
            let received = engine.read(fd).await.unwrap();
            println!(
                "Read {} bytes: {:?}",
                received.len(),
                String::from_utf8_lossy(&received)
            );

            // Verify data is correct
            assert_eq!(&received, test_data);

            // Close connection
            engine.close(fd).await.unwrap();

            // Wait for server to complete
            let _ = server_handle.join();

            // Clean up
            let _ = std::fs::remove_file(ring_path);
        });
    }

    #[test]
    #[cfg(feature = "io_uring")]
    fn test_network_engine_accept() {
        // Skip if not supported
        if !has_network_io_uring_permissions() {
            println!("Skipping io_uring network test - insufficient permissions or support");
            return;
        }

        tokio_uring::start(async {
            // Create a ring buffer for communication
            let ring_path = "/tmp/elgate_test_net_ring_accept";

            // Clean up from previous runs
            let _ = std::fs::remove_file(ring_path);

            // Create ring buffer with proper options
            let ring_options = crate::ring::RingBufferOptions {
                path: std::path::PathBuf::from(ring_path),
                size: 1024 * 1024, // 1 MiB
                slot_size: 4096,   // 4 KiB
            };

            let ring = std::sync::Arc::new(crate::ring::RingBuffer::create(ring_options).unwrap());

            // Create network engine with simplified config
            let config = NetworkConfig {
                queue_depth: 32,
                buffer_size: 4096,
            };

            let engine = NetworkEngine::new(config, ring.clone()).await.unwrap();

            // Setup a TCP listener
            let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let server_addr = std_listener.local_addr().unwrap();
            println!("Test server listening on {}", server_addr);

            // Get the raw fd to pass to our engine
            let listener_fd = std_listener.as_raw_fd();

            // Start a client that connects in a separate thread
            let client_handle = std::thread::spawn(move || {
                // Small delay to ensure the server is ready
                std::thread::sleep(std::time::Duration::from_millis(100));

                // Connect to the server
                let mut client = std::net::TcpStream::connect(server_addr).unwrap();
                println!("Client connected to {}", server_addr);

                // Send some data
                let test_data = b"Hello from test client!";
                client.write_all(test_data).unwrap();
                println!("Client sent data");

                // Read echo response
                let mut buffer = [0u8; 1024];
                let n = client.read(&mut buffer).unwrap();
                println!("Client received {} bytes back", n);

                // Verify data
                assert_eq!(&buffer[..n], test_data);
            });

            // Accept using our engine
            let (client_fd, client_addr) = engine.accept(listener_fd).await.unwrap();
            println!("Server accepted connection from {}", client_addr);

            // Read the data sent by client
            let received = engine.read(client_fd).await.unwrap();
            println!(
                "Server read {} bytes: {:?}",
                received.len(),
                String::from_utf8_lossy(&received)
            );

            // Echo it back
            let bytes_written = engine.write(client_fd, received.clone()).await.unwrap();
            println!("Server echoed {} bytes back", bytes_written);

            // Close the connection
            engine.close(client_fd).await.unwrap();

            // Wait for client to complete
            let _ = client_handle.join();

            // Clean up
            let _ = std::fs::remove_file(ring_path);
        });
    }

    #[cfg(test)]
    fn has_network_io_uring_permissions() -> bool {
        // Check basic io_uring functionality first
        let has_device_permissions = std::path::Path::new("/dev/io_uring").exists()
            && std::fs::metadata("/dev/io_uring")
                .map(|m| m.permissions().readonly())
                .unwrap_or(true)
                == false;

        let has_direct_io = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .custom_flags(libc::O_DIRECT)
            .open("/tmp/io_uring_test_direct")
            .map(|_| {
                let _ = std::fs::remove_file("/tmp/io_uring_test_direct");
                true
            })
            .unwrap_or(false);

        let has_submission_queue_permissions = std::process::Command::new("sh")
            .arg("-c")
            .arg("ulimit -l")
            .output()
            .map(|o| {
                let output = String::from_utf8_lossy(&o.stdout);
                let memlock = output.trim().parse::<u64>().unwrap_or(0);
                memlock > 1024
            })
            .unwrap_or(false);

        // Then check network-specific permissions
        let can_bind_to_localhost = std::net::TcpListener::bind("127.0.0.1:0")
            .map(|listener| {
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

        has_device_permissions
            && has_direct_io
            && has_submission_queue_permissions
            && can_bind_to_localhost
            && has_socket_permissions
    }
}
