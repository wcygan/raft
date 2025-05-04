use raft_core::{Config, NodeId, RaftNode, Role};
use raft_storage::InMemoryStorage;
use raft_transport::{MockTransport, NetworkOptions, TransportRegistry};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
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
    Arc<TransportRegistry>,
) {
    init_tracing();
    let registry: Arc<TransportRegistry> = TransportRegistry::new().into();
    let mut nodes = HashMap::new();

    let peer_ids: Vec<NodeId> = node_ids.clone(); // All nodes know about each other

    for node_id in node_ids {
        let config = Config {
            id: node_id,
            peers: peer_ids.clone(),
            heartbeat_timeout: 150, // Example values
            election_timeout: 300,
        };
        let storage = InMemoryStorage::new();
        let (transport, _receivers) = MockTransport::create_with_options(
            node_id,
            NetworkOptions::default(),
            registry.clone(),
        )
        .await;
        let node = RaftNode::new(node_id, config, storage, transport);
        nodes.insert(node_id, Arc::new(Mutex::new(node)));
    }

    (nodes, registry)
}

#[tokio::test]
async fn test_initial_state() {
    let node_ids = vec![1, 2, 3];
    let (nodes, _registry) = setup_cluster(node_ids).await;

    for node_id in nodes.keys() {
        let node_ref = nodes.get(node_id).unwrap();
        let node_lock = node_ref.lock().await;
        assert_eq!(node_lock.state.server.role, Role::Follower);
        assert_eq!(node_lock.state.hard_state.term, 0);
    }
}

// TODO: Add test_single_node_becomes_leader
// TODO: Add test_three_node_election
