use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::fmt::Debug;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The node is a follower, waiting for heartbeats from the leader
    Follower,
    /// The node is a candidate, trying to become the leader
    Candidate,
    /// The node is the leader, sending heartbeats and log entries to followers
    Leader,
}

/// Represents the volatile state managed by a Raft node.
#[derive(Debug, Clone, PartialEq)]
pub struct VolatileState {
    /// The index of the highest log entry applied to the state machine
    pub last_applied: u64,
}

/// Represents state specific to a Raft leader.
#[derive(Debug, Clone, PartialEq)]
pub struct LeaderState {
    /// For each server, the next log entry to send to that server
    /// (initialized to leader last log index + 1)
    pub next_index: HashMap<NodeId, u64>,
    /// For each server, index of the highest log entry known to be replicated
    /// on server (initialized to 0, increases monotonically)
    pub match_index: HashMap<NodeId, u64>,
}

/// Represents implementation-specific server state.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerState {
    /// The current role of the node (Follower, Candidate, Leader)
    pub role: Role,
    /// The leader ID
    pub leader_id: Option<NodeId>,
    // Other implementation-specific fields could go here
}

/// Represents the complete state of a Raft node
#[derive(Debug, Clone, PartialEq)]
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
    /// The ID of the node
    pub node_id: NodeId,
}

impl RaftState {
    /// Creates a new `RaftState` with default values.
    pub fn new(node_id: NodeId) -> Self {
        let default_hard_state = HardState {
            term: 0,
            voted_for: 0,
            commit_index: 0,
        };
        let default_volatile_state = VolatileState { last_applied: 0 };
        let default_leader_state = LeaderState {
            next_index: HashMap::new(),
            match_index: HashMap::new(),
        };
        let default_server_state = ServerState {
            role: Role::Follower,
            leader_id: None,
        };

        RaftState {
            hard_state: default_hard_state,
            log: Vec::new(),
            volatile: default_volatile_state,
            leader: default_leader_state,
            node_id,
            server: default_server_state,
        }
    }
}

