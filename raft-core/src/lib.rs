use anyhow::Result;
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
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
pub struct RaftNode<S: Storage + Send + Sync + 'static, T: Transport + Clone + Send + Sync + 'static> {
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
    // TODO: Add internal state like election/heartbeat timers
    /// Tracks votes received during a candidacy period.
    votes_received: HashSet<NodeId>,
}

impl<S: Storage + Send + Sync + 'static, T: Transport + Clone + Send + Sync + 'static> RaftNode<S, T> {
    /// Creates a new RaftNode.
    pub fn new(id: NodeId, config: Config, storage: S, transport: T) -> Self {
        let state = RaftState::new(id);
        // TODO: Load persisted state from storage
        // TODO: Initialize timers (election timeout)
        Self {
            id,
            state,
            config,
            storage,
            transport,
            votes_received: HashSet::new(), // Initialize votes_received
        }
    }

    /// Transitions the node to the Follower state.
    async fn become_follower(&mut self, term: u64, leader_id: Option<NodeId>) -> Result<()> {
        tracing::info!(node_id = %self.id, old_term = self.state.hard_state.term, new_term = term, ?leader_id, "Transitioning to Follower");
        self.state.hard_state.term = term;
        self.state.hard_state.voted_for = 0; // Clear vote when becoming follower
        self.state.server.role = Role::Follower;
        self.state.server.leader_id = leader_id;
        self.votes_received.clear(); // Clear any votes from previous candidacy
        // Persist the updated HardState
        self.storage.save_hard_state(&self.state.hard_state).await?;
        // TODO: Reset election timer
        Ok(())
    }

    /// Transitions the node to the Candidate state.
    async fn become_candidate(&mut self) -> Result<()> {
        let new_term = self.state.hard_state.term + 1;
        tracing::info!(node_id = %self.id, old_term = self.state.hard_state.term, new_term, "Transitioning to Candidate");
        self.state.hard_state.term = new_term;
        self.state.hard_state.voted_for = self.id; // Vote for self
        self.state.server.role = Role::Candidate;
        self.state.server.leader_id = None;

        // Clear previous votes and add self-vote
        self.votes_received.clear();
        self.votes_received.insert(self.id);

        // Persist the updated HardState
        self.storage.save_hard_state(&self.state.hard_state).await?;
        // TODO: Reset election timer
        Ok(())
    }

    /// Transitions the node to the Leader state.
    async fn become_leader(&mut self) -> Result<()> {
        tracing::info!(node_id = %self.id, term = self.state.hard_state.term, "Transitioning to Leader");
        if self.state.server.role != Role::Candidate {
            tracing::warn!(node_id = %self.id, current_role = ?self.state.server.role, "Attempted to become leader from non-candidate state");
            // Depending on stricter state machine, could return Err or just log
        }
        self.state.server.role = Role::Leader;
        self.state.server.leader_id = Some(self.id);
        self.votes_received.clear(); // Clear votes after winning election

        // Initialize leader-specific state (next_index, match_index)
        let last_log_index = self.storage.last_log_index().await?;
        self.state.leader.next_index.clear();
        self.state.leader.match_index.clear();
        for &peer_id in &self.config.peers {
            if peer_id != self.id {
                // Don't track self
                self.state
                    .leader
                    .next_index
                    .insert(peer_id, last_log_index + 1);
                self.state.leader.match_index.insert(peer_id, 0);
            }
        }

        // No change to HardState needed specifically for becoming leader (term/vote handled by candidate transition)
        // TODO: Send initial empty AppendEntries (heartbeat) to peers
        // TODO: Reset heartbeat timer(s)
        Ok(())
    }

