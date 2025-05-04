use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;

use wcygan_raft_community_neoeinstein_prost::raft::v1::{
    AppendEntriesRequest, AppendEntriesResponse, HardState, LogEntry, RequestVoteRequest,
    RequestVoteResponse,
};

/// Represents a unique identifier for a node in the Raft cluster.
pub type NodeId = u64;

/// Represents the configuration of the Raft node, including parameters like heartbeat interval, election timeout, etc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The ID of the node
    pub id: NodeId,
    /// The list of peer nodes in the cluster
    pub peers: Vec<NodeId>,
    /// The timeout for heartbeats
    pub heartbeat_timeout: u64,
    /// The timeout for elections
    pub election_timeout: u64,
}

/// Represents the possible roles a Raft node can be in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    /// The node is a follower, waiting for heartbeats from the leader
    Follower,
    /// The node is a candidate, trying to become the leader
    Candidate,
    /// The node is the leader, sending heartbeats and log entries to followers
    Leader,
}

/// Volatile state maintained on all servers but not persisted
#[derive(Debug, Clone)]
pub struct VolatileState {
    /// The index of the highest log entry applied to the state machine
    pub last_applied: u64,
}

/// Volatile state maintained only on leaders, reinitialized after election
#[derive(Debug, Clone, Default)]
pub struct LeaderState {
    /// For each server, the next log entry to send to that server
    pub next_index: HashMap<NodeId, u64>,
    /// For each server, the index of the highest log entry known to be replicated
    pub match_index: HashMap<NodeId, u64>,
}

/// Implementation-specific state not part of the Raft paper's state model
#[derive(Debug, Clone)]
pub struct ServerState {
    /// The current role of the node (Follower, Candidate, Leader)
    pub role: Role,
    // Other implementation-specific fields could go here
}

/// Represents the complete state of a Raft node
#[derive(Debug, Clone)]
pub struct RaftState {
    /// The hard state that must be persisted to stable storage
    pub hard_state: HardState,
    /// The log entries, each containing a term and command
    pub log: Vec<LogEntry>,
    /// Volatile state maintained on all servers
    pub volatile: VolatileState,
    /// Leader-specific state
    pub leader: LeaderState,
    /// Implementation-specific server state
    pub server: ServerState,
}

impl RaftState {
    /// Creates a new RaftState with default values.
    pub fn new() -> Self {
        RaftState {
            hard_state: HardState {
                term: 0,
                voted_for: 0, // 0 means no vote cast (assuming NodeId 0 is not valid)
                commit_index: 0,
            },
            log: Vec::new(),
            volatile: VolatileState { last_applied: 0 },
            leader: LeaderState {
                next_index: HashMap::new(),
                match_index: HashMap::new(),
            },
            server: ServerState {
                role: Role::Follower,
            },
        }
    }
}

// Forward declarations for traits (assuming they are defined elsewhere in raft-core)
// pub trait Storage {}
// pub trait Transport {}

/// Trait defining the persistent storage operations required by Raft.
#[async_trait]
pub trait Storage {
    /// Saves the Raft node's hard state (term, voted_for, commit_index).
    async fn save_hard_state(&mut self, state: &HardState) -> Result<()>;

    /// Reads the persisted hard state.
    async fn read_hard_state(&self) -> Result<HardState>;

    /// Appends a slice of log entries to the storage.
    /// It's the implementation's responsibility to ensure consistency.
    async fn append_log_entries(&mut self, entries: &[LogEntry]) -> Result<()>;

    /// Reads a single log entry by its index.
    async fn read_log_entry(&self, index: u64) -> Result<Option<LogEntry>>;

    /// Reads log entries within a given range (inclusive start, exclusive end).
    async fn read_log_entries(&self, start_index: u64, end_index: u64) -> Result<Vec<LogEntry>>;

    /// Deletes log entries *before* the given index (exclusive).
    async fn truncate_log_prefix(&mut self, end_index_exclusive: u64) -> Result<()>;

