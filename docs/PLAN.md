# Implementation Plan: Learning Raft in Rust

> This plan breaks the project down into **incremental, test-driven work units** that build on one another.  Each task specifies the *artifact(s) produced*, the *appropriate testing strategy*, and (where we consciously trade depth for simplicity) a **🚧 Future Depth** note that sketches potential next steps.

---

## Legend
| Phase | Scope |
|-------|-------|
| 🛠️  Work Unit | Concrete implementation task |
| ✅ Test | Unit, Integration (IT), Deterministic Cluster (DC), End-to-End (E2E) |
| 🚧 Future Depth | Possible advanced extension |

---

## Phase 0 – Repository Skeleton & CI
1. 🛠️  **Create Cargo workspace structure** (`raft-core`, `raft-transport`, `raft-storage`, `examples`).
   * ✅ *Unit*: `cargo check`, `cargo test` placeholder.
2. 🛠️  **Set up Continuous Integration** (GitHub Actions): `fmt`, `clippy`, `test` matrix.
   * ✅ *IT*: workflow executes on PRs.
   * 🚧 **Future Depth**: add MIRI, loom or shuttle for concurrency testing.

---

## Phase 1 – Foundations
### 1.1 Core Types & Proto
* 🛠️  Define shared types (`NodeId`, `Term`, `Index`, `LogEntry`, errors).
* 🛠️  Generate Rust code from `proto/raft/v1/raft.proto` with `prost` + `tonic`.
  * ✅ *Unit*: (de)serialization round-trip tests for all protobuf messages.
  * 🚧 **Future Depth**: enable Protobuf compatibility tests across language bindings.

### 1.2 Trait Contracts
* 🛠️  Draft **`Storage`** and **`Transport`** traits (see overview).
  * ✅ *Unit*: compile-time trait bounds, basic mock impls.
  * 🚧 Provide generic `StateMachine` trait for user commands.

---

## Phase 2 – In-Memory Implementations (Facilitate early testing)
### 2.1 In-Memory Storage
* 🛠️  Simple `Vec<LogEntry>` + `HashMap` persistence in memory.
  * ✅ *Unit*: append/read semantics, term & index invariants.
  * 🚧 **Future Depth**: durability guarantees via disk WAL (see Phase 4).

### 2.2 Mock Transport
* 🛠️  `ChannelTransport` using Tokio channels to simulate an in-process network.
  * ✅ *IT*: message ordering, loss, duplication scenarios.
  * 🚧 Introduce pluggable network partitions/fault injection for DC tests.

---

## Phase 3 – Raft Core: Leader Election
1. 🛠️  Implement **Follower → Candidate → Leader** state transitions, election timers, vote RPC handling.
   * ✅ *Unit*: pure state machine transition tests with mocked storage.
   * ✅ *IT*: 3-node cluster with mock transport reaches single leader.
   * ✅ *DC*: deterministic test harness controlling timers to ensure reproducibility.
   * 🚧 **Future Depth**: *pre-vote*, *leadership transfer*, optimized randomised timeouts.

---

## Phase 4 – Log Replication & Commit
1. 🛠️  AppendEntries request/response logic on leader & follower.
2. 🛠️  Leader commit index advancement & follower catch-up.
   * ✅ *Unit*: log matching property, commit rules.
   * ✅ *IT*: leader replicates commands to followers under normal operation.
   * ✅ *DC*: simulate follower lag, leader crash, new election recovery.
   * 🚧 Snapshotting & log compaction (planned Phase 7).

---

## Phase 5 – Disk-Backed Write-Ahead Log (WAL)
1. 🛠️  Implement minimal append-only WAL using Tokio FS (`raft-storage/wal.rs`).
2. 🛠️  Crash-recovery: reload HardState + log on startup.
   * ✅ *Unit*: fsync ordering, checksum validation.
   * ✅ *IT*: property tests with simulated crashes (close & reopen).
   * 🚧 **Future Depth**: segmented log files, mmap, sync batching.

---

## Phase 6 – gRPC Transport
1. 🛠️  `GrpcTransport` wrapping `tonic::client::Grpc` stubs.
2. 🛠️  `RaftService` server implementation delegating to `RaftNode`.
   * ✅ *IT*: loopback networking; ensure protobuf compatibility.
   * ✅ *E2E*: spin up N binaries in subprocesses communicating via TCP.
   * 🚧 TLS, authentication, connection pooling.

---

## Phase 7 – Snapshotting (Optional Stretch)
* 🛠️  InstallSnapshot RPC handling, snapshot file format, install path.
  * ✅ *IT*: follower too far behind receives snapshot.
  * 🚧 Incremental/streaming snapshots, install chunking.

---

## Phase 8 – Observability & Tooling
1. 🛠️  Integrate `tracing` with structured spans (election, replication, IO).
2. 🛠️  Provide `examples/cli.rs` for spawning local clusters and issuing commands.
   * ✅ *E2E*: demonstration script verifying replicated counter.
   * 🚧 Prometheus metrics, Jaeger tracing, Grafana dashboards.

---

## Phase 9 – Comprehensive Test Matrix & Documentation
1. 🛠️  Consolidate tests into `tests/` with cargo-nextest or similar.
2. 🛠️  Document expected behaviour, runbooks, contribution guide.

---

## Timeline (Indicative)
| Week | Focus |
|------|-------|
| 1 | Phase 0-1 |
| 2 | Phase 2 |
| 3-4 | Phase 3 |
| 5-6 | Phase 4 |
| 7 | Phase 5 |
| 8 | Phase 6 |
| 9 | Phase 7-8 |
| 10 | Phase 9, buffer & polish |

> **Reminder:** Scope can be adapted; correctness & learnings trump calendar precision.

---

## Trade-Off Summary
Throughout the plan we favour **simplicity & correctness** over completeness. Areas marked with 🚧 intentionally defer complexity until the core algorithm is solid. These markers serve as breadcrumbs for future deep dives once the foundational learning goals are achieved.