    /// Handles an election timeout.
    pub async fn handle_election_timeout(&mut self) -> Result<()> {
        // Leaders and Candidates should not start new elections on election timeout
        if self.state.server.role == Role::Leader {
            tracing::debug!(node_id = %self.id, term = self.state.hard_state.term, "Already leader, ignoring election timeout.");
            return Ok(());
        }
        // If already a candidate, just restart the election for the same term (potentially)
        // Though typically a new timeout implies starting a new election term.
        // Let's proceed with becoming candidate, which handles term increment.

        tracing::info!(node_id = %self.id, term = self.state.hard_state.term, "Election timeout triggered, starting election.");

        // 1. Increment currentTerm, transition to Candidate state
        self.become_candidate().await?; // Handles term increment, voting for self, state persistence

        // TODO: Reset election timer

        // 2. Issue RequestVote RPCs in parallel to all other servers
        let current_term = self.state.hard_state.term;
        let last_log_index = self.storage.last_log_index().await?;
        let last_log_term = self.storage.last_log_term().await?;

        let request = RequestVoteRequest {
            term: current_term,
            candidate_id: self.id,
            last_log_index,
            last_log_term,
        };

        let mut vote_tasks = Vec::new();
        for &peer_id in &self.config.peers {
            if peer_id == self.id {
                continue; // Don't send RPC to self
            }
            let transport = self.transport.clone(); // Clone transport for concurrent use
            let request = request.clone(); // Clone request for each task
            let node_id = self.id;
            
            // Spawn a task for each vote request
            vote_tasks.push(tokio::spawn(async move {
                tracing::debug!(%node_id, %peer_id, term = request.term, "Sending RequestVote RPC");
                match transport.send_request_vote(peer_id, request).await {
                    Ok(response) => {
                        tracing::debug!(%node_id, %peer_id, ?response, "Received RequestVote response");
                        Some((peer_id, response))
                    }
                    Err(e) => {
                        tracing::warn!(%node_id, %peer_id, error = %e, "Failed to send RequestVote RPC");
                        None
                    }
                }
            }));
        }

        // 3. Tally votes
        let mut granted_votes_count = self.votes_received.len(); // Start with self-vote
        let required_votes = (self.config.peers.len() / 2) + 1; // Integer division automatically floors

        for task in vote_tasks {
            // Ensure we only process results if still a candidate in the same term
            if self.state.server.role != Role::Candidate || self.state.hard_state.term != current_term {
                 tracing::info!(node_id = %self.id, term = self.state.hard_state.term, role = ?self.state.server.role, election_term = current_term, "State changed during election, aborting vote count.");
                 // Potentially cancel remaining tasks if possible/needed
                 break; 
            }

            if let Ok(Some((peer_id, response))) = task.await {
                // Check if peer's term is higher, if so, step down
                if response.term > self.state.hard_state.term {
                    tracing::info!(node_id = %self.id, peer_id, peer_term = response.term, "Discovered higher term from vote response, stepping down.");
                    self.become_follower(response.term, None).await?; // leader_id unknown
                    // Stop tallying votes as we are no longer a candidate in this term
                    return Ok(()); 
                }
                
                // If terms match and vote granted, record it
                if response.term == current_term && response.vote_granted {
                    tracing::debug!(node_id = %self.id, peer_id, "Vote granted");
                    if self.votes_received.insert(peer_id) {
                        granted_votes_count += 1;
                    }
                    
                    // Check for majority
                    if granted_votes_count >= required_votes {
                        tracing::info!(node_id = %self.id, term = current_term, votes = granted_votes_count, required = required_votes, "Election won, becoming leader.");
                        self.become_leader().await?;
                        // Stop tallying votes as we've won
                        return Ok(());
                    }
                }
            }
            // Handle task error (e.g., panic) if needed, though spawn captures it
        }

        // If loop finishes without becoming leader (e.g., split vote, network issues, stepped down)
        // The node remains a Candidate (or became Follower) and will eventually time out again.
        tracing::debug!(node_id = %self.id, term = current_term, role = ?self.state.server.role, votes = granted_votes_count, required = required_votes, "Election round finished.");

        Ok(())
    }

    /// Handles a heartbeat timeout (specific to leaders).
    pub async fn handle_heartbeat_timeout(&mut self) -> Result<()> {
        // TODO: Implement heartbeat logic (send AppendEntries to peers)
        tracing::debug!(node_id = %self.id, term = self.state.hard_state.term, "Heartbeat timeout triggered");
        Ok(())
    }

