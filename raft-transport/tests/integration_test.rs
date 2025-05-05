// use anyhow::Result; // Removed unused import
use raft_core::NodeId;
use raft_transport::{
    MockTransport, NetworkOptions, PeerReceivers, Transport, TransportError, TransportRegistry,
};
use std::sync::Arc;
use std::time::Duration;
// use tokio::sync::oneshot; // Removed unused import
use tokio::time::{Instant, sleep};
use wcygan_raft_community_neoeinstein_prost::raft::v1::{
    AppendEntriesRequest, AppendEntriesResponse, RequestVoteRequest, RequestVoteResponse,
};

// Helper to initialize tracing for tests
fn init_tracing() {
    // Using try_init ignores errors if already initialized
    let _ = tracing_subscriber::fmt()
        .with_env_filter("raft_transport=trace,integration_test=trace")
        .with_test_writer()
        .try_init();
}

// Helper struct to manage transport and receiver handles for a node
struct TestNode {
    #[allow(dead_code)] // This ID is useful for debugging/tracing but not read in assertions
    id: NodeId,
    transport: MockTransport,
    receivers: Option<PeerReceivers>,
}

impl TestNode {
    async fn new(id: NodeId, registry: Arc<TransportRegistry>) -> Self {
        Self::new_with_options(id, NetworkOptions::default(), registry).await
    }

    async fn new_with_options(
        id: NodeId,
        options: NetworkOptions,
        registry: Arc<TransportRegistry>,
    ) -> Self {
        let (transport, receivers) =
            MockTransport::create_with_options(id, options, registry).await;
        TestNode {
            id,
            transport,
            receivers: Some(receivers),
        }
    }

    fn take_receivers(&mut self) -> PeerReceivers {
        self.receivers.take().expect("Receivers already taken")
    }

    /// Closes the transport associated with this test node.
    fn close(&self) {
        self.transport.close();
    }

    // Simple task to handle incoming requests and send back default responses
    fn spawn_responder_task(mut receivers: PeerReceivers) {
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some((req, resp_tx)) = receivers.append_entries_rx.recv() => {
                        tracing::trace!(node_id = ?req.leader_id, target = "receiver", "Received AE req, sending default OK");
                        let _ = resp_tx.send(Ok(AppendEntriesResponse {
                            term: req.term, // Echo term
                            success: true,
                            match_index: 0, // Default
                        }));
                    }
                    Some((req, resp_tx)) = receivers.request_vote_rx.recv() => {
                        tracing::trace!(node_id = req.candidate_id, target = "receiver", "Received RV req, sending default OK");
                        let _ = resp_tx.send(Ok(RequestVoteResponse {
                            term: req.term, // Echo term
                            vote_granted: true,
                        }));
                    }
                    else => {
                        tracing::debug!(target = "receiver", "Receiver channels closed, ending task.");
                        break;
                    }
                }
            }
        });
    }
}

#[tokio::test]
async fn test_basic_send_recv() {
    init_tracing();
    let registry = Arc::new(TransportRegistry::new());

    let node1 = TestNode::new(1, registry.clone()).await;
    let mut node2 = TestNode::new(2, registry.clone()).await;

    TestNode::spawn_responder_task(node2.take_receivers());

    // Test AppendEntries
    let ae_req = AppendEntriesRequest {
        term: 1,
        leader_id: 1,
        ..Default::default()
    };
    let ae_resp = node1
        .transport
        .send_append_entries(2, ae_req.clone())
        .await
        .expect("AE send should succeed");
    assert_eq!(ae_resp.term, 1);
    assert!(ae_resp.success);

    // Test RequestVote
    let rv_req = RequestVoteRequest {
        term: 1,
        candidate_id: 1,
        ..Default::default()
    };
    let rv_resp = node1
        .transport
        .send_request_vote(2, rv_req.clone())
        .await
        .expect("RV send should succeed");
    assert_eq!(rv_resp.term, 1);
    assert!(rv_resp.vote_granted);
}

