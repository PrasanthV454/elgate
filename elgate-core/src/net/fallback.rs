//! Standard socket I/O fallback implementation.
//!
//! This module provides a fallback network I/O implementation using
//! standard Rust socket I/O for platforms without io_uring support.

use crate::ring::{OperationKind, RingBuffer};
use anyhow::{Context, Result};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::sync::Arc;
use tokio::sync::mpsc;
use std::io::{Read, Write};

/// Configuration for the network I/O engine.
#[derive(Debug, Clone)]
pub struct NetworkConfig {
    /// Number of I/O worker threads to use.
    pub worker_threads: usize,
    
    /// Whether to pin I/O worker threads to CPUs.
    pub pin_threads: bool,
    
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
            buffer_size: 64 * 1024, // 64 KiB
            numa_node: None,
        }
    }
}

/// A network I/O engine using standard socket I/O.
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

/// Result of a network operation.
#[derive(Debug)]
pub enum NetworkResult {
    /// Accept result.
    Accept(RawFd, SocketAddr),
    
    /// Connect result.
    Connect(RawFd),
    
    /// Read result.
    Read(Vec<u8>),
    
    /// Write result.
    Write(usize),
    
    /// Close result.
    Close,
}

/// A network I/O operation.
#[derive(Debug)]
pub struct NetworkOperation {
    /// Type of operation.
    pub op_type: NetworkOperationType,
    
    /// Socket address.
    pub addr: Option<SocketAddr>,
    
    /// Socket file descriptor.
    pub fd: Option<RawFd>,
    
    /// Data buffer (for write operations).
    pub data: Option<Vec<u8>>,
    
    /// Completion callback.
    pub callback: Option<Box<dyn FnOnce(Result<NetworkResult>) + Send + 'static>>,
}

impl NetworkEngine {
    /// Creates a new network I/O engine.
    pub async fn new(config: NetworkConfig, ring_buffer: Arc<RingBuffer>) -> Result<Self> {
        let (tx, rx) = mpsc::channel(config.worker_threads * 32);
        
        let engine = Self {
            config,
            ring_buffer,
            tx,
            worker_handle: None,
        };
        
        // Start worker threads
        let worker_handle = engine.start_workers(rx).await?;
        
        Ok(Self {
            worker_handle: Some(worker_handle),
            ..engine
        })
    }
    
    /// Starts worker threads for processing I/O operations.
    async fn start_workers(
        &self,
        mut rx: mpsc::Receiver<NetworkOperation>,
    ) -> Result<tokio::task::JoinHandle<()>> {
        let worker_handle = tokio::spawn(async move {
            // Process operations from the channel
            while let Some(op) = rx.recv().await {
                match op.op_type {
                    NetworkOperationType::Accept => {
                        let result = if let Some(fd) = op.fd {
                            // Create a TcpListener from the raw file descriptor
                            let listener = unsafe { TcpListener::from_raw_fd(fd) };
                            
                            // Accept a connection
                            match listener.accept() {
                                Ok((stream, addr)) => {
                                    // Get the raw file descriptor
                                    let fd = stream.as_raw_fd();
                                    
                                    // Don't drop the stream or the fd will be closed
                                    std::mem::forget(stream);
                                    
                                    Ok(NetworkResult::Accept(fd, addr))
                                }
                                Err(e) => Err(anyhow::anyhow!("Failed to accept: {}", e)),
                            }
                        } else {
                            Err(anyhow::anyhow!("No file descriptor provided for accept operation"))
                        };
                        
                        // Call the callback
                        if let Some(callback) = op.callback {
                            callback(result);
                        }
                    }
                    NetworkOperationType::Connect => {
                        let result = if let Some(addr) = op.addr {
                            // Connect to the address
                            match TcpStream::connect(addr) {
                                Ok(stream) => {
                                    // Set non-blocking mode
                                    let _ = stream.set_nonblocking(true);
                                    
                                    // Get the raw file descriptor
                                    let fd = stream.as_raw_fd();
                                    
                                    // Don't drop the stream or the fd will be closed
                                    std::mem::forget(stream);
                                    
                                    Ok(NetworkResult::Connect(fd))
                                }
                                Err(e) => Err(anyhow::anyhow!("Failed to connect: {}", e)),
                            }
                        } else {
                            Err(anyhow::anyhow!("No address provided for connect operation"))
                        };
                        
                        // Call the callback
                        if let Some(callback) = op.callback {
                            callback(result);
                        }
                    }
                    NetworkOperationType::Read => {
                        let result = if let Some(fd) = op.fd {
                            // Create a TcpStream from the raw file descriptor
                            let mut stream = unsafe { TcpStream::from_raw_fd(fd) };
                            
                            // Read from the stream
                            let mut buffer = vec![0u8; self.config.buffer_size];
                            match stream.read(&mut buffer) {
                                Ok(n) => {
                                    buffer.truncate(n);
                                    
                                    // Don't drop the stream or the fd will be closed
                                    let _ = stream.as_raw_fd();
                                    std::mem::forget(stream);
                                    
                                    Ok(NetworkResult::Read(buffer))
                                }
                                Err(e) => Err(anyhow::anyhow!("Failed to read: {}", e)),
                            }
                        } else {
                            Err(anyhow::anyhow!("No file descriptor provided for read operation"))
                        };
                        
                        // Call the callback
                        if let Some(callback) = op.callback {
                            callback(result);
                        }
                    }
                    NetworkOperationType::Write => {
                        let result = if let Some(fd) = op.fd {
                            if let Some(data) = op.data {
                                // Create a TcpStream from the raw file descriptor
                                let mut stream = unsafe { TcpStream::from_raw_fd(fd) };
                                
                                // Write to the stream
                                match stream.write(&data) {
                                    Ok(n) => {
                                        // Don't drop the stream or the fd will be closed
                                        let _ = stream.as_raw_fd();
                                        std::mem::forget(stream);
                                        
                                        Ok(NetworkResult::Write(n))
                                    }
                                    Err(e) => Err(anyhow::anyhow!("Failed to write: {}", e)),
                                }
                            } else {
                                Err(anyhow::anyhow!("No data provided for write operation"))
                            }
                        } else {
                            Err(anyhow::anyhow!("No file descriptor provided for write operation"))
                        };
                        
                        // Call the callback
                        if let Some(callback) = op.callback {
                            callback(result);
                        }
                    }
                    NetworkOperationType::Close => {
                        let result = if let Some(fd) = op.fd {
                            // Create a TcpStream from the raw file descriptor
                            let stream = unsafe { TcpStream::from_raw_fd(fd) };
                            
                            // Close the stream by dropping it
                            drop(stream);
                            
                            Ok(NetworkResult::Close)
                        } else {
                            Err(anyhow::anyhow!("No file descriptor provided for close operation"))
                        };
                        
                        // Call the callback
                        if let Some(callback) = op.callback {
                            callback(result);
                        }
                    }
                }
            }
        });
        
        Ok(worker_handle)
    }
    
