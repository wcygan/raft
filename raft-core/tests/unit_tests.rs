use bytes::Bytes;
use raft_core::{Config, NodeId, RaftNode, Role, Storage}; // Core types + Storage trait
use raft_storage::InMemoryStorage; // Use existing storage mock
use raft_transport::mock::{MockTransport, TransportRegistry}; // Use existing transport mock
use raft_transport::network::NetworkOptions; // Correct path for NetworkOptions
use std::collections::HashMap;
use std::sync::Arc; // Ensure Arc is imported
use tokio::time::Duration;
use wcygan_raft_community_neoeinstein_prost::raft::v1::{
    HardState,
    LogEntry,
    RequestVoteRequest, // Import needed proto types
};

// Helper function to create a default configuration for tests
fn default_config(id: NodeId, peers: HashMap<NodeId, String>) -> Config {
    Config {
        id,
        peers,
        heartbeat_interval_ms: 150,
        election_timeout_min_ms: 300,
        election_timeout_max_ms: 600,
    }
}

// Helper to create a RaftNode with mock storage/transport
async fn create_test_node(
    id: NodeId,
    peers: HashMap<NodeId, String>,
    registry: Arc<TransportRegistry>, // Pass the shared registry as Arc
) -> RaftNode<InMemoryStorage, MockTransport> {
    let config = default_config(id, peers);
    let storage = InMemoryStorage::new(); // Use the correct constructor
    let (transport, _receivers) =
        MockTransport::create_with_options(id, NetworkOptions::default(), registry).await;
    RaftNode::new(id, config, storage, transport)
}

// TODO: Move all tests from raft-core/src/lib.rs here and adapt them.

#[tokio::test]
async fn test_handle_request_vote_grant_vote() {
    let registry = Arc::new(TransportRegistry::new());
    let mut peers = HashMap::new();
    peers.insert(1, "addr1".to_string());
    peers.insert(2, "addr2".to_string());
    peers.insert(3, "addr3".to_string());

    let mut node = create_test_node(1, peers.clone(), registry.clone()).await;
    let initial_hs = HardState {
        term: 5,
        voted_for: 0,
        commit_index: 0,
    };
    node.storage.save_hard_state(&initial_hs).await.unwrap();

    // Append log entries 1 through 10 sequentially
    let entries: Vec<LogEntry> = (1..=10)
        .map(|i| LogEntry {
            term: 5, // Assume all entries up to 10 are in term 5 for simplicity
            index: i,
            command: Bytes::new(),
        })
        .collect();
    node.storage.append_log_entries(&entries).await.unwrap();

    node.state.hard_state = initial_hs; // Sync in-memory state too

    let request = RequestVoteRequest {
        term: 5, // Same term
        candidate_id: 2,
        last_log_index: 10,
        last_log_term: 5,
    };

    let response = node.handle_request_vote(request).await.unwrap();

    assert!(response.vote_granted);
    assert_eq!(response.term, 5);
    // Check persisted state
    let stored_state = node.storage.read_hard_state().await.unwrap();
    assert_eq!(stored_state.voted_for, 2);
    assert_eq!(stored_state.term, 5);
    // Check in-memory state (should also be updated by handler)
    assert_eq!(node.state.hard_state.voted_for, 2);
    assert_eq!(node.state.hard_state.term, 5);
}