#[tokio::test]
async fn test_send_to_nonexistent_peer() {
    init_tracing();
    let registry = Arc::new(TransportRegistry::new());
    let node1 = TestNode::new(1, registry.clone()).await;

    let ae_req = AppendEntriesRequest::default();
    let result = node1.transport.send_append_entries(99, ae_req).await;

    assert!(result.is_err());
    let err = result.unwrap_err().downcast::<TransportError>().unwrap();
    assert_eq!(err, TransportError::PeerNotFound(99));
}

#[tokio::test]
async fn test_network_partition() {
    init_tracing();
    let registry = Arc::new(TransportRegistry::new());

    let mut node1 = TestNode::new(1, registry.clone()).await;
    let mut node2 = TestNode::new(2, registry.clone()).await;
    let mut node3 = TestNode::new(3, registry.clone()).await;

    TestNode::spawn_responder_task(node1.take_receivers());
    TestNode::spawn_responder_task(node2.take_receivers());
    TestNode::spawn_responder_task(node3.take_receivers());

    // Partition 1 -> 2
    node1.transport.partition_from(2).await;

    // Send 1 -> 2 (should fail)
    let ae_req = AppendEntriesRequest {
        term: 1,
        leader_id: 1,
        ..Default::default()
    };
    let res12 = node1.transport.send_append_entries(2, ae_req.clone()).await;
    assert!(res12.is_err());
    assert_eq!(
        res12.unwrap_err().downcast::<TransportError>().unwrap(),
        TransportError::Partitioned(1, 2)
    );

    // Send 1 -> 3 (should succeed)
    let res13 = node1.transport.send_append_entries(3, ae_req.clone()).await;
    assert!(res13.is_ok());
    assert_eq!(res13.unwrap().term, 1);

    // Send 2 -> 1 (should succeed, partition is one-way)
    let res21 = node2.transport.send_append_entries(1, ae_req.clone()).await;
    assert!(res21.is_ok(), "Send 2->1 failed unexpectedly"); // Node 1 doesn't have a responder, but the send itself should work via channel
    assert_eq!(res21.unwrap().term, 1); // Assuming node1 would respond eventually if it had a listener

    // Heal partition 1 -> 2
    node1.transport.heal_partition_from(2).await;

    // Send 1 -> 2 (should succeed now)
    let res12_healed = node1.transport.send_append_entries(2, ae_req.clone()).await;
    assert!(res12_healed.is_ok());
    assert_eq!(res12_healed.unwrap().term, 1);
}

#[tokio::test]
async fn test_message_delay() {
    init_tracing();
    let registry = Arc::new(TransportRegistry::new());
    let delay = Duration::from_millis(100);
    let node1 = TestNode::new_with_options(
        1,
        NetworkOptions {
            min_delay: delay,
            max_delay: delay,
            ..Default::default()
        },
        registry.clone(),
    )
    .await;
    let mut node2 = TestNode::new(2, registry.clone()).await;

    TestNode::spawn_responder_task(node2.take_receivers());

    let ae_req = AppendEntriesRequest::default();

    let start = Instant::now();
    let _ = node1
        .transport
        .send_append_entries(2, ae_req)
        .await
        .unwrap();
    let elapsed = start.elapsed();

    tracing::info!(?elapsed, ?delay, "Measured delay");
    assert!(
        elapsed >= delay,
        "Elapsed time should be at least the configured delay"
    );
    assert!(elapsed < delay * 5, "Elapsed time excessively long");
}