    /// Accepts a connection on a listening socket.
    pub async fn accept(&self, fd: RawFd) -> Result<(RawFd, SocketAddr)> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        
        self.tx.send(NetworkOperation {
            op_type: NetworkOperationType::Accept,
            addr: None,
            fd: Some(fd),
            data: None,
            callback: Some(Box::new(move |result| {
                match result {
                    Ok(NetworkResult::Accept(fd, addr)) => {
                        let _ = tx.send(Ok((fd, addr)));
                    }
                    Ok(_) => {
                        let _ = tx.send(Err(anyhow::anyhow!("Unexpected result type")));
                    }
                    Err(err) => {
                        let _ = tx.send(Err(err));
                    }
                }
            })),
        }).await.context("Failed to send accept operation")?;
        
        rx.await.context("Failed to receive accept result")?
    }
    
    /// Connects to a remote host.
    pub async fn connect(&self, addr: SocketAddr) -> Result<RawFd> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        
        self.tx.send(NetworkOperation {
            op_type: NetworkOperationType::Connect,
            addr: Some(addr),
            fd: None,
            data: None,
            callback: Some(Box::new(move |result| {
                match result {
                    Ok(NetworkResult::Connect(fd)) => {
                        let _ = tx.send(Ok(fd));
                    }
                    Ok(_) => {
                        let _ = tx.send(Err(anyhow::anyhow!("Unexpected result type")));
                    }
                    Err(err) => {
                        let _ = tx.send(Err(err));
                    }
                }
            })),
        }).await.context("Failed to send connect operation")?;
        
        rx.await.context("Failed to receive connect result")?
    }
    
    /// Reads from a socket.
    pub async fn read(&self, fd: RawFd) -> Result<Vec<u8>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        
        self.tx.send(NetworkOperation {
            op_type: NetworkOperationType::Read,
            addr: None,
            fd: Some(fd),
            data: None,
            callback: Some(Box::new(move |result| {
                match result {
                    Ok(NetworkResult::Read(data)) => {
                        let _ = tx.send(Ok(data));
                    }
                    Ok(_) => {
                        let _ = tx.send(Err(anyhow::anyhow!("Unexpected result type")));
                    }
                    Err(err) => {
                        let _ = tx.send(Err(err));
                    }
                }
            })),
        }).await.context("Failed to send read operation")?;
        
        rx.await.context("Failed to receive read result")?
    }
    
    /// Writes to a socket.
    pub async fn write(&self, fd: RawFd, data: Vec<u8>) -> Result<usize> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        
        self.tx.send(NetworkOperation {
            op_type: NetworkOperationType::Write,
            addr: None,
            fd: Some(fd),
            data: Some(data),
            callback: Some(Box::new(move |result| {
                match result {
                    Ok(NetworkResult::Write(n)) => {
                        let _ = tx.send(Ok(n));
                    }
                    Ok(_) => {
                        let _ = tx.send(Err(anyhow::anyhow!("Unexpected result type")));
                    }
                    Err(err) => {
                        let _ = tx.send(Err(err));
                    }
                }
            })),
        }).await.context("Failed to send write operation")?;
        
        rx.await.context("Failed to receive write result")?
    }
    
    /// Closes a socket.
    pub async fn close(&self, fd: RawFd) -> Result<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        
        self.tx.send(NetworkOperation {
            op_type: NetworkOperationType::Close,
            addr: None,
            fd: Some(fd),
            data: None,
            callback: Some(Box::new(move |result| {
                match result {
                    Ok(NetworkResult::Close) => {
                        let _ = tx.send(Ok(()));
                    }
                    Ok(_) => {
                        let _ = tx.send(Err(anyhow::anyhow!("Unexpected result type")));
                    }
                    Err(err) => {
                        let _ = tx.send(Err(err));
                    }
                }
            })),
        }).await.context("Failed to send close operation")?;
        
        rx.await.context("Failed to receive close result")?
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
    use std::net::{TcpListener, SocketAddr};
    use std::os::unix::io::AsRawFd;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;
    use std::thread;
    
    #[tokio::test]
    async fn test_fallback_network_engine_connect_write_read() {
        // Create a ring buffer for communication
        let ring_path = "/tmp/elgate_test_fallback_net_ring";
        
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
            buffer_size: 4096,
            numa_node: None,
        };
        
        let net_engine = NetworkEngine::new(config, ring.clone()).await.unwrap();
        
        // Start a TCP server in a separate thread
        let server_addr = "127.0.0.1:9878".parse::<SocketAddr>().unwrap();
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
        let server_addr = "127.0.0.1:9878".parse::<SocketAddr>().unwrap();
        let socket_fd = net_engine.connect(server_addr).await.unwrap();
        
        // Write data to the server
        let test_data = b"Hello, fallback network engine!".to_vec();
        let bytes_written = net_engine.write(socket_fd, test_data.clone()).await.unwrap();
        
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
    
    #[tokio::test]
    async fn test_fallback_network_engine_accept() {
        // Create a ring buffer for communication
        let ring_path = "/tmp/elgate_test_fallback_net_ring_accept";
        
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
            buffer_size: 4096,
            numa_node: None,
        };
        
        let net_engine = NetworkEngine::new(config, ring.clone()).await.unwrap();
        
        // Create a TCP listener
        let server_addr = "127.0.0.1:9879".parse::<SocketAddr>().unwrap();
        let listener = TcpListener::bind(server_addr).unwrap();
        let listener_fd = listener.as_raw_fd();
        
        // Start a client in a separate thread
        let client_thread = thread::spawn(move || {
            // Give the server time to start accepting
            thread::sleep(Duration::from_millis(100));
            
            // Connect to the server
            let _ = TcpStream::connect(server_addr).unwrap();
        });
        
        // Accept connection using the engine
        let (client_fd, addr) = net_engine.accept(listener_fd).await.unwrap();
        
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
        
        // Close the socket
        net_engine.close(client_fd).await.unwrap();
        
        // Wait for the client to complete
        client_thread.join().unwrap();
        
        // Clean up
        let _ = fs::remove_file(ring_path);
    }
    
    #[tokio::test]
    async fn test_fallback_network_engine_error_handling() {
        // Create a ring buffer for communication
        let ring_path = "/tmp/elgate_test_fallback_net_ring_errors";
        
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
            pin_threads: false,
            buffer_size: 4096,
            numa_node: None,
        };
        
        let net_engine = NetworkEngine::new(config, ring.clone()).await.unwrap();
        
        // Try to connect to a non-existent server
        let invalid_addr = "127.0.0.1:1".parse::<SocketAddr>().unwrap(); // Port 1 should not be listening
        let result = net_engine.connect(invalid_addr).await;
        assert!(result.is_err(), "Connecting to non-existent server should fail");
        
        // Try to read from an invalid file descriptor
        let invalid_fd = 99999; // This FD should not exist
        let result = net_engine.read(invalid_fd).await;
        assert!(result.is_err(), "Reading from invalid FD should fail");
        
        // Clean up
        let _ = fs::remove_file(ring_path);
    }
}