    /// Deletes log entries *after* the given index (inclusive).
    async fn truncate_log_suffix(&mut self, start_index_inclusive: u64) -> Result<()>;

    /// Returns the index of the last entry in the log.
    async fn last_log_index(&self) -> Result<u64>;

    /// Returns the term of the last entry in the log.
    async fn last_log_term(&self) -> Result<u64>;

    // TODO: Add methods for snapshotting
    // async fn save_snapshot(&mut self, snapshot: &[u8], last_included_index: u64, last_included_term: u64) -> Result<()>;
    // async fn read_snapshot(&self) -> Result<Option<(Vec<u8>, u64, u64)>>;
}

/// Trait defining the network transport operations required by Raft.
#[async_trait]
pub trait Transport {
    /// Sends an AppendEntries RPC to a specific peer.
    async fn send_append_entries(
        &self,
        peer_id: NodeId,
        request: AppendEntriesRequest,
    ) -> Result<AppendEntriesResponse>;

    /// Sends a RequestVote RPC to a specific peer.
    async fn send_request_vote(
        &self,
        peer_id: NodeId,
        request: RequestVoteRequest,
    ) -> Result<RequestVoteResponse>;

    // TODO: Add method for InstallSnapshot RPC
    // async fn send_install_snapshot(
    //     &self,
    //     peer_id: NodeId,
    //     request: InstallSnapshotRequest,
    // ) -> Result<InstallSnapshotResponse>;
}

/// The main Raft node structure, encapsulating state and logic.
/// Generic over Storage and Transport implementations.
pub struct RaftNode<S: Storage + Send + Sync + 'static, T: Transport + Send + Sync + 'static> {
    /// The ID of the node
    pub id: NodeId,
    /// The current state of the Raft node
    pub state: RaftState,
    /// The configuration of the Raft node
    pub config: Config,
    /// The storage backend for persisting state
    pub storage: S,
    /// The transport layer for sending and receiving messages
    pub transport: T,
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use wcygan_raft_community_neoeinstein_prost::raft::v1::{
        InstallSnapshotRequest, InstallSnapshotResponse,
    };

    #[test]
    fn test_raft_state_new() {
        let state = RaftState::new();

        // Verify hard state defaults
        assert_eq!(state.hard_state.term, 0);
        assert_eq!(state.hard_state.voted_for, 0); // No vote cast
        assert_eq!(state.hard_state.commit_index, 0);

        // Verify log defaults
        assert!(state.log.is_empty());

        // Verify volatile state defaults
        assert_eq!(state.volatile.last_applied, 0);

        // Verify leader state defaults
        assert!(state.leader.next_index.is_empty());
        assert!(state.leader.match_index.is_empty());

        // Verify server state defaults
        assert_eq!(state.server.role, Role::Follower);
    }

    // --- Mock Storage Implementation ---
    #[derive(Debug, Clone, Default)]
    struct MockStorageInner {
        hard_state: HardState,
        log: Vec<LogEntry>,
    }

    #[derive(Debug, Clone, Default)]
    struct MockStorage {
        inner: Arc<Mutex<MockStorageInner>>,
    }

    #[async_trait]
    impl Storage for MockStorage {
        async fn save_hard_state(&mut self, state: &HardState) -> Result<()> {
            let mut inner = self.inner.lock().await;
            inner.hard_state = state.clone();
            Ok(())
        }

        async fn read_hard_state(&self) -> Result<HardState> {
            let inner = self.inner.lock().await;
            Ok(inner.hard_state.clone())
        }

        async fn append_log_entries(&mut self, entries: &[LogEntry]) -> Result<()> {
            let mut inner = self.inner.lock().await;
            inner.log.extend_from_slice(entries);
            Ok(())
        }