#[tokio::test]
async fn test_message_loss() {
    init_tracing();
    let registry = Arc::new(TransportRegistry::new());
    let lossy_options = NetworkOptions {
        message_loss_probability: 1.0, // 100% loss
        ..Default::default()
    };
    let node1 = TestNode::new_with_options(1, lossy_options.clone(), registry.clone()).await;
    let mut node2 = TestNode::new(2, registry.clone()).await;

    TestNode::spawn_responder_task(node2.take_receivers());

    // Define ae_req *after* nodes are created and responder is running
    let ae_req = AppendEntriesRequest::default();

    // Test with 100% loss
    for _ in 0..5 {
        let result = node1.transport.send_append_entries(2, ae_req.clone()).await;
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().downcast::<TransportError>().unwrap(),
            TransportError::MessageDropped(2)
        );
    }

    // Update options to 0% loss
    node1
        .transport
        .update_network_options(NetworkOptions::default())
        .await;
    tokio::time::sleep(Duration::from_millis(10)).await; // Ensure options update propagates if needed

    // Test with 0% loss
    let result = node1.transport.send_append_entries(2, ae_req.clone()).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_concurrent_sends() {
    init_tracing();
    let registry = Arc::new(TransportRegistry::new());

    let node1 = TestNode::new(1, registry.clone()).await;
    let mut node2 = TestNode::new(2, registry.clone()).await;

    TestNode::spawn_responder_task(node2.take_receivers());

    let num_tasks = 20;
    let mut join_handles = Vec::new();

    let transport1 = node1.transport.clone(); // Clone transport for tasks

    for i in 0..num_tasks {
        let transport = transport1.clone();
        let handle = tokio::spawn(async move {
            let req = if i % 2 == 0 {
                // Send AE
                let ae_req = AppendEntriesRequest {
                    term: i as u64,
                    leader_id: 1,
                    ..Default::default()
                };
                transport
                    .send_append_entries(2, ae_req)
                    .await
                    .map(|resp| resp.term)
            } else {
                // Send RV
                let rv_req = RequestVoteRequest {
                    term: i as u64,
                    candidate_id: 1,
                    ..Default::default()
                };
                transport
                    .send_request_vote(2, rv_req)
                    .await
                    .map(|resp| resp.term)
            };
            (i, req)
        });
        join_handles.push(handle);
    }

    let results = futures::future::join_all(join_handles).await;

    let mut success_count = 0;
    for (i, result) in results.into_iter().enumerate() {
        match result {
            Ok((task_idx, Ok(term))) => {
                assert_eq!(task_idx as u64, term, "Term mismatch for task {}", i);
                success_count += 1;
            }
            Ok((_, Err(e))) => panic!("Task {} failed: {:?}", i, e),
            Err(e) => panic!("Task {} panicked: {:?}", i, e),
        }
    }

    assert_eq!(
        success_count, num_tasks,
        "Not all concurrent tasks succeeded"
    );
}

#[tokio::test]
async fn test_temporary_partition_and_heal() {
    init_tracing();
    let registry = Arc::new(TransportRegistry::new());
    let mut node1 = TestNode::new(1, registry.clone()).await;
    let mut node2 = TestNode::new(2, registry.clone()).await;
    TestNode::spawn_responder_task(node2.take_receivers());

    let req = AppendEntriesRequest {
        leader_id: 1,
        ..Default::default()
    };

    // 1. Partition 1 -> 2
    node1.transport.partition_from(2).await;
    let res_partitioned = node1.transport.send_append_entries(2, req.clone()).await;
    assert!(
        matches!(res_partitioned, Err(e) if e.downcast_ref::<TransportError>() == Some(&TransportError::Partitioned(1, 2)))
    );
    tracing::info!("Confirmed message blocked by partition");

    // 2. Heal partition 1 -> 2
    node1.transport.heal_partition_from(2).await;
    tracing::info!("Partition healed");
    sleep(Duration::from_millis(10)).await; // Give time for state changes if any

    // 3. Send again (should succeed)
    let res_healed = node1.transport.send_append_entries(2, req.clone()).await;
    assert!(
        res_healed.is_ok(),
        "Send failed after partition heal: {:?}",
        res_healed.err()
    );
    assert!(res_healed.unwrap().success);
    tracing::info!("Confirmed message successful after heal");
}

