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
- CMake (for some dependencies)
- Docker (for development on non-Linux platforms)

**Note**: This project uses Linux-specific features (io_uring, NUMA topology, etc.) and cannot run natively on macOS or Windows.

### Development Environments

#### Native Linux Development

If you're on Linux, you can build directly:

```bash
# Build with io_uring support
cargo build --features io_uring

# Run tests
cargo test --features io_uring
```

#### Docker Development (for macOS/Windows)

We provide a Docker environment for development on non-Linux platforms:

```bash
# Build the Docker image (first time only)
docker-compose build

# Interactive development shell
docker-compose run --rm elgate-dev

# Inside the container:
cargo build --features io_uring
cargo test --features io_uring
```

For one-off commands:

```bash
# Build the project
docker-compose run --rm elgate-dev cargo build --features io_uring

# Run tests
docker-compose run --rm elgate-dev cargo test --features io_uring
```

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
