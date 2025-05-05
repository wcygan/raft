use raft_core::{Config, NodeId, RaftNode, Role};
use raft_storage::InMemoryStorage;
use raft_transport::{MockTransport, NetworkOptions, PeerReceivers, TransportRegistry};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{Duration, sleep, timeout};
use tracing::Level;

// Helper to initialize tracing for tests
fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(Level::INFO) // Default level
        .with_test_writer()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
}

// Helper to create a cluster of nodes for testing
async fn setup_cluster(
    node_ids: Vec<NodeId>,
) -> (
    HashMap<NodeId, Arc<Mutex<RaftNode<InMemoryStorage, MockTransport>>>>,
    HashMap<NodeId, PeerReceivers>,
    HashMap<NodeId, MockTransport>,
    Arc<TransportRegistry>,
) {
    init_tracing();
    let registry: Arc<TransportRegistry> = TransportRegistry::new().into();
    let mut nodes = HashMap::new();
    let mut receivers_map = HashMap::new();
    let mut transports_map = HashMap::new();

    let peer_ids: Vec<NodeId> = node_ids.clone();

    for node_id in node_ids.clone() {
        let config = Config {
            id: node_id,
            peers: peer_ids.clone(),
            heartbeat_timeout: 150,
            election_timeout: 300,
        };
        let storage = InMemoryStorage::new();
        let (transport, receivers) = MockTransport::create_with_options(
            node_id,
            NetworkOptions::default(),
            registry.clone(),
        )
        .await;
        let node = RaftNode::new(node_id, config, storage, transport.clone());
        nodes.insert(node_id, Arc::new(Mutex::new(node)));
        receivers_map.insert(node_id, receivers);
        transports_map.insert(node_id, transport);
    }

    (nodes, receivers_map, transports_map, registry)
}

/// Spawns a task that listens for RPC requests for a given node
/// and calls the node's actual handler methods.
fn spawn_raft_responder_task(
    node_ref: Arc<Mutex<RaftNode<InMemoryStorage, MockTransport>>>,
    mut receivers: PeerReceivers,
) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                // Biased select ensures we check shutdown first if available
                // biased;
                // TODO: Add a shutdown signal if needed for cleaner termination

                maybe_req = receivers.append_entries_rx.recv() => {
                    if let Some((req, resp_tx)) = maybe_req {
                        let node_id;
                        let result;
                        {
                            // Lock acquisition scope
                            let mut node_lock = node_ref.lock().await;
                            node_id = node_lock.id;
                            tracing::debug!(target: "responder", %node_id, term = req.term, leader_id = req.leader_id, "Received AppendEntries RPC. Current state: term={}, role={:?}", node_lock.state.hard_state.term, node_lock.state.server.role);
                            result = node_lock.handle_append_entries(req).await;
                        } // Lock released here

                        match &result {
                            Ok(resp) => tracing::trace!(target: "responder", %node_id, ?resp, "Sending AppendEntries response (Ok)"),
                            Err(e) => tracing::warn!(target: "responder", %node_id, error = %e, "Sending AppendEntries response (Err)"),
                        }
                        let _ = resp_tx.send(result).map_err(|_e| {
                             tracing::warn!(target: "responder", %node_id, "Failed to send AppendEntries response channel, receiver dropped?");
                        });
                    } else {
                        // Channel closed, exit task
                        let node_id = node_ref.lock().await.id;
                        tracing::debug!(target: "responder", node_id, "AppendEntries channel closed, stopping responder.");
                        break;
                    }
                }

                maybe_req = receivers.request_vote_rx.recv() => {
                    if let Some((req, resp_tx)) = maybe_req {
                         let node_id;
                         let result;
                         {
                            // Lock acquisition scope
                            let mut node_lock = node_ref.lock().await;
                            node_id = node_lock.id;
                            tracing::debug!(target: "responder", %node_id, term = req.term, candidate_id = req.candidate_id, "Received RequestVote RPC. Current state: term={}, role={:?}, voted_for={}", node_lock.state.hard_state.term, node_lock.state.server.role, node_lock.state.hard_state.voted_for);
                            result = node_lock.handle_request_vote(req).await;
                         } // Lock released here

                         match &result {
                            Ok(resp) => tracing::trace!(target: "responder", %node_id, ?resp, "Sending RequestVote response (Ok)"),
                            Err(e) => tracing::warn!(target: "responder", %node_id, error = %e, "Sending RequestVote response (Err)"),
                         }
                         let _ = resp_tx.send(result).map_err(|_e| {
                             tracing::warn!(target: "responder", %node_id, "Failed to send RequestVote response channel, receiver dropped?");
                        });
                    } else {
                        // Channel closed, exit task
                        let node_id = node_ref.lock().await.id;
                        tracing::debug!(target: "responder", node_id, "RequestVote channel closed, stopping responder.");
                        break;
                    }
                }
            }
        }
    });
}

#[tokio::test]
async fn test_initial_state() {
    let node_ids = vec![1, 2, 3];
    let (nodes, _receivers, _transports, _registry) = setup_cluster(node_ids).await;

    for node_id in nodes.keys() {
        let node_ref = nodes.get(node_id).unwrap();
        let node_lock = node_ref.lock().await;
        assert_eq!(node_lock.state.server.role, Role::Follower);
        assert_eq!(node_lock.state.hard_state.term, 0);
    }
}