#[tokio::test]
async fn test_variable_delays_slow_node() {
    init_tracing();
    let registry = Arc::new(TransportRegistry::new());
    let slow_delay = Duration::from_millis(150);
    let normal_delay = Duration::from_millis(10);

    // Node 1 sends slowly
    let node1 = TestNode::new_with_options(
        1,
        NetworkOptions {
            min_delay: slow_delay,
            max_delay: slow_delay,
            ..Default::default()
        },
        registry.clone(),
    )
    .await;
    // Node 2 sends normally
    let node2 = TestNode::new_with_options(
        2,
        NetworkOptions {
            min_delay: normal_delay,
            max_delay: normal_delay,
            ..Default::default()
        },
        registry.clone(),
    )
    .await;
    // Node 3 receives
    let mut node3 = TestNode::new(3, registry.clone()).await;
    TestNode::spawn_responder_task(node3.take_receivers());

    let req = AppendEntriesRequest {
        leader_id: 1,
        ..Default::default()
    };

    let start1 = Instant::now();
    let res1 = node1.transport.send_append_entries(3, req.clone()).await;
    let elapsed1 = start1.elapsed();

    let start2 = Instant::now();
    let res2 = node2.transport.send_append_entries(3, req.clone()).await;
    let elapsed2 = start2.elapsed();

    assert!(res1.is_ok());
    assert!(res2.is_ok());

    tracing::info!(?elapsed1, ?slow_delay, "Node 1 (slow) send time");
    tracing::info!(?elapsed2, ?normal_delay, "Node 2 (normal) send time");

    // Check that slow node took significantly longer
    assert!(elapsed1 >= slow_delay);
    assert!(elapsed1 < slow_delay * 5); // Upper bound
    assert!(elapsed2 >= normal_delay);
    assert!(elapsed2 < normal_delay * 10); // Wider bound for faster delay

    // Crucially, check that node 1 was slower than node 2
    // Use a threshold slightly less than the difference in delays to account for jitter
    let expected_diff = slow_delay - normal_delay;
    assert!(elapsed1 > elapsed2 + expected_diff / 2);
}

#[tokio::test]
async fn test_asymmetric_partition() {
    init_tracing();
    let registry = Arc::new(TransportRegistry::new());
    let mut node1 = TestNode::new(1, registry.clone()).await;
    let mut node2 = TestNode::new(2, registry.clone()).await;
    TestNode::spawn_responder_task(node1.take_receivers()); // Node 1 needs to respond to Node 2
    TestNode::spawn_responder_task(node2.take_receivers()); // Node 2 needs to respond to Node 1 (when allowed)

    let req = AppendEntriesRequest {
        ..Default::default()
    };

    // Partition 1 -> 2 (but not 2 -> 1)
    node1.transport.partition_from(2).await;
    tracing::info!("Asymmetric partition 1->2 created");

    // Send 1 -> 2 (should fail)
    let res12 = node1.transport.send_append_entries(2, req.clone()).await;
    assert!(
        matches!(res12, Err(e) if e.downcast_ref::<TransportError>() == Some(&TransportError::Partitioned(1, 2))),
        "Send 1->2 should be partitioned"
    );
    tracing::info!("Confirmed 1->2 send failed");

    // Send 2 -> 1 (should succeed)
    let res21 = node2.transport.send_append_entries(1, req.clone()).await;
    assert!(res21.is_ok(), "Send 2->1 should succeed: {:?}", res21.err());
    assert!(res21.unwrap().success);
    tracing::info!("Confirmed 2->1 send succeeded");
}

