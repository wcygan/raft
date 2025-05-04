use std::collections::HashMap;

/// Represents a unique identifier for a node in the Raft cluster.
pub type NodeId = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    /// The term number of the leader that sent this entry
    pub term: u64,
    /// The index of the log entry in the leader's log
    pub index: u64,
    /// The command to be applied to the state machine
    pub command: String,
}

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

/// Persistent state that must be saved to stable storage before responding to RPCs
#[derive(Debug, Clone)]
pub struct PersistentState {
    /// The current term of the node
    pub current_term: u64,
    /// The ID of the candidate that received vote in the current term
    pub voted_for: Option<NodeId>,
    /// The log entries, each containing a term and command
    pub log: Vec<LogEntry>,
}

/// Volatile state maintained on all servers but not persisted
#[derive(Debug, Clone)]
pub struct VolatileState {
    /// The index of the highest log entry known to be committed
    pub commit_index: u64,
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
    pub persistent: PersistentState,
    pub volatile: VolatileState,
    pub leader: LeaderState,
    pub server: ServerState,
}

impl RaftState {
    /// Creates a new RaftState with default values.
    pub fn new() -> Self {
        RaftState {
            persistent: PersistentState {
                current_term: 0,
                voted_for: None,
                log: Vec::new(),
            },
            volatile: VolatileState {
                commit_index: 0,
                last_applied: 0,
            },
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
pub trait Storage {}
pub trait Transport {}

/// The main Raft node structure, encapsulating state and logic.
/// Generic over Storage and Transport implementations.
pub struct RaftNode<S: Storage, T: Transport> {
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