#[tokio::test]
async fn test_handle_request_vote_reject_term() {
    let registry = Arc::new(TransportRegistry::new());
    let mut peers = HashMap::new();
    peers.insert(1, "addr1".to_string());
    peers.insert(2, "addr2".to_string());

    let mut node = create_test_node(1, peers.clone(), registry.clone()).await;
    let initial_hs = HardState {
        term: 5,
        voted_for: 0,
        commit_index: 0,
    };
    node.storage.save_hard_state(&initial_hs).await.unwrap();
    node.state.hard_state = initial_hs;

    let request = RequestVoteRequest {
        term: 4, // Lower term
        candidate_id: 2,
        last_log_index: 10,
        last_log_term: 4,
    };

    let response = node.handle_request_vote(request).await.unwrap();

    assert!(!response.vote_granted);
    assert_eq!(response.term, 5); // Returns own higher term
                                  // Check persisted state (should be unchanged)
    let stored_state = node.storage.read_hard_state().await.unwrap();
    assert_eq!(stored_state.voted_for, 0);
    assert_eq!(stored_state.term, 5);
    // Check in-memory state
    assert_eq!(node.state.hard_state.voted_for, 0);
}

#[tokio::test]
async fn test_handle_request_vote_step_down() {
    let registry = Arc::new(TransportRegistry::new());
    let mut peers = HashMap::new();
    peers.insert(1, "addr1".to_string());
    peers.insert(2, "addr2".to_string());

    let mut node = create_test_node(1, peers.clone(), registry.clone()).await;
    let initial_hs = HardState {
        term: 5,
        voted_for: 1, // Voted for self in term 5
        commit_index: 0,
    };
    node.storage.save_hard_state(&initial_hs).await.unwrap();
    node.state.hard_state = initial_hs;
    node.state.server.role = Role::Candidate; // Was a candidate

    let request = RequestVoteRequest {
        term: 6, // Higher term
        candidate_id: 2,
        last_log_index: 10, // Assume log is ok
        last_log_term: 6,
    };

    let response = node.handle_request_vote(request).await.unwrap();

    assert!(response.vote_granted); // Should grant vote as term is higher
    assert_eq!(response.term, 6);
    assert_eq!(node.state.server.role, Role::Follower); // Stepped down in memory
                                                        // Check persistence
    let stored_state = node.storage.read_hard_state().await.unwrap();
    assert_eq!(stored_state.term, 6);
    assert_eq!(stored_state.voted_for, 2);
    // Check in-memory state
    assert_eq!(node.state.hard_state.term, 6);
    assert_eq!(node.state.hard_state.voted_for, 2);
}

#[tokio::test]
async fn test_handle_request_vote_reject_already_voted() {
    let registry = Arc::new(TransportRegistry::new());
    let mut peers = HashMap::new();
    peers.insert(1, "addr1".to_string());
    peers.insert(2, "addr2".to_string());
    peers.insert(3, "addr3".to_string());

    let mut node = create_test_node(1, peers.clone(), registry.clone()).await;
    let initial_hs = HardState {
        term: 5,
        voted_for: 3, // Already voted for node 3
        commit_index: 0,
    };
    node.storage.save_hard_state(&initial_hs).await.unwrap();
    node.state.hard_state = initial_hs;

    let request = RequestVoteRequest {
        term: 5, // Same term
        candidate_id: 2,
        last_log_index: 10,
        last_log_term: 5,
    };

    let response = node.handle_request_vote(request).await.unwrap();

    assert!(!response.vote_granted);
    assert_eq!(response.term, 5);
    // Check persistence (should be unchanged)
    let stored_state = node.storage.read_hard_state().await.unwrap();
    assert_eq!(stored_state.voted_for, 3);
    assert_eq!(stored_state.term, 5);
    // Check in-memory state
    assert_eq!(node.state.hard_state.voted_for, 3);
}