#[tokio::test]
async fn test_large_cluster_communication_5_nodes() {
    init_tracing();
    let registry = Arc::new(TransportRegistry::new());
    let node_ids: Vec<NodeId> = (1..=5).collect();
    let expected_successes = node_ids.len() * (node_ids.len() - 1);

    // Run the main test logic within a spawned task to control lifetime
    let test_task = tokio::spawn(async move {
        let mut nodes: Vec<TestNode> = Vec::new(); // Store TestNode instances

        // Create nodes and spawn responders
        for id in &node_ids {
            let mut node = TestNode::new(*id, registry.clone()).await;
            TestNode::spawn_responder_task(node.take_receivers());
            nodes.push(node); // Push the whole TestNode
        }

        let req = AppendEntriesRequest {
            term: 1,
            ..Default::default()
        };
        let mut handles = Vec::new();

        // Each node sends to all other nodes
        for i in 0..nodes.len() {
            let sender_transport = nodes[i].transport.clone();
            let sender_id = node_ids[i];
            for j in 0..nodes.len() {
                if i == j {
                    continue;
                }
                let receiver_id = node_ids[j];
                let transport = sender_transport.clone();
                let request = AppendEntriesRequest {
                    leader_id: sender_id,
                    ..req.clone()
                };
                handles.push(tokio::spawn(async move {
                    transport.send_append_entries(receiver_id, request).await
                }));
            }
        }

        let results = futures::future::join_all(handles).await;
        let mut success_count = 0;

        for result in results {
            match result {
                Ok(Ok(resp)) if resp.success => success_count += 1,
                Ok(Ok(resp)) => {
                    tracing::error!(?resp, "Request succeeded but response indicated failure")
                }
                Ok(Err(e)) => tracing::error!(error = ?e, "Send failed unexpectedly"),
                Err(e) => tracing::error!(error = ?e, "Task panicked"),
            }
        }

        assert_eq!(
            success_count, expected_successes,
            "Not all messages in 5-node cluster succeeded"
        );
        tracing::info!(
            sent = expected_successes,
            received = success_count,
            "Completed 5-node communication test"
        );

        // Explicitly close transports after awaiting results
        for node in nodes {
            node.close();
        }

        success_count // Return success count for final assertion
    });

    // Wait for the test task to complete
    let final_success_count = test_task.await.expect("Test task panicked");
    assert_eq!(
        final_success_count, expected_successes,
        "Final assertion failed: Not all messages succeeded"
    );
}

#[tokio::test]
async fn test_receiver_dropped_before_response() {
    init_tracing();
    let registry = Arc::new(TransportRegistry::new());

    let node1 = TestNode::new(1, registry.clone()).await;
    let mut node2 = TestNode::new(2, registry.clone()).await;

    // Take receivers but DON'T spawn a responder task
    let node2_receivers = node2.take_receivers();

    let req = AppendEntriesRequest {
        leader_id: 1,
        ..Default::default()
    };

    // Spawn a task to drop the receivers after a short delay, while node 1 is sending/waiting
    tokio::spawn(async move {
        sleep(Duration::from_millis(50)).await;
        drop(node2_receivers);
        tracing::info!("Node 2 receivers dropped");
    });

    // Send from node 1 - this should eventually error out
    let start = Instant::now();
    let send_timeout = Duration::from_millis(200); // Set a reasonable timeout

    let result =
        tokio::time::timeout(send_timeout, node1.transport.send_append_entries(2, req)).await;
    let elapsed = start.elapsed();
    tracing::info!(?elapsed, "Send attempt finished");

    match result {
        Ok(Ok(resp)) => {
            // Unexpected success!
            panic!(
                "Send unexpectedly succeeded after receiver drop: {:?}",
                resp
            );
        }
        Ok(Err(inner_err)) => {
            // Send failed before timeout, as expected.
            tracing::info!(error = ?inner_err, "Send failed before timeout as expected");
            assert!(
                matches!(
                    inner_err.downcast_ref::<TransportError>(),
                    Some(TransportError::RecvError(2)) | Some(TransportError::SendError(..))
                ),
                "Expected RecvError(2) or SendError after receiver drop, but got: {:?}",
                inner_err
            );
        }
        Err(_elapsed_error) => {
            // Timeout elapsed.
            panic!(
                "Send blocked indefinitely until timeout, expected quicker error after receiver drop."
            );
        }
    }
}
