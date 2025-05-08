# RFC-0001: Elgate Architecture Specification

## Overview

Elgate is a high-performance I/O offloading sidecar service designed to handle disk and network operations on behalf of application processes. It provides durability, low-latency, and efficient resource utilization through a shared-memory communication model.

## Components

### 1. Architecture Detection & Runtime Modes

Elgate adapts to the underlying hardware by:
- Detecting available CPU cores and NUMA topology
- Selecting appropriate runtime mode:
  - `SingleThread`: One thread handling all operations (for single-core environments)
  - `PinnedSharded`: Worker threads pinned to specific cores/NUMA nodes
  - `ForcedSingle`: Force single-threaded mode (for testing/debugging)
  - `TestStub`: Mock implementation for testing

Implementation will use `num_cpus`, `sysinfo`, and `core_affinity` crates.

### 2. Shared Memory Ring Buffer

A lock-free, multi-producer, single-consumer ring buffer for inter-process communication:
- Located in `/dev/shm/elgate_ring`
- 4 MiB `mmap` region with 64-byte padded header
- 4 KiB slots containing operation metadata and payload
- CAS-based producer advancement with back-pressure via futex
- Batch consumption by Elgate workers

### 3. Write-Ahead Log (WAL)

Provides durability through journaling:
- Segment files named by route and timestamp
- Atomic append operations with LSN (Log Sequence Number)
- Group fsync for efficiency
- Replay capability on restart
- Garbage collection based on size/age thresholds

### 4. Disk I/O Engine

Low-latency disk operations using:
- `io_uring` via `tokio-uring`
- Registered buffer arenas using huge pages (NUMA-aware)
- Batch write operations with linked SQEs
- Configurable flush modes

### 5. Network I/O Engine

Efficient network operations with:
- TLS support via `rustls`
- Connection pooling and keep-alive management
- Retry logic with exponential backoff
- Zero-copy send operations where kernel support exists

### 6. Language SDKs

Interceptors for common languages that redirect I/O operations to Elgate:
- Python: `elgate.patch_all()` monkey-patching
- Java: ByteBuddy agent 
- Node.js: Module patching
- C/C++: `LD_PRELOAD` library

## Performance Targets

- WAL Performance: 10k ops/s with ≤80μs average latency (single-core)
- Disk I/O: ≤250μs P95 latency for WAL operations at 100MB/s
- Network I/O: TLS P99 ≤1ms at 50k msg/s
- Throughput: ≥200k ops/s on 8-core NVMe system

## Failure Handling

- Zero data loss on process kill (via WAL replay)
- Graceful handling of disk errors (ENOSPC, read-only)
- Circuit breaking for back-pressure

## Security Considerations

- TLS for all network communications
- Optional mTLS for client authentication
- IP-based allow/deny lists

## Observability

- Prometheus metrics endpoint
- Route configuration management
- WAL inspection API 
