---
description: 
globs: 
alwaysApply: true
---
# Project Goal: Learning Raft

The primary purpose of this project is to learn about the Raft consensus algorithm by building a simplified, working implementation in Rust using Tokio and gRPC.

A key focus is ensuring the implementation is correct and robust, which will be achieved through a comprehensive testing strategy including unit, integration, deterministic cluster, and end-to-end tests.

Refer to [docs/overview.md](mdc:docs/overview.md) for the high-level design and testing approach.

---
description: 
globs: 
alwaysApply: true
---
# Rust Project Information

This project primarily uses the Rust programming language. Developers should be aware of and utilize modern Rust features where applicable.

## Key Recent Rust Features (as of ~1.85/1.86)

### Asynchronous Closures (`async || {}`)

Rust now supports asynchronous closures like `async || {}` which return futures when called. This works like an `async fn` which can also capture values from the local environment, just like the difference between regular closures and functions. This also comes with 3 analogous traits in the standard library prelude: `AsyncFn`, `AsyncFnMut`, and `AsyncFnOnce`.

In some cases, you could already approximate this with a regular closure and an asynchronous block, like `|| async {}`. However, the future returned by such an inner block is not able to borrow from the closure captures, but this does work with async closures:

```rust
let mut vec: Vec<String> = vec![];

let closure = async || {
    vec.push(ready(String::from("")).await);
};
```

It also has not been possible to properly express higher-ranked function signatures with the `Fn` traits returning a `Future`, but you can write this with the `AsyncFn` traits:

```rust
use core::future::Future;
async fn f<Fut>(_: impl for<'a> Fn(&'a u8) -> Fut)
where
    Fut: Future<Output = ()>,
{ todo!() }

async fn f2(_: impl for<'a> AsyncFn(&'a u8))
{ todo!() }

async fn main() {
    async fn g(_: &u8) { todo!() }
    f(g).await; // Causes compilation errors
    f2(g).await; // ok!
}
```

---
description: 
globs: 
alwaysApply: true
---
# Rust Workspace Structure

This project is structured as a Rust [Cargo Workspace](mdc:https:/doc.rust-lang.org/book/ch14-03-cargo-workspaces.html).

## Key Points:

*   **Shared Dependencies:** All member crates within the workspace share a single `Cargo.lock` file located at the root. This ensures consistent dependency versions across the entire project. Dependencies added to any crate's `Cargo.toml` are resolved together using this single lock file.
*   **Root `Cargo.toml`:** The root [`Cargo.toml`](mdc:Cargo.toml) file defines the workspace members. Individual crates have their own `Cargo.toml` files to declare their specific dependencies.
*   **Output Directory:** Compiled artifacts for all crates are placed in a single `target` directory at the workspace root.

When adding new crates, ensure they are added to the `members` list in the root [`Cargo.toml`](mdc:Cargo.toml). When adding dependencies, add them to the `[dependencies]` section of the specific crate's `Cargo.toml` file that requires them. Cargo handles ensuring version compatibility across the workspace.

# Rust Workspace Dependencies

This project utilizes a Cargo Workspace to manage common dependencies across its member crates.

## Common Dependencies

The core dependencies shared across the workspace are defined in the `[workspace.dependencies]` section of the root @`Cargo.toml` file. These include:

*   `wcygan_raft_community_neoeinstein-prost`: Protocol Buffers implementation (via Buf registry).
*   `wcygan_raft_community_neoeinstein-tonic`: gRPC implementation (via Buf registry).
*   `tokio`: Asynchronous runtime.
*   `tracing`: Application-level tracing framework.
*   `tracing-subscriber`: Utilities for implementing `tracing` subscribers.
*   `anyhow`: Flexible error handling.
*   `serde`: Framework for serializing/deserializing Rust data structures.
*   `bytes`: Utilities for working with bytes.
*   `thiserror`: Library for deriving `std::error::Error`.
*   `futures`: Abstractions for asynchronous programming.
*   `rand`: Random number generation utilities.

## Using Workspace Dependencies in a Crate

To use one of these shared dependencies in a specific crate (e.g., `raft-core`, `raft-grpc`), add it to the `[dependencies]` section of that crate's `Cargo.toml` file and specify `workspace = true`.