#[tokio::test]
async fn test_handle_request_vote_reject_log_less_up_to_date() {
    let registry = Arc::new(TransportRegistry::new());
    let mut peers = HashMap::new();
    peers.insert(1, "addr1".to_string());
    peers.insert(2, "addr2".to_string());

    let mut node = create_test_node(1, peers.clone(), registry.clone()).await;
    let initial_hs = HardState {
        term: 5,
        voted_for: 0,
        commit_index: 0,
    };
    node.storage.save_hard_state(&initial_hs).await.unwrap();

    // Append log entries 1 through 11 sequentially
    let entries: Vec<LogEntry> = (1..=11)
        .map(|i| LogEntry {
            term: 5, // Assume term 5 for simplicity
            index: i,
            command: Bytes::new(),
        })
        .collect();
    node.storage.append_log_entries(&entries).await.unwrap();

    node.state.hard_state = initial_hs;

    // Test case: Candidate's log is shorter (index 10 vs 11)
    let request_shorter = RequestVoteRequest {
        term: 5,
        candidate_id: 2,
        last_log_index: 10,
        last_log_term: 5,
    };
    let response_shorter = node.handle_request_vote(request_shorter).await.unwrap();
    assert!(!response_shorter.vote_granted);
    assert_eq!(response_shorter.term, 5);
    let stored_state_1 = node.storage.read_hard_state().await.unwrap();
    assert_eq!(stored_state_1.voted_for, 0);
    assert_eq!(node.state.hard_state.voted_for, 0);

    // Test case: Candidate's log has older term (term 4 vs 5)
    let request_older_term = RequestVoteRequest {
        term: 5,
        candidate_id: 2,
        last_log_index: 11, // Same length
        last_log_term: 4,   // Older term
    };
    let response_older_term = node.handle_request_vote(request_older_term).await.unwrap();
    assert!(!response_older_term.vote_granted);
    assert_eq!(response_older_term.term, 5);
    let stored_state_2 = node.storage.read_hard_state().await.unwrap();
    assert_eq!(stored_state_2.voted_for, 0);
    assert_eq!(node.state.hard_state.voted_for, 0);
}

#[tokio::test]
async fn test_is_log_up_to_date() {
    let registry = Arc::new(TransportRegistry::new());
    let mut peers = HashMap::new();
    peers.insert(1, "addr1".to_string());
    let mut node = create_test_node(1, peers.clone(), registry.clone()).await;

    // Append log entries 1 through 10 sequentially
    // Use term 4 for index 9 and term 5 for index 10
    let entries: Vec<LogEntry> = (1..=10)
        .map(|i| LogEntry {
            term: if i <= 9 { 4 } else { 5 },
            index: i,
            command: Bytes::new(),
        })
        .collect();
    node.storage.append_log_entries(&entries).await.unwrap();

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
    let registry_empty = Arc::new(TransportRegistry::new());
    let mut peers_empty = HashMap::new();
    peers_empty.insert(1, "addr1".to_string());
    let node_empty = create_test_node(1, peers_empty.clone(), registry_empty.clone()).await;

    // Candidate log always more up-to-date than empty
    assert!(node_empty.is_log_up_to_date(1, 1).await.unwrap());
    assert!(node_empty.is_log_up_to_date(0, 0).await.unwrap()); // Candidate also empty/non-existent log
}

#[tokio::test]
async fn test_become_follower() {
    let registry = Arc::new(TransportRegistry::new());
    let mut peers = HashMap::new();
    peers.insert(1, "addr1".to_string());
    let mut node = create_test_node(1, peers.clone(), registry.clone()).await;

    // Set initial state (as if candidate)
    let initial_hs = HardState {
        term: 3,
        voted_for: 1,
        commit_index: 0,
    };
    node.storage.save_hard_state(&initial_hs).await.unwrap();
    node.state.hard_state = initial_hs;
    node.state.server.role = Role::Candidate;
    node.state.server.leader_id = None;
    node.votes_received.insert(1);

    // Call the method under test
    node.become_follower(5, Some(2)).await.unwrap();

    // Assert final state
    assert_eq!(node.state.server.role, Role::Follower);
    assert_eq!(node.state.server.leader_id, Some(2));
    assert!(node.votes_received.is_empty());
    // Check persistence
    let stored_state = node.storage.read_hard_state().await.unwrap();
    assert_eq!(stored_state.term, 5);
    assert_eq!(stored_state.voted_for, 0); // Voted_for reset
                                           // Check in-memory state
    assert_eq!(node.state.hard_state.term, 5);
    assert_eq!(node.state.hard_state.voted_for, 0);
}