    /// Handles an incoming RequestVote RPC.
    pub async fn handle_request_vote(
        &mut self,
        request: RequestVoteRequest,
    ) -> Result<RequestVoteResponse> {
        tracing::debug!(node_id = %self.id, current_term = self.state.hard_state.term, ?request, "Handling RequestVote RPC");

        let mut vote_granted = false;
        let current_term = self.state.hard_state.term;

        // 1. Reply false if term < currentTerm (§5.1)
        if request.term < current_term {
            tracing::debug!(node_id = %self.id, "Rejecting vote: Request term {} < current term {}", request.term, current_term);
            return Ok(RequestVoteResponse {
                term: current_term,
                vote_granted: false,
            });
        }

        // 2. If RPC request contains term T > currentTerm: set currentTerm = T, convert to follower (§5.1)
        if request.term > current_term {
            tracing::info!(node_id = %self.id, "Received higher term {} from candidate {}. Stepping down.", request.term, request.candidate_id);
            self.become_follower(request.term, None).await?; 
            // HardState is persisted within become_follower
            // Proceed to voting check below with the updated term
        }

        // Now, self.state.hard_state.term == request.term
        let current_voted_for = self.state.hard_state.voted_for;
        let log_ok = self.is_log_up_to_date(request.last_log_term, request.last_log_index).await?;

        // 3. Grant vote if votedFor is null or candidateId, and candidate's log is at least as up-to-date as receiver's log (§5.2, §5.4)
        if (current_voted_for == 0 || current_voted_for == request.candidate_id) && log_ok {
            tracing::info!(node_id = %self.id, term = self.state.hard_state.term, candidate_id = request.candidate_id, "Granting vote");
            vote_granted = true;
            self.state.hard_state.voted_for = request.candidate_id;
            // Persist the vote decision
            self.storage.save_hard_state(&self.state.hard_state).await?;
            // TODO: If vote granted, reset election timer here as well
        } else {
            tracing::debug!(node_id = %self.id, term = self.state.hard_state.term, candidate_id = request.candidate_id, current_voted_for, log_ok, "Rejecting vote: Already voted or log not up-to-date");
        }

        Ok(RequestVoteResponse {
            term: self.state.hard_state.term, // Use potentially updated term
            vote_granted,
        })
    }

    /// Checks if the candidate's log (represented by last_log_term and last_log_index)
    /// is at least as up-to-date as this node's log. (§5.4.1)
    async fn is_log_up_to_date(&self, candidate_last_log_term: u64, candidate_last_log_index: u64) -> Result<bool> {
        let my_last_log_term = self.storage.last_log_term().await?;
        let my_last_log_index = self.storage.last_log_index().await?;

        let log_ok = 
            // Raft determines which of two logs is more up-to-date by comparing the index and term of the last entries in the logs.
            // If the logs have last entries with different terms, then the log with the later term is more up-to-date. 
            candidate_last_log_term > my_last_log_term ||
            // If the logs end with the same term, then whichever log is longer is more up-to-date.
            (candidate_last_log_term == my_last_log_term && candidate_last_log_index >= my_last_log_index);
        
        tracing::trace!(node_id = %self.id, candidate_last_log_term, candidate_last_log_index, my_last_log_term, my_last_log_index, log_ok, "Log up-to-date check");
        Ok(log_ok)
    }

    /// Handles an incoming AppendEntries RPC.
    pub async fn handle_append_entries(
        &mut self,
        request: AppendEntriesRequest,
    ) -> Result<AppendEntriesResponse> {
        // TODO: Implement AppendEntries logic (check term, leader, log consistency)
        tracing::debug!(node_id = %self.id, ?request, "Handling AppendEntries RPC");
        // Placeholder response
        Ok(AppendEntriesResponse {
            term: self.state.hard_state.term,
            success: false,
            match_index: 0, // Or appropriate value
        })
    }
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

    // --- RaftNode Method Tests ---

    // Helper to create a default RaftNode for testing
    fn create_test_node(id: NodeId, peers: Vec<NodeId>) -> RaftNode<MockStorage, MockTransport> {
        let config = Config {
            id,
            peers,
            heartbeat_timeout: 150, // Example value
            election_timeout: 300, // Example value
        };
        let storage = MockStorage::default();
        let transport = MockTransport::default();
        RaftNode::new(id, config, storage, transport)
    }