Example (in `raft-core/Cargo.toml`):

```toml
[dependencies]
tokio = { workspace = true }
tracing = { workspace = true }
anyhow = { workspace = true }
serde = { workspace = true, features = ["derive"] } # You can still enable features
wcygan_raft_community_neoeinstein-prost = { workspace = true }
```

Cargo will then use the version specified in the root `Cargo.toml`'s `[workspace.dependencies]` section, ensuring consistency across the project. You do not need to specify the version again in the individual crate's `Cargo.toml`.

## Project Overview

This project implements a simplified version of the Raft consensus algorithm using Rust, Tokio, and gRPC. Raft is a distributed consensus algorithm designed to be more understandable than alternatives like Paxos while providing the same safety and reliability guarantees.

## Architecture

The project is structured as a Rust workspace with the following components:

```
raft/
├── Cargo.toml (workspace)
├── raft-core/    (consensus algorithm)
├── raft-grpc/    (network transport)
├── raft-storage/ (persistence layer)
└── examples/     (demo applications)
```

## Core Abstractions

### RaftNode

The central component implementing the state machine:

```rust
pub struct RaftNode<S: Storage, T: Transport> {
    state: RaftState,
    config: Config,
    storage: S,
    transport: T,
}

enum Role {
    Follower,
    Candidate,
    Leader,
}

struct RaftState {
    current_term: u64,
    role: Role,
    voted_for: Option<NodeId>,
    log: Vec<LogEntry>,
    commit_index: u64,
    last_applied: u64,
    // Leader-specific state
    next_index: HashMap<NodeId, u64>,
    match_index: HashMap<NodeId, u64>,
}
```

### Transport Interface

An abstraction for network communication between nodes:

```rust
#[async_trait]
pub trait Transport {
    async fn send_append_entries(&self, to: NodeId, req: AppendEntriesRequest) 
        -> Result<AppendEntriesResponse>;
    
    async fn send_vote_request(&self, to: NodeId, req: VoteRequest) 
        -> Result<VoteResponse>;
}
```

### Storage Interface

For persisting Raft state:

```rust
#[async_trait]
pub trait Storage {
    async fn save_current_term(&mut self, term: u64) -> Result<()>;
    async fn save_voted_for(&mut self, node_id: Option<NodeId>) -> Result<()>;
    async fn append_log_entries(&mut self, entries: &[LogEntry]) -> Result<()>;
    async fn get_log_entries(&self, start: u64, end: u64) -> Result<Vec<LogEntry>>;
}
```

## Implementation Approach

### Protocol Buffers for gRPC

We'll define message types in `proto/raft/v1/raft.proto `:

