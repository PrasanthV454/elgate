# 🚀 Elgate

**Elgate** is a high-performance, architecture-aware sidecar service written in Rust that offloads all disk and network I/O from applications to a dedicated async engine using:

- 🧠 Shared memory ring buffers (RAM-based IPC)
- ⚡ `io_uring` for low-latency disk writes
- 🔐 TLS-enabled async TCP sending
- 🧾 Durable Write-Ahead Logs (WAL)
- 🧩 Pluggable SDKs for Python, Java, Node, C/C++
- 🛠️ Runtime-aware thread/core binding (NUMA/CPU-topology adaptive)

---

## 💡 Why Elgate?

Elgate allows you to:
- Offload file and network I/O from your app with **no code rewrite**
- Achieve **near-zero-copy I/O** and **sub-millisecond latencies**
- Handle spikes in load with **WAL-backed retries**
- Deploy across single-core to multi-core machines efficiently

---

## 🧱 Architecture Overview

![image](https://github.com/user-attachments/assets/0f3245fb-124a-4237-9f6b-3d94d64fa40a)


## ⚙️ Key Components

### 🔸 Ring Buffer (Shared Memory)
- Located in `/dev/shm/elgate_ring`
- App writes I/O ops to ring (via SDK)
- Elgate polls and processes them asynchronously

### 🔸 WAL (Write-Ahead Log)
- Journals all write operations to disk
- Guarantees durability and crash recovery
- Replay logic on startup

### 🔸 Disk I/O Engine
- Backed by `tokio-uring`
- Supports `O_DIRECT`, `fsync`, and batching
- Optional registered buffers for zero-copy

### 🔸 Network I/O Engine
- TLS support via `rustls`
- Connection pooling, retry with backoff
- Supports TCP and HTTP transports

### 🔸 Runtime Awareness
- Detects CPU cores and NUMA nodes
- Switches between single-threaded and sharded-worker mode
- Core pinning for cache-local execution

---

## Getting Started

Elgate is currently in early development. Check back soon for installation and usage instructions.

## Development

### Requirements

- Rust 1.86.0 or later
- **Linux** kernel 5.10+ (5.11+ recommended for best performance)
- `liburing-dev` package for io_uring support
- `numactl` package for NUMA support

**Note**: This project uses Linux-specific features (io_uring, NUMA topology, etc.) and cannot run natively on macOS or Windows.

### Development Environments

#### Native Linux Development

If you're on Linux, you can build directly:

```bash
# Install required dependencies
sudo apt-get update
sudo apt-get install -y liburing-dev numactl

# Build the project (io_uring and NUMA support are enabled by default)
cargo build

# Run tests
cargo test
```

#### Using Semaphore CI for Development

For the best development experience, we recommend using Semaphore CI which provides a proper Linux environment with all dependencies pre-configured:

1. Fork the repository
2. Connect your fork to Semaphore CI
3. Use the provided environment for development and testing

The `.semaphore/semaphore.yml` configuration file is already set up with all necessary dependencies.

#### Disabling Linux-specific Features

If you need to build without Linux-specific features (for documentation or cross-platform code):

```bash
# Build without io_uring and NUMA support
cargo build --no-default-features

# Enable only specific features
cargo build --no-default-features --features "numa"
```

> ⚠️ **Note**: While Docker-based development is configured in the repository, it is currently not functioning properly. We recommend using native Linux or Semaphore CI instead.

### Project Structure

```
elgate/
├── .github/workflows/  # CI configuration
├── docs/rfcs/          # Architecture specifications
├── elgate-core/        # Core library code
├── Dockerfile          # Development container
├── docker-compose.yml  # Container orchestration
└── ...
```

## License

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for details.