    #[tokio::test]
    async fn test_handle_request_vote() {
        // --- Scenario 1: Grant Vote --- 
        let mut node1 = create_test_node(1, vec![1, 2, 3]);
        node1.state.hard_state.term = 1;
        node1.storage.save_hard_state(&node1.state.hard_state).await.unwrap(); // Sync mock storage

        let request1 = RequestVoteRequest {
            term: 1,
            candidate_id: 2,
            last_log_index: 0, // Assuming empty logs for simplicity here
            last_log_term: 0,
        };
        let response1 = node1.handle_request_vote(request1).await.unwrap();
        assert!(response1.vote_granted, "Scenario 1: Vote should be granted");
        assert_eq!(response1.term, 1, "Scenario 1: Term should match");
        assert_eq!(node1.state.hard_state.voted_for, 2, "Scenario 1: Should have voted for candidate 2");
        let stored_state1 = node1.storage.read_hard_state().await.unwrap();
        assert_eq!(stored_state1.voted_for, 2, "Scenario 1: Persisted voted_for should be 2");
        assert_eq!(stored_state1.term, 1, "Scenario 1: Persisted term should be 1");

        // --- Scenario 2: Reject Vote (Lower Term) ---
        let mut node2 = create_test_node(1, vec![1, 2, 3]);
        node2.state.hard_state.term = 2;
        node2.storage.save_hard_state(&node2.state.hard_state).await.unwrap();
        
        let request2 = RequestVoteRequest {
            term: 1, // Lower term
            candidate_id: 2,
            last_log_index: 0, 
            last_log_term: 0,
        };
        let response2 = node2.handle_request_vote(request2).await.unwrap();
        assert!(!response2.vote_granted, "Scenario 2: Vote should be rejected (lower term)");
        assert_eq!(response2.term, 2, "Scenario 2: Term should be node's current term (2)");
        assert_eq!(node2.state.hard_state.voted_for, 0, "Scenario 2: Should not have voted");
        let stored_state2 = node2.storage.read_hard_state().await.unwrap();
        assert_eq!(stored_state2.voted_for, 0, "Scenario 2: Persisted voted_for should remain 0");

        // --- Scenario 3: Reject Vote (Already Voted in Term) ---
        let mut node3 = create_test_node(1, vec![1, 2, 3]);
        node3.state.hard_state.term = 1;
        node3.state.hard_state.voted_for = 3; // Already voted for node 3
        node3.storage.save_hard_state(&node3.state.hard_state).await.unwrap();

        let request3 = RequestVoteRequest {
            term: 1,
            candidate_id: 2,
            last_log_index: 0,
            last_log_term: 0,
        };
        let response3 = node3.handle_request_vote(request3).await.unwrap();
        assert!(!response3.vote_granted, "Scenario 3: Vote should be rejected (already voted)");
        assert_eq!(response3.term, 1, "Scenario 3: Term should match");
        assert_eq!(node3.state.hard_state.voted_for, 3, "Scenario 3: Voted_for should remain 3");

        // --- Scenario 4: Grant Vote (Higher Term Received, Step Down) ---
        let mut node4 = create_test_node(1, vec![1, 2, 3]);
        node4.state.hard_state.term = 1;
        node4.state.server.role = Role::Candidate; // Assume was candidate
        node4.storage.save_hard_state(&node4.state.hard_state).await.unwrap();

        let request4 = RequestVoteRequest {
            term: 2, // Higher term
            candidate_id: 2,
            last_log_index: 0, // Assuming log is ok for simplicity
            last_log_term: 0,
        };
        let response4 = node4.handle_request_vote(request4).await.unwrap();
        assert!(response4.vote_granted, "Scenario 4: Vote should be granted in new term");
        assert_eq!(response4.term, 2, "Scenario 4: Term should be updated term (2)");
        assert_eq!(node4.state.hard_state.term, 2, "Scenario 4: Node term should be updated to 2");
        assert_eq!(node4.state.hard_state.voted_for, 2, "Scenario 4: Should have voted for candidate 2 in new term");
        assert_eq!(node4.state.server.role, Role::Follower, "Scenario 4: Node should become Follower");
        let stored_state4 = node4.storage.read_hard_state().await.unwrap();
        assert_eq!(stored_state4.term, 2, "Scenario 4: Persisted term should be 2");
        assert_eq!(stored_state4.voted_for, 2, "Scenario 4: Persisted voted_for should be 2");

        // --- Scenario 5: Reject Vote (Log Not Up-to-Date) --- 
        let mut node5 = create_test_node(1, vec![1, 2, 3]);
        node5.state.hard_state.term = 2;
        let node_log_entries = vec![
            LogEntry { index: 1, term: 1, command: vec![].into() },
            LogEntry { index: 2, term: 2, command: vec![].into() }, // Node's log ends at index 2, term 2
        ];
        node5.storage.append_log_entries(&node_log_entries).await.unwrap();
        node5.storage.save_hard_state(&node5.state.hard_state).await.unwrap();
        
        // Candidate 1: Same term (2), shorter log (index 1)
        let request5a = RequestVoteRequest {
            term: 2, 
            candidate_id: 2,
            last_log_index: 1, // Candidate log shorter
            last_log_term: 2,
        };
        let response5a = node5.handle_request_vote(request5a).await.unwrap();
        assert!(!response5a.vote_granted, "Scenario 5a: Vote should be rejected (candidate log shorter, same term)");
        assert_eq!(response5a.term, 2);
        assert_eq!(node5.state.hard_state.voted_for, 0, "Scenario 5a: Should not have voted");

        // Candidate 2: Lower term (1), longer log (index 3)
        let request5b = RequestVoteRequest {
            term: 2,
            candidate_id: 3,
            last_log_index: 3, 
            last_log_term: 1, // Candidate log has lower term
        };
        let response5b = node5.handle_request_vote(request5b).await.unwrap();
        assert!(!response5b.vote_granted, "Scenario 5b: Vote should be rejected (candidate log older term)");
        assert_eq!(response5b.term, 2);
        assert_eq!(node5.state.hard_state.voted_for, 0, "Scenario 5b: Should still not have voted");

        // Candidate 3: Log is up-to-date (same term, same length)
        let request5c = RequestVoteRequest {
            term: 2, 
            candidate_id: 4,
            last_log_index: 2,
            last_log_term: 2,
        };
        let response5c = node5.handle_request_vote(request5c).await.unwrap();
        assert!(response5c.vote_granted, "Scenario 5c: Vote should be granted (log up-to-date)");
        assert_eq!(response5c.term, 2);
        assert_eq!(node5.state.hard_state.voted_for, 4, "Scenario 5c: Should have voted for 4");
    }

