use anyhow::Result;
use async_trait::async_trait;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use tokio::sync::oneshot;
use tokio::time::Duration;

use wcygan_raft_community_neoeinstein_prost::raft::v1::{
    AppendEntriesRequest, AppendEntriesResponse, HardState, LogEntry, RequestVoteRequest,
    RequestVoteResponse,
};

/// Represents a unique identifier for a node in the Raft cluster.
pub type NodeId = u64;

/// Configuration for a Raft node.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    /// The unique identifier of this node within the cluster.
    pub id: NodeId,
    /// The network addresses of all nodes in the cluster (including this node).
    pub peers: HashMap<NodeId, String>, // Assuming String for address for now
    /// Base election timeout in milliseconds.
    pub election_timeout_min_ms: u64,
    /// Maximum election timeout addition in milliseconds (timeout will be min + random(0..max)).
    pub election_timeout_max_ms: u64,
    /// Heartbeat interval in milliseconds (how often leaders send AppendEntries).
    pub heartbeat_interval_ms: u64,
    // TODO: Add storage configuration (e.g., log directory)
    // TODO: Add transport configuration (e.g., bind address)
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
        let default_state = ServerState {
            role: Role::Follower,
            leader_id: None,
        };

        RaftState {
            hard_state: default_hard_state,
            log: Vec::new(),
            volatile: default_volatile_state,
            leader: default_leader_state,
            node_id,
            server: default_state,
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
pub struct RaftNode<
    S: Storage + Send + Sync + 'static,
    T: Transport + Clone + Send + Sync + 'static,
> {
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

    // Election timer control
    election_timeout_duration_ms: u64, // Current randomized timeout
    election_timer_reset_tx: Option<oneshot::Sender<()>>, // Sends signal to reset timer
}

impl<S: Storage + Send + Sync + 'static, T: Transport + Clone + Send + Sync + 'static>
    RaftNode<S, T>
{
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
            election_timeout_duration_ms: 0, // Will be set by first reset
            election_timer_reset_tx: None,
        }
    }

    /// Calculates the initial election timeout duration.
    /// Called once during initialization.
    fn initialize_election_timeout(&mut self) {
        // Calculate the first timeout AND initialize the reset channel/sender
        let (_initial_rx, initial_duration) = self.reset_election_timer();
        self.election_timeout_duration_ms = initial_duration; // Store the duration

        tracing::debug!(
            node_id = self.config.id,
            timeout_ms = self.election_timeout_duration_ms,
            "Initialized election timeout"
        );
    }

    /// Calculates a random election timeout within the configured range.
    fn calculate_random_timeout(&self) -> u64 {
        let min = self.config.election_timeout_min_ms;
        let max = self.config.election_timeout_max_ms;
        if max <= min {
            // Avoid panic in Rng if max <= min
            return min;
        }
        rand::thread_rng().gen_range(min..=max)
    }

    /// Resets the election timer.
    ///
    /// This sends a signal to the existing timer (if any) to cancel it,
    /// calculates a new random timeout duration, and returns a receiver
    /// that will be notified if a *subsequent* reset occurs before the new
    /// timeout expires, along with the new duration.
    ///
    /// The main run loop is responsible for using this information to manage
    /// the actual `tokio::time::sleep` future.
    fn reset_election_timer(&mut self) -> (oneshot::Receiver<()>, u64) {
        // Signal the old timer task to stop, ignore error if it already completed/was dropped
        if let Some(tx) = self.election_timer_reset_tx.take() {
            let _ = tx.send(());
        }

        // Calculate new random duration
        self.election_timeout_duration_ms = self.calculate_random_timeout();
        tracing::debug!(
            node_id = self.config.id,
            new_timeout_ms = self.election_timeout_duration_ms,
            "Resetting election timer"
        );

        // Create a new channel for the *next* reset signal
        let (new_tx, new_rx) = oneshot::channel::<()>();
        self.election_timer_reset_tx = Some(new_tx);

        (new_rx, self.election_timeout_duration_ms)
    }

    /// Transitions the node to Follower state for the given term.
    /// Persists HardState.
    async fn become_follower(&mut self, term: u64, leader_id: Option<NodeId>) -> Result<()> {
        tracing::info!(
            node_id = self.config.id,
            old_term = self.state.hard_state.term,
            new_term = term,
            ?leader_id,
            "Becoming follower"
        );
        let mut state_changed = false;

        if term > self.state.hard_state.term {
            self.state.hard_state.term = term;
            self.state.hard_state.voted_for = 0; // Reset vote when entering new term
            state_changed = true;
            tracing::debug!(
                node_id = self.config.id,
                term = term,
                "Updated term and reset voted_for"
            );
        } else if term < self.state.hard_state.term {
            // This case should theoretically not happen if callers check terms properly,
            // but handle defensively.
            tracing::warn!(
                node_id = self.config.id,
                current_term = self.state.hard_state.term,
                requested_term = term,
                "Attempted to become follower for older term"
            );
            // Do not change voted_for if term doesn't advance
            return Ok(());
        }
        // If term is the same, we might just be learning the leader_id

        self.state.server.role = Role::Follower;
        self.state.server.leader_id = leader_id;
        self.votes_received.clear(); // No longer applicable

        if state_changed {
            self.storage.save_hard_state(&self.state.hard_state).await?;
            tracing::debug!(
                node_id = self.config.id,
                term = term,
                "Persisted HardState (term updated)"
            );
        }

        // Reset the election timer whenever becoming a follower
        // (e.g., stepping down, starting up, receiving valid AppendEntries)
        let (_reset_rx, _duration) = self.reset_election_timer();
        // TODO: Main loop needs to use _reset_rx and _duration

        Ok(())
    }

    /// Transitions the node to Candidate state.
    /// Increments term, votes for self, persists HardState.
    async fn become_candidate(&mut self) -> Result<()> {
        let new_term = self.state.hard_state.term + 1;
        tracing::info!(
            node_id = self.config.id,
            old_term = self.state.hard_state.term,
            new_term,
            "Becoming candidate"
        );

        self.state.server.role = Role::Candidate;
        self.state.hard_state.term = new_term;
        self.state.server.leader_id = None; // No leader known in candidate state

        // Vote for self
        self.state.hard_state.voted_for = self.config.id;
        self.votes_received.clear();
        self.votes_received.insert(self.config.id); // Record self-vote

        self.storage.save_hard_state(&self.state.hard_state).await?;
        tracing::debug!(
            node_id = self.config.id,
            term = new_term,
            voted_for = self.config.id,
            "Persisted HardState (voted for self)"
        );

        // Reset the election timer to start the timeout for *this* election round
        let (_reset_rx, _duration) = self.reset_election_timer();
        // TODO: Main loop needs to use _reset_rx and _duration

        Ok(())
    }

    /// Transitions the node to Leader state.
    /// Initializes leader-specific volatile state. Does NOT persist.
    async fn become_leader(&mut self) -> Result<()> {
        // Result not needed as no fallible ops -> Changed to Result<()> to allow ?
        // Ensure we are actually a candidate before becoming leader
        // This prevents accidental transitions, e.g. from follower directly
        if self.state.server.role != Role::Candidate {
            tracing::error!(node_id = self.config.id, term = self.state.hard_state.term, role = ?self.state.server.role, "Attempted to become leader from non-candidate state!");
            // Return an error or handle differently? For now, return Ok, but log indicates issue.
            return Ok(()); // Or perhaps Err(anyhow!("Invalid state transition"))?
        }

        tracing::info!(
            node_id = self.config.id,
            term = self.state.hard_state.term,
            "Becoming leader"
        );
        self.state.server.role = Role::Leader;
        self.state.server.leader_id = Some(self.config.id);
        self.votes_received.clear(); // Clear votes from candidacy

        // Initialize leader state (Volatile state on leaders [Figure 2])
        let last_log_idx = self.storage.last_log_index().await?;
        let next_log_index = last_log_idx + 1;

        self.state.leader.next_index.clear();
        self.state.leader.match_index.clear();

        // Initialize next_index for all peers to leader's last log index + 1
        for (peer_id, _peer_addr) in self.config.peers.iter() {
            // Iterate over items
            if *peer_id != self.config.id {
                self.state
                    .leader
                    .next_index
                    .insert(*peer_id, next_log_index);
                self.state.leader.match_index.insert(*peer_id, 0); // Initialized to 0
            }
        }

        // TODO: Send initial empty AppendEntries (heartbeats) to peers immediately.
        // This is crucial to assert authority and prevent other nodes from timing out.
        self.send_heartbeats().await; // Placeholder for heartbeat logic

        // NOTE: Leaders do not have an election timer running.
        // They rely on sending heartbeats. If they fail to maintain quorum,
        // they will eventually step down when receiving RPCs with higher terms.
        // We might need to cancel any pending election timer explicitly here if the
        // run loop doesn't handle the state transition cancellation implicitly.
        if let Some(tx) = self.election_timer_reset_tx.take() {
            let _ = tx.send(()); // Cancel any pending election timer
            tracing::debug!(
                node_id = self.config.id,
                "Cancelled election timer upon becoming leader."
            );
        }
        Ok(())
    }

    /// Placeholder for sending heartbeats (empty AppendEntries)
    async fn send_heartbeats(&self) {
        // TODO: Implement actual heartbeat sending logic
        tracing::debug!(
            node_id = self.config.id,
            term = self.state.hard_state.term,
            "(Placeholder) Sending heartbeats..."
        );
    }

    /// Handles an incoming RequestVote RPC.
    pub async fn handle_request_vote(
        &mut self,
        request: RequestVoteRequest,
    ) -> Result<RequestVoteResponse> {
        tracing::debug!(
            node_id = self.config.id,
            term = self.state.hard_state.term,
            ?request,
            "Handling RequestVote"
        );
        let mut vote_granted = false;
        let mut persist_required = false;

        // 1. Reply false if term < currentTerm (§5.1)
        if request.term < self.state.hard_state.term {
            tracing::debug!(
                node_id = self.config.id,
                req_term = request.term,
                current_term = self.state.hard_state.term,
                "Rejecting vote: older term"
            );
            return Ok(RequestVoteResponse {
                term: self.state.hard_state.term,
                vote_granted: false,
            });
        }

        // If RPC request or response contains term T > currentTerm: set currentTerm = T, convert to follower (§5.1)
        if request.term > self.state.hard_state.term {
            tracing::info!(
                node_id = self.config.id,
                old_term = self.state.hard_state.term,
                new_term = request.term,
                "Stepping down due to higher term in RequestVote"
            );
            // We become follower first, which resets voted_for and updates term.
            // Persistence happens within become_follower.
            self.become_follower(request.term, None).await?;
            // We don't set persist_required = true here, as become_follower handled it.
        }

        // 2. If voted_for is null or candidateId, and candidate's log is at least as
        //    up-to-date as receiver's log, grant vote (§5.2, §5.4)
        let log_is_ok = self
            .is_log_up_to_date(request.last_log_term, request.last_log_index)
            .await?;
        let can_vote = self.state.hard_state.voted_for == 0
            || self.state.hard_state.voted_for == request.candidate_id;

        if can_vote && log_is_ok {
            tracing::debug!(
                node_id = self.config.id,
                term = self.state.hard_state.term,
                candidate_id = request.candidate_id,
                "Granting vote"
            );
            vote_granted = true;
            // Only update voted_for if it wasn't already set to this candidate
            if self.state.hard_state.voted_for != request.candidate_id {
                self.state.hard_state.voted_for = request.candidate_id;
                persist_required = true; // Need to persist the new vote
            }

            // Important: If a server grants a vote, it should reset its election timer, just like when it receives AppendEntries RPCs. (§5.2)
            let (_reset_rx, _duration) = self.reset_election_timer();
            // TODO: Main loop needs to use _reset_rx and _duration
        } else {
            tracing::debug!(node_id = self.config.id, term = self.state.hard_state.term, candidate_id = request.candidate_id, %can_vote, %log_is_ok, "Rejecting vote");
        }

        // Persist if we updated voted_for
        if persist_required {
            self.storage.save_hard_state(&self.state.hard_state).await?;
            tracing::debug!(
                node_id = self.config.id,
                term = self.state.hard_state.term,
                voted_for = self.state.hard_state.voted_for,
                "Persisted HardState (vote granted)"
            );
        }

        Ok(RequestVoteResponse {
            term: self.state.hard_state.term, // Use potentially updated term
            vote_granted,
        })
    }

    /// Checks if the candidate's log is at least as up-to-date as the node's own log.
    /// Raft determines which of two logs is more up-to-date by comparing the
    /// index and term of the last entries in the logs. If the logs have last entries
    /// with different terms, then the log with the later term is more up-to-date.
    /// If the logs end with the same term, then whichever log is longer is more up-to-date.
    /// (§5.4.1)
    async fn is_log_up_to_date(
        &self,
        candidate_last_log_term: u64,
        candidate_last_log_index: u64,
    ) -> Result<bool> {
        // Changed to async fn returning Result<bool>
        let last_log_term = self.storage.last_log_term().await?; // Use await and ?
        let last_log_index = self.storage.last_log_index().await?; // Use await and ?

        Ok(if candidate_last_log_term > last_log_term {
            true // Candidate's term is higher
        } else if candidate_last_log_term == last_log_term {
            candidate_last_log_index >= last_log_index // Same term, candidate's index must be >= ours
        } else {
            false // Candidate's term is lower
        })
    }

    /// Retrieves the last log index from storage.
    async fn last_log_index(&self) -> Result<u64> {
        // TODO: This currently uses the in-memory cache, should use storage trait
        // self.storage.last_log_index().await
        Ok(self.state.log.last().map_or(0, |e| e.index))
    }

    /// Retrieves the last log term from storage.
    async fn last_log_term(&self) -> Result<u64> {
        // TODO: This currently uses the in-memory cache, should use storage trait
        // self.storage.last_log_term().await
        Ok(self.state.log.last().map_or(0, |e| e.term))
    }

    // TODO: This should ideally not be public, but is needed for direct testing
    //       of the timeout logic until the main run loop manages timers.
    /// Handles the election timeout event, triggering a potential candidacy.
    pub async fn handle_election_timeout(&mut self) -> Result<()> {
        // Ignore timeout if already leader
        if self.state.server.role == Role::Leader {
            tracing::trace!(
                node_id = self.config.id,
                "Ignoring election timeout as leader."
            );
            return Ok(());
        }

        tracing::info!(
            node_id = self.config.id,
            term = self.state.hard_state.term,
            "Election timeout triggered."
        );

        // Transition to candidate
        self.become_candidate().await?;

        // Send RequestVote RPCs to all peers concurrently
        let current_term = self.state.hard_state.term;
        let last_log_idx = self.storage.last_log_index().await?;
        let last_log_term = self.storage.last_log_term().await?;

        let request = RequestVoteRequest {
            term: current_term,
            candidate_id: self.config.id,
            last_log_index: last_log_idx,
            last_log_term,
        };

        let mut vote_responses = Vec::new();
        let peer_ids: Vec<NodeId> = self.config.peers.keys().copied().collect(); // Collect keys

        // Spawn tasks for each peer
        for peer_id in peer_ids {
            if peer_id == self.config.id {
                continue;
            } // Don't send to self
            let transport_clone = self.transport.clone(); // Clone transport for the task
            let request_clone = request.clone(); // Clone request for the task
            let node_id = self.config.id; // Capture node_id

            let task = tokio::spawn(async move {
                tracing::debug!(target: "election", node_id, peer_id, ?request_clone, "Sending RequestVote RPC");
                transport_clone
                    .send_request_vote(peer_id, request_clone)
                    .await
            });
            vote_responses.push(task);
        }

        // Tally votes
        let mut votes_granted_count = self.votes_received.len(); // Start with self-vote
        let required_votes = (self.config.peers.len() / 2) + 1;

        // --- Optimization/Edge Case: Single Node Cluster ---
        // If we are the only node, the self-vote is sufficient for majority.
        if self.state.server.role == Role::Candidate
            && self.state.hard_state.term == current_term
            && votes_granted_count >= required_votes
            && self.config.peers.is_empty()
        {
            // Check if it's a single-node cluster
            tracing::info!(target: "election", node_id = self.config.id, term = current_term, votes = votes_granted_count, "Single node cluster: becoming leader immediately after candidacy.");
            self.become_leader().await?;
            // Return early as no need to process non-existent peer responses
            return Ok(());
        }
        // --- End Single Node Cluster Check ---

        for handle in vote_responses {
            // If we are no longer a candidate (e.g., stepped down due to higher term in response),
            // stop processing votes for this election.
            if self.state.server.role != Role::Candidate
                || self.state.hard_state.term != current_term
            {
                tracing::info!(node_id = self.config.id, old_term = current_term, new_term = self.state.hard_state.term, old_role = ?Role::Candidate, new_role = ?self.state.server.role, "Aborting vote count due to state change.");
                break; // Exit the loop
            }

            match handle.await {
                Ok(Ok(response)) => {
                    tracing::debug!(target: "election", node_id = self.config.id, ?response, "Received RequestVote response");
                    // Step down if response term is higher
                    if response.term > self.state.hard_state.term {
                        tracing::info!(
                            node_id = self.config.id,
                            old_term = self.state.hard_state.term,
                            new_term = response.term,
                            "Stepping down due to higher term in RequestVote response"
                        );
                        self.become_follower(response.term, None).await?;
                        // Important: Break here because state changed, vote is irrelevant now
                        break;
                    } else if response.vote_granted {
                        votes_granted_count += 1;
                        // Record the voter - though not strictly necessary for just counting
                        // self.votes_received.insert(peer_id_from_response); // Need peer id from response/context
                        tracing::debug!(target: "election", node_id = self.config.id, term = current_term, votes_granted_count, required_votes, "Vote granted");
                    }
                }
                Ok(Err(e)) => {
                    tracing::warn!(target: "election", node_id = self.config.id, error = ?e, "Error receiving RequestVote response");
                }
                Err(e) => {
                    // Task panicked or was cancelled
                    tracing::warn!(target: "election", node_id = self.config.id, error = ?e, "RequestVote task failed");
                }
            }

            // Check if majority achieved after processing each vote
            if self.state.server.role == Role::Candidate
                && self.state.hard_state.term == current_term
                && votes_granted_count >= required_votes
            {
                tracing::info!(target: "election", node_id = self.config.id, term = current_term, votes = votes_granted_count, "Majority achieved, becoming leader");
                self.become_leader().await?;
                break; // Stop processing votes once leader
            }
        }

        // If loop finished without becoming leader, we remain candidate (and will eventually time out again)
        if self.state.server.role == Role::Candidate {
            tracing::debug!(target: "election", node_id = self.config.id, term = current_term, votes = votes_granted_count, "Election finished without becoming leader");
            // The election timer would have been reset in become_candidate
            // The main loop would start a new timeout based on that reset
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::collections::HashMap;
    use tokio::time::Duration;

    // Helper function to create a default configuration for tests
    fn default_config(id: NodeId, peers: HashMap<NodeId, String>) -> Config {
        Config {
            id,
            peers,
            heartbeat_interval_ms: 150,   // Example value
            election_timeout_min_ms: 300, // Example value
            election_timeout_max_ms: 600, // Example value
        }
    }

    // Mock Storage implementation for testing
    #[derive(Default, Debug)]
    struct MockStorage {
        hard_state: HardState,
        log: Vec<LogEntry>,
    }

    impl MockStorage {
        // Helper to directly set log for specific test setups
        fn set_log(&mut self, log: Vec<LogEntry>) {
            self.log = log;
        }
        // Helper to directly set hard state for specific test setups
        fn set_hard_state(&mut self, hs: HardState) {
            self.hard_state = hs;
        }
    }

    #[async_trait]
    impl Storage for MockStorage {
        async fn read_hard_state(&self) -> Result<HardState> {
            Ok(self.hard_state)
        }

        async fn save_hard_state(&mut self, hs: &HardState) -> Result<()> {
            self.hard_state = *hs;
            Ok(())
        }

        async fn append_log_entries(&mut self, entries: &[LogEntry]) -> Result<()> {
            self.log.extend_from_slice(entries);
            Ok(())
        }

        async fn read_log_entry(&self, index: u64) -> Result<Option<LogEntry>> {
            Ok(self.log.iter().find(|e| e.index == index).cloned())
        }

        async fn read_log_entries(
            &self,
            start_index: u64,
            end_index_exclusive: u64,
        ) -> Result<Vec<LogEntry>> {
            Ok(self
                .log
                .iter()
                .filter(|e| e.index >= start_index && e.index < end_index_exclusive)
                .cloned()
                .collect())
        }

        async fn truncate_log_prefix(&mut self, end_index_exclusive: u64) -> Result<()> {
            self.log.retain(|entry| entry.index >= end_index_exclusive);
            Ok(())
        }

        async fn truncate_log_suffix(&mut self, start_index_inclusive: u64) -> Result<()> {
            self.log.retain(|entry| entry.index < start_index_inclusive);
            Ok(())
        }

        async fn last_log_index(&self) -> Result<u64> {
            Ok(self.log.last().map_or(0, |e| e.index))
        }

        async fn last_log_term(&self) -> Result<u64> {
            Ok(self.log.last().map_or(0, |e| e.term))
        }
    }

    // Mock Transport implementation for testing
    #[derive(Clone, Default)]
    struct MockTransport; // Simple mock, doesn't actually send/receive

    #[async_trait]
    impl Transport for MockTransport {
        async fn send_append_entries(
            &self,
            _peer_id: NodeId,
            _request: AppendEntriesRequest,
        ) -> Result<AppendEntriesResponse> {
            unimplemented!("MockTransport send_append_entries not implemented for this test")
        }

        async fn send_request_vote(
            &self,
            _peer_id: NodeId,
            _request: RequestVoteRequest,
        ) -> Result<RequestVoteResponse> {
            unimplemented!("MockTransport send_request_vote not implemented for this test")
        }
    }

    // Helper to create a RaftNode with mock storage/transport
    // Returns mutable node for test setup convenience
    fn create_test_node(
        id: NodeId,
        peers: HashMap<NodeId, String>,
    ) -> RaftNode<MockStorage, MockTransport> {
        let config = default_config(id, peers);
        let storage = MockStorage::default();
        let transport = MockTransport::default();
        RaftNode::new(id, config, storage, transport)
    }

    #[tokio::test]
    async fn test_handle_request_vote_grant_vote() {
        let mut peers = HashMap::new();
        peers.insert(1, "addr1".to_string());
        peers.insert(2, "addr2".to_string());
        peers.insert(3, "addr3".to_string());

        let mut node = create_test_node(1, peers.clone());
        node.state.hard_state.term = 5;
        node.state.hard_state.voted_for = 0; // Ensure node hasn't voted yet
        node.storage.set_hard_state(node.state.hard_state); // Sync mock storage
        node.storage.set_log(vec![LogEntry {
            term: 5,
            index: 10,
            command: Bytes::new(),
        }]); // Sync mock storage log

        let request = RequestVoteRequest {
            term: 5, // Same term
            candidate_id: 2,
            last_log_index: 10,
            last_log_term: 5,
        };

        let response = node.handle_request_vote(request).await.unwrap();

        assert!(response.vote_granted);
        assert_eq!(response.term, 5);
        assert_eq!(node.state.hard_state.voted_for, 2); // Check voted_for updated in memory
        assert_eq!(node.state.hard_state.term, 5); // Term should not change
        // Check persistence
        let stored_state = node.storage.read_hard_state().await.unwrap();
        assert_eq!(stored_state.voted_for, 2);
        assert_eq!(stored_state.term, 5);
    }

    #[tokio::test]
    async fn test_handle_request_vote_reject_term() {
        let mut peers = HashMap::new();
        peers.insert(1, "addr1".to_string());
        peers.insert(2, "addr2".to_string());

        let mut node = create_test_node(1, peers.clone());
        node.state.hard_state.term = 5;
        node.state.hard_state.voted_for = 0;
        node.storage.set_hard_state(node.state.hard_state); // Sync mock storage

        let request = RequestVoteRequest {
            term: 4, // Lower term
            candidate_id: 2,
            last_log_index: 10,
            last_log_term: 4,
        };

        let response = node.handle_request_vote(request).await.unwrap();

        assert!(!response.vote_granted);
        assert_eq!(response.term, 5); // Returns own higher term
        assert_eq!(node.state.hard_state.voted_for, 0); // Should not have voted
        // Check persistence (should be unchanged)
        let stored_state = node.storage.read_hard_state().await.unwrap();
        assert_eq!(stored_state.voted_for, 0);
        assert_eq!(stored_state.term, 5);
    }

    #[tokio::test]
    async fn test_handle_request_vote_step_down() {
        let mut peers = HashMap::new();
        peers.insert(1, "addr1".to_string());
        peers.insert(2, "addr2".to_string());

        let mut node = create_test_node(1, peers.clone());
        node.state.hard_state.term = 5;
        node.state.hard_state.voted_for = 1; // Voted for self in term 5
        node.state.server.role = Role::Candidate; // Was a candidate
        node.storage.set_hard_state(node.state.hard_state);

        let request = RequestVoteRequest {
            term: 6, // Higher term
            candidate_id: 2,
            last_log_index: 10, // Assume log is ok
            last_log_term: 6,
        };

        let response = node.handle_request_vote(request).await.unwrap();

        assert!(response.vote_granted); // Should grant vote as term is higher
        assert_eq!(response.term, 6);
        assert_eq!(node.state.hard_state.term, 6); // Term updated in memory
        assert_eq!(node.state.hard_state.voted_for, 2); // Voted for new candidate in memory
        assert_eq!(node.state.server.role, Role::Follower); // Stepped down
        // Check persistence
        let stored_state = node.storage.read_hard_state().await.unwrap();
        assert_eq!(stored_state.term, 6);
        assert_eq!(stored_state.voted_for, 2);
    }

    #[tokio::test]
    async fn test_handle_request_vote_reject_already_voted() {
        let mut peers = HashMap::new();
        peers.insert(1, "addr1".to_string());
        peers.insert(2, "addr2".to_string());
        peers.insert(3, "addr3".to_string());

        let mut node = create_test_node(1, peers.clone());
        node.state.hard_state.term = 5;
        node.state.hard_state.voted_for = 3; // Already voted for node 3
        node.storage.set_hard_state(node.state.hard_state);

        let request = RequestVoteRequest {
            term: 5, // Same term
            candidate_id: 2,
            last_log_index: 10,
            last_log_term: 5,
        };

        let response = node.handle_request_vote(request).await.unwrap();

        assert!(!response.vote_granted);
        assert_eq!(response.term, 5);
        assert_eq!(node.state.hard_state.voted_for, 3); // Vote should not change
        // Check persistence (should be unchanged)
        let stored_state = node.storage.read_hard_state().await.unwrap();
        assert_eq!(stored_state.voted_for, 3);
        assert_eq!(stored_state.term, 5);
    }

    #[tokio::test]
    async fn test_handle_request_vote_reject_log_less_up_to_date() {
        let mut peers = HashMap::new();
        peers.insert(1, "addr1".to_string());
        peers.insert(2, "addr2".to_string());

        let mut node = create_test_node(1, peers.clone());
        node.state.hard_state.term = 5;
        node.state.hard_state.voted_for = 0;
        node.storage.set_hard_state(node.state.hard_state); // Set term
        node.storage.set_log(vec![LogEntry {
            term: 5,
            index: 11,
            command: Bytes::new(),
        }]); // Own log is newer

        let request = RequestVoteRequest {
            term: 5,
            candidate_id: 2,
            last_log_index: 10, // Candidate's log is older (index)
            last_log_term: 5,
        };

        let response = node.handle_request_vote(request).await.unwrap();

        assert!(!response.vote_granted);
        assert_eq!(response.term, 5);
        assert_eq!(node.state.hard_state.voted_for, 0); // Should not vote
        assert_eq!(node.storage.read_hard_state().await.unwrap().voted_for, 0); // Verify not persisted

        // --- Test case where term is older ---
        let request_older_term = RequestVoteRequest {
            term: 5,
            candidate_id: 2,
            last_log_index: 11,
            last_log_term: 4, // Candidate's log is older (term)
        };

        let response_older_term = node.handle_request_vote(request_older_term).await.unwrap();
        assert!(!response_older_term.vote_granted);
        assert_eq!(response_older_term.term, 5);
        assert_eq!(node.state.hard_state.voted_for, 0); // Should not vote
        assert_eq!(node.storage.read_hard_state().await.unwrap().voted_for, 0); // Verify not persisted
    }

    #[tokio::test]
    async fn test_is_log_up_to_date() {
        let mut peers = HashMap::new();
        peers.insert(1, "addr1".to_string());
        let mut node = create_test_node(1, peers);

        // Use MockStorage methods to set state for test
        node.storage.set_log(vec![
            LogEntry {
                term: 4,
                index: 9,
                command: Bytes::new(),
            },
            LogEntry {
                term: 5,
                index: 10,
                command: Bytes::new(),
            },
        ]);

        // Candidate log has higher term
        assert!(node.is_log_up_to_date(6, 11).await.unwrap());

        // Same term, candidate log has higher index
        assert!(node.is_log_up_to_date(5, 11).await.unwrap());

        // Same term, same index
        assert!(node.is_log_up_to_date(5, 10).await.unwrap());

        // Own log has higher term (candidate term lower)
        assert!(!node.is_log_up_to_date(4, 12).await.unwrap());

        // Same term, own log has higher index (candidate index lower)
        assert!(!node.is_log_up_to_date(5, 9).await.unwrap());

        // --- Edge case: Empty local log ---
        node.storage.set_log(vec![]); // Use helper to clear log
        // Candidate log always more up-to-date than empty
        assert!(node.is_log_up_to_date(1, 1).await.unwrap());
        assert!(node.is_log_up_to_date(0, 0).await.unwrap()); // Candidate also empty/non-existent log
    }

    #[tokio::test]
    async fn test_become_follower() {
        let mut peers = HashMap::new();
        peers.insert(1, "addr1".to_string());
        let mut node = create_test_node(1, peers.clone());
        node.state.server.role = Role::Candidate;
        node.state.hard_state.term = 3;
        node.state.hard_state.voted_for = 1;
        node.state.server.leader_id = None;
        node.votes_received.insert(1);
        node.storage.set_hard_state(node.state.hard_state); // Sync storage

        node.become_follower(5, Some(2)).await.unwrap();

        assert_eq!(node.state.server.role, Role::Follower);
        assert_eq!(node.state.hard_state.term, 5);
        assert_eq!(node.state.hard_state.voted_for, 0); // Check voted_for reset in memory
        assert_eq!(node.state.server.leader_id, Some(2));
        assert!(node.votes_received.is_empty());
        // Check persistence
        let stored_state = node.storage.read_hard_state().await.unwrap();
        assert_eq!(stored_state.term, 5);
        assert_eq!(stored_state.voted_for, 0);
    }

    #[tokio::test]
    async fn test_become_candidate() {
        let mut peers = HashMap::new();
        peers.insert(1, "addr1".to_string());
        peers.insert(2, "addr2".to_string()); // Need peers for vote count check later

        let mut node = create_test_node(1, peers.clone());
        node.state.server.role = Role::Follower;
        node.state.hard_state.term = 3;
        node.state.hard_state.voted_for = 0;
        node.state.server.leader_id = Some(2);
        node.storage.set_hard_state(node.state.hard_state);

        node.become_candidate().await.unwrap();

        assert_eq!(node.state.server.role, Role::Candidate);
        assert_eq!(node.state.hard_state.term, 4);
        assert_eq!(node.state.hard_state.voted_for, 1); // Check voted for self in memory
        assert_eq!(node.state.server.leader_id, None);
        assert!(node.votes_received.contains(&1));
        assert_eq!(node.votes_received.len(), 1);
        // Check persistence
        let stored_state = node.storage.read_hard_state().await.unwrap();
        assert_eq!(stored_state.term, 4);
        assert_eq!(stored_state.voted_for, 1);
    }

    #[tokio::test]
    async fn test_become_leader() {
        let mut peers = HashMap::new();
        peers.insert(1, "addr1".to_string());
        peers.insert(2, "addr2".to_string());
        peers.insert(3, "addr3".to_string());

        let mut node = create_test_node(1, peers.clone());
        // Set state as if it just won an election in term 4
        node.state.server.role = Role::Candidate;
        node.state.hard_state.term = 4;
        node.state.hard_state.voted_for = 1;
        node.votes_received.insert(1);
        node.votes_received.insert(2); // Assume got vote from 2
        node.storage.set_hard_state(node.state.hard_state);
        node.storage.set_log(vec![LogEntry {
            term: 4,
            index: 5,
            command: Bytes::new(),
        }]);

        node.become_leader()
            .await
            .expect("become_leader should succeed");

        assert_eq!(node.state.server.role, Role::Leader);
        assert_eq!(node.state.server.leader_id, Some(1));
        assert!(node.votes_received.is_empty()); // Should be cleared

        // Check leader state initialization (nextIndex, matchIndex)
        assert_eq!(node.state.leader.next_index.get(&2), Some(&6)); // nextIndex = last log index + 1
        assert_eq!(node.state.leader.next_index.get(&3), Some(&6));
        assert_eq!(node.state.leader.match_index.get(&2), Some(&0)); // matchIndex = 0
        assert_eq!(node.state.leader.match_index.get(&3), Some(&0));
        assert!(!node.state.leader.next_index.contains_key(&1)); // Should not track self

        // Verify no unexpected persistence changes occurred in become_leader itself
        let stored_state = node.storage.read_hard_state().await.unwrap();
        assert_eq!(stored_state.term, 4);
        assert_eq!(stored_state.voted_for, 1);
    }

    // Add more tests for handle_election_timeout later, requires MockTransport improvements
    // and potentially timer control.

    // TODO: Test timer reset logic
    #[tokio::test]
    async fn test_election_timer_reset() {
        let mut peers = HashMap::new();
        peers.insert(1, "addr1".to_string());
        let mut node = create_test_node(1, peers);
        node.config.election_timeout_min_ms = 100;
        node.config.election_timeout_max_ms = 200;

        // Initialize timer state properly for the test
        node.initialize_election_timeout();

        // Initial timeout duration
        let initial_timeout = node.election_timeout_duration_ms;
        assert!(initial_timeout >= 100 && initial_timeout <= 200);
        assert!(node.election_timer_reset_tx.is_some()); // Sender should exist after init

        // First reset
        let (mut rx1, duration1) = node.reset_election_timer(); // Make rx1 mutable
        assert!(duration1 >= 100 && duration1 <= 200);
        let _tx1 = node.election_timer_reset_tx.as_ref().unwrap(); // Check sender exists
        // rx1 should not be ready yet
        assert!(rx1.try_recv().is_err());

        // Second reset
        let (mut rx2, duration2) = node.reset_election_timer(); // Make rx2 mutable
        assert!(duration2 >= 100 && duration2 <= 200);
        let _tx2 = node.election_timer_reset_tx.as_ref().unwrap();
        assert!(rx2.try_recv().is_err());

        // Check that the first receiver (rx1) was signalled by the second reset
        match tokio::time::timeout(Duration::from_millis(10), &mut rx1).await {
            // Pass mutable ref
            Ok(Ok(())) => { /* Signal received as expected */ }
            _ => panic!("rx1 did not receive reset signal from second call"),
        }

        // Third reset, signal should go to rx2
        let (mut rx3, _duration3) = node.reset_election_timer(); // Make rx3 mutable
        match tokio::time::timeout(Duration::from_millis(10), &mut rx2).await {
            // Pass mutable ref
            Ok(Ok(())) => { /* Signal received as expected */ }
            _ => panic!("rx2 did not receive reset signal from third call"),
        }
        assert!(rx3.try_recv().is_err());
    }
}