#[tokio::test]
async fn test_become_candidate() {
    let registry = Arc::new(TransportRegistry::new());
    let mut peers = HashMap::new();
    peers.insert(1, "addr1".to_string());
    peers.insert(2, "addr2".to_string()); // Need peers for vote count check later
    let mut node = create_test_node(1, peers.clone(), registry.clone()).await;

    // Set initial state (as if follower)
    let initial_hs = HardState {
        term: 3,
        voted_for: 0,
        commit_index: 0,
    };
    node.storage.save_hard_state(&initial_hs).await.unwrap();
    node.state.hard_state = initial_hs;
    node.state.server.role = Role::Follower;
    node.state.server.leader_id = Some(2);

    // Call the method under test
    node.become_candidate().await.unwrap();

    // Assert final state
    assert_eq!(node.state.server.role, Role::Candidate);
    assert_eq!(node.state.server.leader_id, None);
    assert!(node.votes_received.contains(&1)); // Self-vote recorded
    assert_eq!(node.votes_received.len(), 1);
    // Check persistence
    let stored_state = node.storage.read_hard_state().await.unwrap();
    assert_eq!(stored_state.term, 4); // Term incremented
    assert_eq!(stored_state.voted_for, 1); // Voted for self
                                           // Check in-memory state
    assert_eq!(node.state.hard_state.term, 4);
    assert_eq!(node.state.hard_state.voted_for, 1);
}

#[tokio::test]
async fn test_become_leader() {
    let registry = Arc::new(TransportRegistry::new());
    let mut peers = HashMap::new();
    peers.insert(1, "addr1".to_string());
    peers.insert(2, "addr2".to_string());
    peers.insert(3, "addr3".to_string());
    let mut node = create_test_node(1, peers.clone(), registry.clone()).await;

    // Set state as if it just won an election in term 4
    let initial_hs = HardState {
        term: 4,
        voted_for: 1,
        commit_index: 0,
    };
    node.storage.save_hard_state(&initial_hs).await.unwrap();

    // Append log entries 1 through 5 sequentially
    let entries: Vec<LogEntry> = (1..=5)
        .map(|i| LogEntry {
            term: 4, // Assume term 4 for simplicity
            index: i,
            command: Bytes::new(),
        })
        .collect();
    node.storage.append_log_entries(&entries).await.unwrap();

    node.state.hard_state = initial_hs;
    node.state.server.role = Role::Candidate;
    node.votes_received.insert(1); // Self-vote
    node.votes_received.insert(2); // Assume got vote from 2

    // Call the method under test
    node.become_leader()
        .await
        .expect("become_leader should succeed");

    // Assert final state
    assert_eq!(node.state.server.role, Role::Leader);
    assert_eq!(node.state.server.leader_id, Some(1));
    assert!(node.votes_received.is_empty()); // Should be cleared

    // Check leader state initialization (nextIndex, matchIndex)
    let last_log_idx = node.storage.last_log_index().await.unwrap(); // Get actual last log index
    assert_eq!(last_log_idx, 5);
    assert_eq!(
        node.state.leader.next_index.get(&2),
        Some(&(last_log_idx + 1))
    ); // nextIndex = last log index + 1
    assert_eq!(
        node.state.leader.next_index.get(&3),
        Some(&(last_log_idx + 1))
    );
    assert_eq!(node.state.leader.match_index.get(&2), Some(&0)); // matchIndex = 0
    assert_eq!(node.state.leader.match_index.get(&3), Some(&0));
    assert!(!node.state.leader.next_index.contains_key(&1)); // Should not track self

    // Verify no unexpected persistence changes occurred in become_leader itself
    let stored_state = node.storage.read_hard_state().await.unwrap();
    assert_eq!(stored_state.term, 4);
    assert_eq!(stored_state.voted_for, 1);
}