    #[tokio::test]
    async fn test_handle_election_timeout_basic() {
        // --- Scenario 1: Follower transitions to Candidate --- 
        let mut node1 = create_test_node(1, vec![1, 2, 3]);
        node1.state.hard_state.term = 5; // Start at term 5
        node1.state.server.role = Role::Follower;
        node1.storage.save_hard_state(&node1.state.hard_state).await.unwrap(); // Save initial term

        // Simulate election timeout
        node1.handle_election_timeout().await.unwrap();

        // Assertions
        assert_eq!(node1.state.server.role, Role::Candidate, "Scenario 1: Role should be Candidate");
        assert_eq!(node1.state.hard_state.term, 6, "Scenario 1: Term should be incremented to 6");
        assert_eq!(node1.state.hard_state.voted_for, node1.id, "Scenario 1: Should vote for self");
        assert_eq!(node1.votes_received.len(), 1, "Scenario 1: Should have 1 vote (self)");
        assert!(node1.votes_received.contains(&node1.id), "Scenario 1: votes_received should contain self");
        
        // Verify persistence
        let stored_state1 = node1.storage.read_hard_state().await.unwrap();
        assert_eq!(stored_state1.term, 6, "Scenario 1: Persisted term should be 6");
        assert_eq!(stored_state1.voted_for, node1.id, "Scenario 1: Persisted voted_for should be self");

        // --- Scenario 2: Candidate restarts election (becomes candidate again) --- 
        // Simulate another timeout while already a candidate in term 6
        node1.handle_election_timeout().await.unwrap();

        // Assertions: Should start a *new* election in term 7
        assert_eq!(node1.state.server.role, Role::Candidate, "Scenario 2: Role should remain Candidate");
        assert_eq!(node1.state.hard_state.term, 7, "Scenario 2: Term should be incremented to 7");
        assert_eq!(node1.state.hard_state.voted_for, node1.id, "Scenario 2: Should vote for self again in new term");
        assert_eq!(node1.votes_received.len(), 1, "Scenario 2: Votes should reset to 1 (self)"); 
        assert!(node1.votes_received.contains(&node1.id));

        // Verify persistence for term 7
        let stored_state2 = node1.storage.read_hard_state().await.unwrap();
        assert_eq!(stored_state2.term, 7, "Scenario 2: Persisted term should be 7");
        assert_eq!(stored_state2.voted_for, node1.id, "Scenario 2: Persisted voted_for should be self");

        // --- Scenario 3: Leader ignores election timeout ---
        let mut node3 = create_test_node(1, vec![1, 2, 3]);
        node3.state.hard_state.term = 8;
        // Use the internal helper directly for setup simplicity in test
        node3.become_leader().await.unwrap(); // Become leader in term 8
        let initial_leader_state = node3.state.clone();
        let initial_stored_state = node3.storage.read_hard_state().await.unwrap();

        // Simulate election timeout
        node3.handle_election_timeout().await.unwrap();

        // Assertions: State should not change
        assert_eq!(node3.state, initial_leader_state, "Scenario 3: Leader state should not change");
        let final_stored_state = node3.storage.read_hard_state().await.unwrap();
        assert_eq!(final_stored_state, initial_stored_state, "Scenario 3: Leader persisted state should not change");
    }
}