impl Default for RaftState {
    fn default() -> Self {
        Self::new(0)
    }
}

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
        HardState, InstallSnapshotRequest, InstallSnapshotResponse,
    };

    // --- RaftState Initialization and Defaults ---
    #[test]
    fn test_raft_state_new() {
        let node_id = 1;
        let state = RaftState::new(node_id);

        // Verify node id
        assert_eq!(state.node_id, node_id);

        // Verify hard state defaults
        assert_eq!(state.hard_state.term, 0);
        assert_eq!(state.hard_state.voted_for, 0); // Use 0 for no vote
        assert_eq!(state.hard_state.commit_index, 0);

        // Verify log defaults
        assert!(state.log.is_empty());

        // Verify volatile state defaults
        assert_eq!(state.volatile.last_applied, 0);

        // Verify leader state defaults
        assert!(state.leader.next_index.is_empty());
        assert!(state.leader.match_index.is_empty());

        // Verify server state defaults using the local Role
        assert_eq!(state.server.role, Role::Follower);
        assert_eq!(state.server.leader_id, None); // Leader ID starts as None
    }

    // --- Helper functions for state transitions (simplified for direct manipulation) ---

    // Simulates becoming a candidate
    fn become_candidate(state: &mut RaftState, new_term: u64) {
        state.hard_state.term = new_term;
        state.hard_state.voted_for = state.node_id; // Vote for self
        state.server.role = Role::Candidate;
        state.server.leader_id = None; // No leader when candidate
    }

    // Simulates becoming a leader
    fn become_leader(state: &mut RaftState, peers: &[NodeId], last_log_index: u64) {
        state.server.role = Role::Leader;
        state.server.leader_id = Some(state.node_id); // Leader is self
        state.leader.next_index.clear();
        state.leader.match_index.clear();
        for &peer_id in peers {
            state.leader.next_index.insert(peer_id, last_log_index + 1);
            state.leader.match_index.insert(peer_id, 0);
        }
    }

    // Simulates reverting to follower
    fn become_follower(state: &mut RaftState, new_term: u64, leader_id: Option<NodeId>) {
        state.hard_state.term = new_term;
        state.hard_state.voted_for = 0; // Reset vote when becoming follower potentially
        state.server.role = Role::Follower;
        state.server.leader_id = leader_id;
        // Leader state (next/match_index) is implicitly ignored when not leader
    }

    // --- RaftState Transition Tests ---
    #[test]
    fn test_raft_state_transitions() {
        let node_id = 1;
        let peers = vec![2, 3];
        let mut state = RaftState::new(node_id);
        let initial_last_log_index = 0; // Assuming empty log initially

        // 1. Initial state is Follower
        assert_eq!(state.server.role, Role::Follower);
        assert_eq!(state.hard_state.term, 0);
        assert_eq!(state.hard_state.voted_for, 0);
        assert_eq!(state.server.leader_id, None);

        // 2. Transition to Candidate
        let term1 = 1;
        become_candidate(&mut state, term1);
        assert_eq!(state.server.role, Role::Candidate);
        assert_eq!(state.hard_state.term, term1);
        assert_eq!(state.hard_state.voted_for, node_id); // Voted for self
        assert_eq!(state.server.leader_id, None);

        // 3. Transition to Leader
        become_leader(&mut state, &peers, initial_last_log_index);
        assert_eq!(state.server.role, Role::Leader);
        assert_eq!(state.hard_state.term, term1); // Term doesn't change on becoming leader
        assert_eq!(state.hard_state.voted_for, node_id); // Vote remains
        assert_eq!(state.server.leader_id, Some(node_id));
        assert_eq!(state.leader.next_index.len(), peers.len());
        assert_eq!(state.leader.match_index.len(), peers.len());
        for peer_id in &peers {
            assert_eq!(state.leader.next_index[peer_id], initial_last_log_index + 1);
            assert_eq!(state.leader.match_index[peer_id], 0);
        }

        // 4. Discover higher term, revert to Follower
        let term2 = 2;
        let new_leader_id = Some(peers[0]);
        become_follower(&mut state, term2, new_leader_id);
        assert_eq!(state.server.role, Role::Follower);
        assert_eq!(state.hard_state.term, term2);
        assert_eq!(state.hard_state.voted_for, 0); // Vote resets
        assert_eq!(state.server.leader_id, new_leader_id);
        // Leader state maps remain but are ignored

        // 5. Become Candidate again in a later term
        let term3 = 3;
        become_candidate(&mut state, term3);
        assert_eq!(state.server.role, Role::Candidate);
        assert_eq!(state.hard_state.term, term3);
        assert_eq!(state.hard_state.voted_for, node_id);
        assert_eq!(state.server.leader_id, None);
    }

    #[test]
    fn test_leader_state_updates() {
        let node_id = 1;
        let peers = vec![2, 3];
        let mut state = RaftState::new(node_id);
        let initial_log_index = 5; // Assume some log entries exist

        // Become leader
        become_leader(&mut state, &peers, initial_log_index);

        // Simulate successful replication to peer 2 up to index 7
        let peer2_match_index = 7;
        state.leader.match_index.insert(2, peer2_match_index);
        state.leader.next_index.insert(2, peer2_match_index + 1);

        // Simulate successful replication to peer 3 up to index 6
        let peer3_match_index = 6;
        state.leader.match_index.insert(3, peer3_match_index);
        state.leader.next_index.insert(3, peer3_match_index + 1);

        assert_eq!(state.leader.match_index[&2], peer2_match_index);
        assert_eq!(state.leader.next_index[&2], peer2_match_index + 1);
        assert_eq!(state.leader.match_index[&3], peer3_match_index);
        assert_eq!(state.leader.next_index[&3], peer3_match_index + 1);

        // Simulate a scenario where peer 2 needs to backtrack (e.g., after leader change)
        let peer2_new_next_index = 4;
        state.leader.next_index.insert(2, peer2_new_next_index);
        // Match index wouldn't typically decrease here, but next_index would
        assert_eq!(state.leader.next_index[&2], peer2_new_next_index);
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
            // Basic append, assumes caller ensures consistency (no gaps, correct overwrite logic)
            // For mock, we might not need the complex overwrite/gap detection of InMemoryStorage
            // If the first new entry index is <= current last index, truncate suffix first.
            if let Some(first_new) = entries.first() {
                if let Some(last_existing) = inner.log.last() {
                    if first_new.index <= last_existing.index {
                        // Find the position to truncate at
                        let truncate_at_pos =
                            inner.log.iter().position(|e| e.index >= first_new.index);
                        if let Some(pos) = truncate_at_pos {
                            inner.log.truncate(pos);
                        }
                    }
                    // Basic gap check (more robust check belongs in Raft logic)
                    else if first_new.index > last_existing.index + 1 {
                        return Err(anyhow::anyhow!(
                            "MockStorage: Gap detected between log end ({}) and new entries starting at {}",
                            last_existing.index,
                            first_new.index
                        ));
                    }
                }
            }
            inner.log.extend_from_slice(entries);
            Ok(())
        }

        async fn read_log_entry(&self, index: u64) -> Result<Option<LogEntry>> {
            let inner = self.inner.lock().await;
            // Find entry by absolute index
            Ok(inner
                .log
                .iter()
                .find(|&entry| entry.index == index)
                .cloned())
        }

        async fn read_log_entries(
            &self,
            start_index: u64, // Inclusive, 1-based
            end_index: u64,   // Exclusive, 1-based
        ) -> Result<Vec<LogEntry>> {
            let inner = self.inner.lock().await;
            // Filter by absolute index range
            let result = inner
                .log
                .iter()
                .filter(|&entry| entry.index >= start_index && entry.index < end_index)
                .cloned()
                .collect();
            Ok(result)
        }

        async fn truncate_log_prefix(&mut self, end_index_exclusive: u64) -> Result<()> {
            let mut inner = self.inner.lock().await;
            if end_index_exclusive <= 1 {
                return Ok(()); // Keep all
            }
            // Find the Vec position of the first entry to *keep* (index >= end_index_exclusive)
            let keep_from_pos = inner
                .log
                .iter()
                .position(|entry| entry.index >= end_index_exclusive);

            match keep_from_pos {
                Some(pos) => {
                    // Drain elements *before* this position
                    inner.log.drain(..pos);
                }
                None => {
                    // No entry found with index >= end_index_exclusive, clear the whole log
                    inner.log.clear();
                }
            }
            Ok(())
        }

        async fn truncate_log_suffix(&mut self, start_index_inclusive: u64) -> Result<()> {
            let mut inner = self.inner.lock().await;
            // Find the Vec position of the first entry to *remove* (index >= start_index_inclusive)
            let truncate_at_pos = inner
                .log
                .iter()
                .position(|entry| entry.index >= start_index_inclusive);

            if let Some(pos) = truncate_at_pos {
                inner.log.truncate(pos); // truncate keeps elements *before* pos
            }
            // If no entry >= start_index_inclusive exists, truncate does nothing (keeps all), which is correct.
            Ok(())
        }

        async fn last_log_index(&self) -> Result<u64> {
            let inner = self.inner.lock().await;
            // Get index of the actual last entry
            Ok(inner.log.last().map_or(0, |entry| entry.index))
        }

        async fn last_log_term(&self) -> Result<u64> {
            let inner = self.inner.lock().await;
            // Get term of the actual last entry
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
        let hs_vote = HardState {
            term: 3,
            voted_for: 4,
            commit_index: 5,
        };
        let buf_vote = hs_vote.encode_to_vec();
        let decoded_vote = HardState::decode(buf_vote.as_slice()).unwrap();
        assert_eq!(decoded_vote, hs_vote);

        let hs_no_vote = HardState {
            term: 6,
            voted_for: 0,
            commit_index: 7,
        };
        let buf_no_vote = hs_no_vote.encode_to_vec();
        let decoded_no_vote = HardState::decode(buf_no_vote.as_slice()).unwrap();
        assert_eq!(decoded_no_vote, hs_no_vote);

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

    // --- Storage Trait Edge Case Tests (using MockStorage) ---

    #[tokio::test]
    async fn test_storage_read_boundary_conditions() {
        let mut storage = MockStorage::default();
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

        // Read single entry at boundaries
        assert!(
            storage.read_log_entry(0).await.unwrap().is_none(),
            "Index 0 read"
        );
        assert!(
            storage.read_log_entry(1).await.unwrap().is_some(),
            "Index 1 read"
        );
        assert!(
            storage.read_log_entry(2).await.unwrap().is_some(),
            "Index 2 read"
        );
        assert!(
            storage.read_log_entry(3).await.unwrap().is_none(),
            "Index 3 read"
        );

        // Read range at boundaries
        assert_eq!(
            storage.read_log_entries(1, 1).await.unwrap().len(),
            0,
            "Range 1..1"
        );
        assert_eq!(
            storage.read_log_entries(1, 2).await.unwrap().len(),
            1,
            "Range 1..2"
        ); // Entry 1
        assert_eq!(
            storage.read_log_entries(1, 3).await.unwrap().len(),
            2,
            "Range 1..3"
        ); // Entries 1, 2
        assert_eq!(
            storage.read_log_entries(2, 3).await.unwrap().len(),
            1,
            "Range 2..3"
        ); // Entry 2
        assert_eq!(
            storage.read_log_entries(3, 4).await.unwrap().len(),
            0,
            "Range 3..4"
        );
        assert_eq!(
            storage.read_log_entries(0, 3).await.unwrap().len(),
            2,
            "Range 0..3 -> 1..3"
        );
    }

    #[tokio::test]
    async fn test_storage_truncate_boundary_conditions() {
        let mut storage = MockStorage::default();
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
            LogEntry {
                index: 3,
                term: 2,
                command: vec![3].into(),
            },
        ];
        storage.append_log_entries(&entries).await.unwrap(); // Log: [1, 2, 3]

        // Truncate suffix at boundaries
        storage.truncate_log_suffix(4).await.unwrap(); // Keep up to index 3 (exclusive -> keep 1,2,3). No-op.
        assert_eq!(storage.last_log_index().await.unwrap(), 3);
        storage.truncate_log_suffix(3).await.unwrap(); // Keep up to index 2 (exclusive -> keep 1,2). Removes 3.
        assert_eq!(storage.last_log_index().await.unwrap(), 2);
        storage.truncate_log_suffix(1).await.unwrap(); // Keep up to index 0 (exclusive -> keep none). Removes 1, 2.
        assert_eq!(storage.last_log_index().await.unwrap(), 0);

        // Reset and test truncate prefix
        storage.append_log_entries(&entries).await.unwrap(); // Log: [1, 2, 3]
        storage.truncate_log_prefix(0).await.unwrap(); // Keep >= 0. No-op. (Corrected: Should keep >= 1, effectively no-op)
        assert_eq!(storage.last_log_index().await.unwrap(), 3);
        storage.truncate_log_prefix(1).await.unwrap(); // Keep >= 1. No-op.
        assert_eq!(storage.last_log_index().await.unwrap(), 3);
        storage.truncate_log_prefix(2).await.unwrap(); // Keep >= 2. Removes 1. Log: [2, 3]
        assert_eq!(storage.last_log_index().await.unwrap(), 3);
        assert!(storage.read_log_entry(1).await.unwrap().is_none());
        assert!(storage.read_log_entry(2).await.unwrap().is_some());
        storage.truncate_log_prefix(4).await.unwrap(); // Keep >= 4. Removes 2, 3. Log: []
        assert_eq!(storage.last_log_index().await.unwrap(), 0);
        assert!(storage.read_log_entry(2).await.unwrap().is_none());
        assert!(storage.read_log_entry(3).await.unwrap().is_none());
    }
}