        async fn read_log_entry(&self, index: u64) -> Result<Option<LogEntry>> {
            let inner = self.inner.lock().await;
            // Log indices are 1-based
            if index == 0 || index > inner.log.len() as u64 {
                return Ok(None);
            }
            Ok(inner.log.get((index - 1) as usize).cloned())
        }

        async fn read_log_entries(
            &self,
            start_index: u64,
            end_index: u64,
        ) -> Result<Vec<LogEntry>> {
            let inner = self.inner.lock().await;
            // Indices are 1-based, range is exclusive end
            let start = (start_index.max(1) - 1) as usize;
            let end = (end_index.max(1) - 1) as usize;
            if start >= inner.log.len() || start > end {
                return Ok(Vec::new());
            }
            let actual_end = end.min(inner.log.len());
            Ok(inner.log[start..actual_end].to_vec())
        }

        async fn truncate_log_prefix(&mut self, end_index_exclusive: u64) -> Result<()> {
            let mut inner = self.inner.lock().await;
            // Indices are 1-based, exclusive end
            let keep_from = (end_index_exclusive.max(1) - 1) as usize;
            if keep_from >= inner.log.len() {
                inner.log.clear();
            } else {
                inner.log = inner.log.split_off(keep_from);
            }
            Ok(())
        }

        async fn truncate_log_suffix(&mut self, start_index_inclusive: u64) -> Result<()> {
            let mut inner = self.inner.lock().await;
            // Indices are 1-based, inclusive start
            if start_index_inclusive == 0 {
                inner.log.clear();
                return Ok(());
            }
            let truncate_at = (start_index_inclusive - 1) as usize;
            if truncate_at < inner.log.len() {
                inner.log.truncate(truncate_at);
            }
            Ok(())
        }

        async fn last_log_index(&self) -> Result<u64> {
            let inner = self.inner.lock().await;
            Ok(inner.log.len() as u64)
        }

        async fn last_log_term(&self) -> Result<u64> {
            let inner = self.inner.lock().await;
            Ok(inner.log.last().map_or(0, |entry| entry.term))
        }
    }

    #[tokio::test]
    async fn test_mock_storage_basic() {
        let mut storage = MockStorage::default();
        let initial_state = storage.read_hard_state().await.unwrap();
        assert_eq!(initial_state.term, 0);
        assert_eq!(initial_state.voted_for, 0);
        assert_eq!(initial_state.commit_index, 0);

        let new_state = HardState {
            term: 1,
            voted_for: 1,
            commit_index: 0,
        };
        storage.save_hard_state(&new_state).await.unwrap();

        let read_state = storage.read_hard_state().await.unwrap();
        assert_eq!(read_state.term, 1);
        assert_eq!(read_state.voted_for, 1);
        assert_eq!(read_state.commit_index, 0);

        let entries = vec![
            LogEntry {
                index: 1,
                term: 1,
                command: vec![1].into(),
            },
            LogEntry {
                index: 2,
                term: 1,
                command: vec![2].into(),
            },
        ];
        storage.append_log_entries(&entries).await.unwrap();

        assert_eq!(storage.last_log_index().await.unwrap(), 2);
        assert_eq!(storage.last_log_term().await.unwrap(), 1);

        let entry1 = storage.read_log_entry(1).await.unwrap().unwrap();
        assert_eq!(entry1.index, 1);

        storage.truncate_log_suffix(2).await.unwrap(); // Keep entry 1
        assert_eq!(storage.last_log_index().await.unwrap(), 1);
        assert!(storage.read_log_entry(2).await.unwrap().is_none());

        storage.truncate_log_prefix(1).await.unwrap(); // Remove nothing (index 1 exclusive)
        assert_eq!(storage.last_log_index().await.unwrap(), 1);
        storage.truncate_log_prefix(2).await.unwrap(); // Remove entry 1
        assert_eq!(storage.last_log_index().await.unwrap(), 0);
    }

    // --- Mock Transport Implementation ---
    #[derive(Debug, Clone, Default)]
    struct MockTransport;