```protobuf
syntax = "proto3";

// Package raft.v1 defines the protocol buffer messages and services for the Raft consensus protocol.
// This implementation follows the Raft paper (https://raft.github.io/).
package raft.v1;

// LogEntry represents a single entry in the replicated log.
// Each entry contains a command for the state machine and info about when it was created.
message LogEntry {
  uint64 index   = 1;          // position in the replicated log (starts at 1)
  uint64 term    = 2;          // leader term when entry was created
  bytes  command = 3;          // opaque state-machine command

  // Reserved tag numbers for graceful, non-breaking evolution (e.g. entry_type).
  reserved 4, 5, 6, 7, 8, 9;
}

// AppendEntriesRequest is sent by the leader to replicate log entries and as a heartbeat.
// It's one of the core RPCs in the Raft protocol.
message AppendEntriesRequest {
  uint64 term            = 1;  // leader's current term
  uint64 leader_id       = 2;  // for redirects by followers
  uint64 prev_log_index  = 3;  // index of log entry immediately preceding new ones
  uint64 prev_log_term   = 4;  // term of prev_log_index entry
  repeated LogEntry entries = 5;  // may be empty for heartbeat
  uint64 leader_commit   = 6;     // leader's commit index
}

// AppendEntriesResponse is the reply to AppendEntriesRequest.
// It indicates whether the append was successful and helps the leader track follower progress.
message AppendEntriesResponse {
  uint64 term         = 1;    // follower's current term
  bool   success      = 2;    // true if follower contained matching prefix
  uint64 match_index  = 3;    // highest index stored on follower
}

// RequestVoteRequest is sent by candidates during elections to gather votes.
message RequestVoteRequest {
  uint64 term             = 1; // candidate's term
  uint64 candidate_id     = 2; // candidate requesting vote
  uint64 last_log_index   = 3; // index of candidate's last log entry
  uint64 last_log_term    = 4; // term  of candidate's last log entry
}

// RequestVoteResponse is the reply to a vote request.
// It indicates whether the vote was granted to the candidate.
message RequestVoteResponse {
  uint64 term         = 1;    // current term of voter
  bool   vote_granted = 2;    // true = vote given, false = rejected
}

// InstallSnapshotRequest is sent by leaders to followers that are too far behind.
// It contains a snapshot of the state machine to help the follower catch up more quickly.
message InstallSnapshotRequest {
  uint64 term                 = 1;  // leader's current term
  uint64 leader_id            = 2;  // so follower can redirect clients
  uint64 last_included_index  = 3;  // the snapshot replaces all entries up through this index
  uint64 last_included_term   = 4;  // term of last_included_index
  bytes  data                 = 5;  // entire snapshot blob
}

// InstallSnapshotResponse is the reply to InstallSnapshotRequest.
// It contains the current term of the follower for leader to update itself.
message InstallSnapshotResponse {
  uint64 term = 1;  // current term of the responding server
}

// HardState contains the persistent state that must be saved to stable storage
// before responding to RPCs to ensure the Raft consensus algorithm's correctness.
message HardState {
  uint64 term         = 1;    // latest term seen
  uint64 voted_for    = 2;    // candidate_id voted for in current term
  uint64 commit_index = 3;    // highest log index known to be committed
}

// RaftService provides the core RPCs required for the Raft consensus protocol.
service RaftService {
  // AppendEntries is used by the leader to replicate log entries and send heartbeats.
  // Followers respond with confirmation of receipt or rejection.
  rpc AppendEntries   (AppendEntriesRequest)  returns (AppendEntriesResponse);

  // RequestVote is used by candidates during an election to request votes from peers.
  // Each server will vote for at most one candidate in a given term.
  rpc RequestVote     (RequestVoteRequest)    returns (RequestVoteResponse);

  // InstallSnapshot is used to transfer a snapshot of the state machine to followers
  // who are too far behind and need to catch up quickly.
  rpc InstallSnapshot (InstallSnapshotRequest) returns (InstallSnapshotResponse);
}

```

### gRPC Transport Implementation

```rust
pub struct GrpcTransport {
    clients: HashMap<NodeId, RaftServiceClient>,
}

#[async_trait]
impl Transport for GrpcTransport {
    async fn send_append_entries(&self, to: NodeId, req: AppendEntriesRequest) 
        -> Result<AppendEntriesResponse> {
        let client = self.clients.get(&to)
            .ok_or_else(|| Error::UnknownNode(to.clone()))?;
        Ok(client.append_entries(req).await?.into_inner())
    }
    
    // Other methods...
}
```

### Tokio Runtime Integration

We'll use Tokio for asynchronous execution:

```rust
pub struct RaftServer {
    node: Arc<RwLock<RaftNode<impl Storage, impl Transport>>>,
}

impl RaftServer {
    pub async fn start(self, addr: SocketAddr) -> Result<()> {
        let service = RaftServiceImpl { node: self.node.clone() };
        
        Server::builder()
            .add_service(RaftServiceServer::new(service))
            .serve(addr)
            .await?;
            
        Ok(())
    }
}
```

## Testing Strategy

### Unit Tests

Test individual components in isolation:

- Focus on pure functions and state transitions within `raft-core`.
- Use mock implementations for `Storage` and `Transport` traits to isolate the unit under test.

```rust
#[tokio::test]
async fn test_vote_request_handling() {
    let mut state = RaftState::new();
    // Test state transitions with mock storage/transport
}
```

### Integration Tests

Test interactions between components:

- Verify interactions between `raft-core` and specific `Storage` or `Transport` implementations (e.g., in-memory versions).
- Test the `raft-grpc` layer for correct serialization/deserialization and RPC handling, potentially using loopback connections.