#[tokio::test]
async fn test_single_node_becomes_leader() {
    let node_ids = vec![1];
    let (nodes, _receivers, transports, _registry) = setup_cluster(node_ids).await;
    let node_ref = nodes.get(&1).unwrap();

    // Initial state check (already tested, but good for context)
    {
        let node = node_ref.lock().await;
        assert_eq!(node.state.server.role, Role::Follower);
        assert_eq!(node.state.hard_state.term, 0);
    }

    // Simulate election timeout
    // In a real scenario, a timer would trigger this. Here we call it directly.
    node_ref
        .lock()
        .await
        .handle_election_timeout()
        .await
        .unwrap();

    // Check final state
    {
        let node = node_ref.lock().await;
        assert_eq!(
            node.state.server.role,
            Role::Leader,
            "Node should become leader"
        );
        assert_eq!(
            node.state.hard_state.term, 1,
            "Term should be incremented to 1"
        );
        assert_eq!(
            node.state.hard_state.voted_for, 1,
            "Should have voted for self"
        );
        assert_eq!(
            node.state.server.leader_id,
            Some(1),
            "Leader ID should be self"
        );
    }

    // Cleanup
    transports.get(&1).unwrap().close();
}

#[tokio::test]
async fn test_three_node_election() {
    let node_ids = vec![1, 2, 3];
    let cluster_node_ids = node_ids.clone();
    let (nodes, mut receivers_map, transports, _registry) = setup_cluster(node_ids).await;

    // Spawn responder tasks for nodes 2 and 3
    for node_id in 2..=3 {
        let node_ref = nodes.get(&node_id).unwrap().clone();
        let receivers = receivers_map.remove(&node_id).unwrap();
        spawn_raft_responder_task(node_ref, receivers);
        tracing::info!(target: "test", node_id, "Spawned responder task");
    }

    let node1_ref = nodes.get(&1).unwrap().clone();

    // Trigger election timeout on node 1
    tracing::info!(target: "test", node_id = 1, "Triggering election timeout");
    let election_task =
        tokio::spawn(async move { node1_ref.lock().await.handle_election_timeout().await });

    // Wait for leader election to complete (with a timeout)
    let election_timeout_duration = Duration::from_secs(5);
    let poll_interval = Duration::from_millis(50);
    let mut leader_id: Option<NodeId> = None;
    let mut current_term: Option<u64> = None;

    tracing::info!(target: "test", "Polling for election result...");
    let result = timeout(election_timeout_duration, async {
        loop {
            let mut potential_leader = None;
            let mut follower_count = 0;
            let mut highest_term = 0;
            let mut all_in_term = true;
            let mut node_states = HashMap::new(); // Log states

            // Use the cloned cluster_node_ids here
            for node_id in &cluster_node_ids {
                let node_lock = nodes.get(node_id).unwrap().lock().await;
                let state_info = (node_lock.state.server.role, node_lock.state.hard_state.term, node_lock.state.server.leader_id);
                node_states.insert(*node_id, state_info);
                
                highest_term = highest_term.max(node_lock.state.hard_state.term);
                if node_lock.state.server.role == Role::Leader {
                    if potential_leader.is_some() {
                        tracing::warn!(target: "poll", "Multiple leaders detected!");
                        potential_leader = None;
                        break;
                    }
                    potential_leader = Some(*node_id);
                } else if node_lock.state.server.role == Role::Follower {
                    follower_count += 1;
                }
            }

            if let Some(leader_node_id) = potential_leader {
                let leader_term = nodes.get(&leader_node_id).unwrap().lock().await.state.hard_state.term;
                current_term = Some(leader_term);
                leader_id = Some(leader_node_id);

                // Check if all nodes are in the leader's term
                for node_id in &cluster_node_ids {
                    let node_lock = nodes.get(node_id).unwrap().lock().await;
                    if node_lock.state.hard_state.term != leader_term {
                        all_in_term = false;
                    }
                    // We already capture leader_id in state_info for logging
                }
            } else {
                current_term = Some(highest_term);
                all_in_term = false; 
            }

            // Log detailed state before checking condition
            tracing::trace!(target: "poll", ?node_states, ?potential_leader, follower_count, highest_term, all_in_term, "Polling cluster state...");
            
            // Use the cloned cluster_node_ids here for count
            if potential_leader.is_some() && follower_count == cluster_node_ids.len() - 1 && all_in_term {
                tracing::info!(target: "poll", term = current_term.unwrap_or(0), leader = leader_id.unwrap(), "Election completed (Leader + Followers in term).");
                break; // Success!
            }

            // No need for separate trace log here, it's combined above
            sleep(poll_interval).await;
        }
    }).await;

    election_task
        .await
        .expect("Election task panicked")
        .expect("Election task failed");

    assert!(
        result.is_ok(),
        "Election timed out after {:?}",
        election_timeout_duration
    );
    assert_eq!(leader_id, Some(1), "Node 1 should be the leader");
    assert_eq!(current_term, Some(1), "Term should be 1");

    // Final state verification for all nodes
    for node_id in &cluster_node_ids {
        let node_lock = nodes.get(node_id).unwrap().lock().await;
        assert_eq!(node_lock.state.hard_state.term, 1, "Node {} Term", node_id);
        if *node_id == 1 { // The expected leader
            assert_eq!(node_lock.state.server.role, Role::Leader, "Node 1 Role");
            assert_eq!(node_lock.state.server.leader_id, Some(1), "Node 1 Leader ID");
        } else { // Followers
            assert_eq!(node_lock.state.server.role, Role::Follower, "Node {} Role", node_id);
            // Followers won't know the leader until the first AppendEntries, so we don't assert leader_id here.
            // assert_eq!(node_lock.state.server.leader_id, Some(1), "Node {} Leader ID", node_id); 
        }
    }

    for node_id in &cluster_node_ids {
        if let Some(transport) = transports.get(node_id) {
            transport.close();
            tracing::debug!(target: "test", node_id, "Closed transport");
        }
    }
    tracing::info!(target: "test", "Test finished.");
}
