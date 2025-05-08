# 🗺️ Elgate – Implementation Roadmap
`[ ]` todo  `[x]` done  

---

## 0 · Project Bootstrap
- [ ] **Repo & Workspace** – create `elgate/` mono-repo, Cargo workspace, MIT license  
- [ ] **RFC-0001** – high-level architecture spec committed  
- [ ] **CI pipeline** (GitHub Actions) – clippy • fmt • unit • miri • cargo-audit → green badge

---

## 1 · Architecture Detection
- [ ] `arch` module – detect logical cores & NUMA nodes (`num_cpus`, `sysinfo`)  
- [ ] Runtime modes → `SingleThread | PinnedSharded | ForcedSingle | TestStub`  
- [ ] Thread/worker builder using `core_affinity` (fallback graceful)  
- [ ] `/status` endpoint: cores, mode, load, build SHA  
- [ ] **Tests** – mock 1-core / 8-core / 32-core; verify pinning in `htop`

---

## 2 · Shared-Memory Ring & WAL
### Ring Buffer
- [ ] `/dev/shm/elgate_ring` – 4 MiB `mmap`  
- [ ] Header: padded 64 B (`head`, `tail`, `size`, `flags`)  
- [ ] 4 KiB slots → `[len][crc][op][payload]`  
- [ ] Producer SDK → CAS advance; back-pressure via futex  
- [ ] Consumer batch-pop ≤ 64 slots

### WAL
- [ ] Segment naming `route-YYYYMMDD-NNN.wal`  
- [ ] Append API → LSN; flush thread `fsync` group  
- [ ] Replay & tail-truncate on restart  
- [ ] GC by size/age or ACK checkpoint

### Benchmarks / Gates
- [ ] 10 k ops / s ≤ 80 µs avg (single-core)  
- [ ] Kill → replay with zero data loss  
- [ ] 1 M ops / s on 16 cores: CPU ≤ 20 %

---

## 3 · Disk I/O Engine (`tokio-uring`)
### Setup
- [ ] `io_uring_setup()` with flags  
  `IORING_SETUP_SQPOLL` (auto/on/off) `IORING_SETUP_SINGLE_ISSUER` (single-core)  
  `IORING_SETUP_COOP_TASKRUN` (non-SQPOLL)

### Registered Buffer Arena
- [ ] Per-NUMA huge-page arena (`huge_pages_mb` config)  
- [ ] `IORING_REGISTER_BUFFERS` once per worker; slab-allocator fallback

### Write Pipeline
- [ ] Batch WAL blocks → `IORING_OP_WRITE_FIXED` (`writev` fallback)  
- [ ] Linked SQEs `WRITE_FIXED → FSYNC` (`IOSQE_IO_LINK`)  
- [ ] Flush modes: `auto | manual | none`

### Telemetry
- `elgate_disk_write_latency_ms`, `disk_zero_copy_bytes_total`, `fsync_batch_size`

### Tests
- [ ] 100 MB / s WAL stream (NVMe) ≤ 250 µs P95  
- [ ] ENOSPC / read-only remount handled gracefully  
- [ ] Kernel < 5.10 fallback verified

---

## 4 · Network I/O Engine
### Core
- [ ] TLS via `rustls` (hot-reload certs)  
- [ ] Connection pool `host:port:tls`, keep-alive  
- [ ] Retry with exponential backoff + jitter (per-route overrides)

### Zero-Copy Send
- [ ] Detect kernel ≥ 5.11 & NIC `MSG_ZEROCOPY`  
- [ ] Use `IORING_OP_SEND_ZC` else fallback to `SEND`  
- [ ] Size threshold (default 2 KiB) configurable  
- [ ] Metric `elgate_net_zero_copy_bytes_total`

### Fixed-Buffer Send
- [ ] Re-use NUMA arena; prefer `SEND_ZC_FIXED` if supported

### Multishot Recv (optional)
- [ ] `IORING_OP_RECV_MULTISHOT` feature-flag

### SQPOLL / COOP
- [ ] SQPOLL kernel thread (poll 100 µs) when enabled  
- [ ] COOP TASKRUN otherwise

### Latency Gate
- [ ] TLS P99 ≤ 1 ms @ 50 k msg / s (25 GbE, 4 cores)

---

## 5 · SDKs / Interceptors
| Lang   | Work Items |
|--------|------------|
| Python | `[ ]` `elgate.patch_all()` – monkey-patch `open`, `requests.post` |
| Java   | `[ ]` ByteBuddy agent for `java.nio.*`, `HttpClient` |
| Node   | `[ ]` Patch `fs.writeFile`, `net.Socket.write` |
| C/C++  | `[ ]` `LD_PRELOAD` (`open`, `write`, `send`) + static client lib |

---

## 6 · Control Plane & Observability
- [ ] `/metrics` (Prometheus)  
- [ ] `/routes` GET/POST hot-reload  
- [ ] `/wal` list + tail LSN  
- [ ] `/info` — expose kernel flags (`sqpoll`, `send_zc`, `fixed_buf`)  
- [ ] Grafana dashboard JSON

---

## 7 · Kubernetes & Packaging
- [ ] Musl/scratch Docker image (small)  
- [ ] Helm chart — sidecar injection, `emptyDir.medium: Memory`  
- [ ] Multi-arch image (`amd64`, `arm64`) in CI  
- [ ] Example pods: Flask + JVM app with Elgate sidecar

---

## 8 · Advanced Performance & Security
- [ ] Stress-test `SEND_ZC` under NIC flap / driver reload  
- [ ] NUMA arena rebalance helper  
- [ ] BPF trace: DMA map/unmap counts  
- [ ] Circuit breaker when ring ≥ 90 %  
- [ ] mTLS option, ACLs (`allow_ips`, `deny_ips`)

---

## 9 · Benchmark Matrix (continuous)
| Category | Tool | Target |
|----------|------|--------|
| Ring latency | `ringbench` | P99 ≤ 120 µs |
| WAL throughput | `elgate-bench wal` | ≥ 250 MB / s |
| Disk I/O | `fio` + Elgate | P95 ≤ 250 µs |
| Network | `wrk`, `tc` loss | P99 ≤ 1 ms |
| Failure | ChaosMesh | Lost ops = 0 |

---

## 10 · Documentation & Release
- [ ] README, Quick-start, Roadmap, diagrams (`.mmd`, `.svg`)  
- [ ] MkDocs Material docs (`/docs`)  
- [ ] Cut `v0.1.0` tag, push images to GHCR

---

### 🚦 Success Criteria
| Metric | Goal |
|--------|------|
| **Durability** | 0 lost ops (kill / replay tests) |
| **Throughput** | ≥ 200 k ops / s on 8-core NVMe |
| **Disk P95** | ≤ 250 µs (write + fsync) |
| **Network TLS P99** | ≤ 1 ms |
| **Zero-copy ratio** | ≥ 90 % of eligible payload bytes |
| **Hot-reload** | Routes reload with zero downtime |
| **Observability** | Prometheus metrics + Grafana dashboards |

---

> Tick the boxes as you progress