```rust
#[tokio::test]
async fn test_leader_election_with_mock_network() {
    let nodes = create_test_cluster_with_mock_transport(3).await;
    disconnect_leader_mock(&nodes).await;
    
    // Wait for new leader election using the mock transport
    assert_eventually!(|| {
        nodes.iter().any(|n| n.state().role == Role::Leader)
    });
}
```

### Deterministic Testing

Create a controlled testing environment to validate the core consensus algorithm under specific conditions:

- Use a `TestHarness` with a `MockClock` and `TestNetwork`.
- Control time precisely to trigger timeouts and simulate network events (partitions, delays, message loss) deterministically.
- Allows for repeatable testing of complex failure and recovery scenarios.

```rust
struct TestHarness {
    nodes: Vec<TestNode>,
    network: TestNetwork, // Controls message delivery
    clock: MockClock,     // Controls time
}

impl TestHarness {
    async fn disconnect_node(&mut self, id: &NodeId) {
        self.network.disconnect(id);
    }
    
    async fn advance_time(&mut self, duration: Duration) {
        self.clock.advance(duration);
        // Process any pending timers based on mock clock
    }
}
```

### End-to-End Tests

Verify the complete system behaves as expected in a more realistic (but still local) environment:

- Spin up multiple `RaftServer` instances using the actual `GrpcTransport` and a chosen `Storage` backend (e.g., temporary file storage).
- Nodes communicate over real (local) network sockets.
- Validates the integration of all components working together.

```rust
#[tokio::test]
async fn test_cluster_recovers_from_network_partition() {
    let mut cluster = Cluster::new(5).await;
    
    // Create network partition
    cluster.partition([0, 1], [2, 3, 4]).await;
    
    // Verify cluster stabilizes
    cluster.wait_for_consensus().await;
    
    // Heal partition
    cluster.heal_partition().await;
    
    // Verify cluster reconverges
    cluster.wait_for_single_leader().await;
}
```

#### The `Cluster` Test Utility

The `Cluster` struct (used in E2E tests) is a helper designed to manage multiple Raft nodes running as independent entities (e.g., Tokio tasks) within a single test run. Its responsibilities typically include:

- **Node Management:** Spawning, stopping, and restarting individual Raft server instances.
- **Configuration:** Setting up each node with its unique ID, peer addresses, and dedicated storage (like temporary directories).
- **Interaction:** Providing methods to interact with the cluster, such as finding the leader or submitting commands.
- **Network Simulation:** Offering capabilities to simulate network failures like partitions (by potentially managing message filtering or dropping) or node isolation.
- **Assertion Helpers:** Including functions to wait for specific cluster states, like the election of a stable leader or the commitment of a log entry across a majority, simplifying test validation.

Using `Cluster` allows E2E tests to orchestrate complex scenarios involving multiple nodes and network conditions to verify the overall system's behavior and resilience.

## Testing with Real Processes (Multi-Process E2E)

Beyond the potentially in-process E2E testing facilitated by the `Cluster` utility, another crucial testing layer involves running multiple instances of the compiled Raft server binary as separate operating system processes.

This approach provides a higher fidelity test environment that more closely resembles a real deployment.

### Setup and Execution

1.  **Build Executable:** Compile the Raft server application into a runnable binary (e.g., using `cargo build`).
2.  **Configuration:** Each process requires unique configuration, typically provided via command-line arguments or configuration files:
    *   Unique `node-id`.
    *   Unique `listen-addr` for its gRPC server.
    *   List of `peer-addrs` (gRPC addresses of other nodes).
    *   Path to a dedicated `storage-dir`.
3.  **Launch Processes:** Start multiple instances of the binary. This can be done:
    *   Manually in separate terminal windows.
    *   Using shell scripts for automation.
    *   Via containerization tools like Docker and Docker Compose, which help manage networking, configuration, and lifecycles.
4.  **Communication:** Processes communicate directly over the network (even if it's the local loopback interface) using the `GrpcTransport` implementation.

### Benefits

-   **Higher Fidelity:** Tests interaction between actual OS processes, including process startup/shutdown behavior.
-   **Network Realism:** Uses actual network sockets for gRPC communication.
-   **Resource Isolation:** Each node runs with its own process resources.

This multi-process testing strategy complements other testing layers by verifying the system's behavior in an environment that is one step closer to a production deployment.