    #[async_trait]
    impl Transport for MockTransport {
        async fn send_append_entries(
            &self,
            _peer_id: NodeId,
            _request: AppendEntriesRequest,
        ) -> Result<AppendEntriesResponse> {
            // In a real mock, you might record calls or return programmed responses
            Ok(AppendEntriesResponse {
                term: 0,
                success: false,
                match_index: 0,
            })
        }

        async fn send_request_vote(
            &self,
            _peer_id: NodeId,
            _request: RequestVoteRequest,
        ) -> Result<RequestVoteResponse> {
            // In a real mock, you might record calls or return programmed responses
            Ok(RequestVoteResponse {
                term: 0,
                vote_granted: false,
            })
        }
    }

    #[tokio::test]
    async fn test_mock_transport_basic() {
        let transport = MockTransport::default();
        let req = AppendEntriesRequest::default();
        let res = transport.send_append_entries(1, req).await.unwrap();
        assert!(!res.success); // Basic check that the default response is returned

        let req = RequestVoteRequest::default();
        let res = transport.send_request_vote(1, req).await.unwrap();
        assert!(!res.vote_granted);
    }

    // --- Protobuf (de)serialization round-trip tests ---
    #[test]
    fn test_protobuf_roundtrip_messages() {
        // LogEntry
        let le = LogEntry {
            index: 42,
            term: 7,
            command: vec![1, 2, 3].into(),
        };
        let buf = le.encode_to_vec();
        let decoded = LogEntry::decode(buf.as_slice()).unwrap();
        assert_eq!(decoded, le);

        // HardState
        let hs = HardState {
            term: 3,
            voted_for: 4,
            commit_index: 5,
        };
        let buf = hs.encode_to_vec();
        let decoded = HardState::decode(buf.as_slice()).unwrap();
        assert_eq!(decoded, hs);

        // AppendEntriesRequest
        let aer = AppendEntriesRequest {
            term: 1,
            leader_id: 2,
            prev_log_index: 3,
            prev_log_term: 4,
            entries: vec![le.clone()],
            leader_commit: 5,
        };
        let buf = aer.encode_to_vec();
        let decoded = AppendEntriesRequest::decode(buf.as_slice()).unwrap();
        assert_eq!(decoded, aer);

        // AppendEntriesResponse
        let aeres = AppendEntriesResponse {
            term: 6,
            success: true,
            match_index: 7,
        };
        let buf = aeres.encode_to_vec();
        let decoded = AppendEntriesResponse::decode(buf.as_slice()).unwrap();
        assert_eq!(decoded, aeres);

        // RequestVoteRequest
        let rvr = RequestVoteRequest {
            term: 8,
            candidate_id: 9,
            last_log_index: 10,
            last_log_term: 11,
        };
        let buf = rvr.encode_to_vec();
        let decoded = RequestVoteRequest::decode(buf.as_slice()).unwrap();
        assert_eq!(decoded, rvr);

        // RequestVoteResponse
        let rvres = RequestVoteResponse {
            term: 12,
            vote_granted: false,
        };
        let buf = rvres.encode_to_vec();
        let decoded = RequestVoteResponse::decode(buf.as_slice()).unwrap();
        assert_eq!(decoded, rvres);

        // InstallSnapshotRequest
        let isr = InstallSnapshotRequest {
            term: 13,
            leader_id: 14,
            last_included_index: 15,
            last_included_term: 16,
            data: vec![8, 9].into(),
        };
        let buf = isr.encode_to_vec();
        let decoded = InstallSnapshotRequest::decode(buf.as_slice()).unwrap();
        assert_eq!(decoded, isr);

        // InstallSnapshotResponse
        let isres = InstallSnapshotResponse { term: 17 };
        let buf = isres.encode_to_vec();
        let decoded = InstallSnapshotResponse::decode(buf.as_slice()).unwrap();
        assert_eq!(decoded, isres);
    }
}
