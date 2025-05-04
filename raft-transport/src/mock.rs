use crate::error::TransportError;
use crate::network::NetworkOptions;
use anyhow::Result;
use once_cell::sync::Lazy;
use raft_core::NodeId;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc, oneshot};
use wcygan_raft_community_neoeinstein_prost::raft::v1::{
    AppendEntriesRequest, AppendEntriesResponse, RequestVoteRequest, RequestVoteResponse,
};

/// Type alias for the sender part of the AppendEntries channel.
type AppendEntriesSender = mpsc::Sender<(
    AppendEntriesRequest,
    oneshot::Sender<Result<AppendEntriesResponse>>,
)>;
/// Type alias for the receiver part of the AppendEntries channel.
pub type AppendEntriesReceiver = mpsc::Receiver<(
    AppendEntriesRequest,
    oneshot::Sender<Result<AppendEntriesResponse>>,
)>;

/// Type alias for the sender part of the RequestVote channel.
type RequestVoteSender = mpsc::Sender<(
    RequestVoteRequest,
    oneshot::Sender<Result<RequestVoteResponse>>,
)>;
/// Type alias for the receiver part of the RequestVote channel.
pub type RequestVoteReceiver = mpsc::Receiver<(
    RequestVoteRequest,
    oneshot::Sender<Result<RequestVoteResponse>>,
)>;

/// Holds the sender channels for communicating with a specific peer.
#[derive(Debug, Clone)]
struct PeerSenders {
    append_entries_tx: AppendEntriesSender,
    request_vote_tx: RequestVoteSender,
    // Add InstallSnapshot sender if needed
}

/// Holds the receiver channels for a specific node.
#[derive(Debug)]
pub struct PeerReceivers {
    pub append_entries_rx: AppendEntriesReceiver,
    pub request_vote_rx: RequestVoteReceiver,
    // Add InstallSnapshot receiver if needed
}

/// Global registry mapping NodeId to their transport senders.
static REGISTRY: Lazy<RwLock<HashMap<NodeId, PeerSenders>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// A mock transport layer using Tokio MPSC channels for in-memory communication.
///
/// This transport simulates network conditions like delay, loss, and partitions.
#[derive(Debug, Clone)]
pub struct MockTransport {
    node_id: NodeId,
    network_options: Arc<RwLock<NetworkOptions>>,
}

impl MockTransport {
    /// Creates a new MockTransport for the given node ID and registers it.
    ///
    /// Returns the transport instance and the receiving ends of the channels.
    /// Panics if a transport for this node_id already exists.
    pub async fn create(node_id: NodeId) -> (Self, PeerReceivers) {
        Self::create_with_options(node_id, NetworkOptions::default()).await
    }

    /// Creates a new MockTransport with specific network simulation options.
    pub async fn create_with_options(
        node_id: NodeId,
        options: NetworkOptions,
    ) -> (Self, PeerReceivers) {
        let (ae_tx, ae_rx) = mpsc::channel(100); // Buffer size 100
        let (rv_tx, rv_rx) = mpsc::channel(100);

        let senders = PeerSenders {
            append_entries_tx: ae_tx,
            request_vote_tx: rv_tx,
        };

        let receivers = PeerReceivers {
            append_entries_rx: ae_rx,
            request_vote_rx: rv_rx,
        };

        let transport = MockTransport {
            node_id,
            network_options: Arc::new(RwLock::new(options)),
        };

        let mut registry = REGISTRY.write().await;
        if registry.insert(node_id, senders).is_some() {
            // Use panic because this indicates a setup error in tests
            panic!("Transport for node {} already exists!", node_id);
        }

        tracing::debug!(node_id, "MockTransport created and registered");
        (transport, receivers)
    }

    /// Removes the transport for the given node ID from the registry.
    /// Should be called during test teardown.
    pub async fn destroy(node_id: NodeId) {
        let mut registry = REGISTRY.write().await;
        if registry.remove(&node_id).is_some() {
            tracing::debug!(node_id, "MockTransport destroyed and unregistered");
        } else {
            tracing::warn!(node_id, "Attempted to destroy non-existent MockTransport");
        }
    }

    /// Updates the network simulation options for this transport.
    pub async fn update_network_options(&self, options: NetworkOptions) {
        let mut current_options = self.network_options.write().await;
        *current_options = options;
        tracing::debug!(
            node_id = self.node_id,
            ?current_options,
            "Updated network options"
        );
    }

    /// Partitions this node from the specified peer (one-way).
    pub async fn partition_from(&self, peer_id: NodeId) {
        let mut options = self.network_options.write().await;
        options.partitioned_links.insert((self.node_id, peer_id));
        tracing::info!(
            from = self.node_id,
            to = peer_id,
            "Network partition created"
        );
    }

    /// Removes a previously created partition from this node to the specified peer.
    pub async fn heal_partition_from(&self, peer_id: NodeId) {
        let mut options = self.network_options.write().await;
        if options.partitioned_links.remove(&(self.node_id, peer_id)) {
            tracing::info!(
                from = self.node_id,
                to = peer_id,
                "Network partition healed"
            );
        }
    }

    /// Helper function to send a request and wait for the response.
    async fn send_request<Req, Resp>(
        &self,
        peer_id: NodeId,
        request: Req,
        sender_channel: &mpsc::Sender<(Req, oneshot::Sender<Result<Resp>>)>,
    ) -> Result<Resp> {
        let options = self.network_options.read().await;

        // 1. Check for partition
        if options.is_partitioned(self.node_id, peer_id) {
            tracing::warn!(
                from = self.node_id,
                to = peer_id,
                "Message blocked by partition"
            );
            return Err(TransportError::Partitioned(self.node_id, peer_id).into());
        }

        // 2. Simulate delay
        options.simulate_delay().await;

        // 3. Check for message loss
        if options.should_drop_message() {
            tracing::warn!(
                from = self.node_id,
                to = peer_id,
                "Message dropped due to simulated loss"
            );
            return Err(TransportError::MessageDropped(peer_id).into());
        }

        // 4. Prepare response channel
        let (resp_tx, resp_rx) = oneshot::channel();

        // 5. Send the request
        if let Err(e) = sender_channel.send((request, resp_tx)).await {
            tracing::error!(from = self.node_id, to = peer_id, error = ?e, "Failed to send request via channel");
            return Err(TransportError::SendError(peer_id, e.to_string()).into());
        }

        // 6. Wait for the response
        match resp_rx.await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(e)) => {
                tracing::error!(from = self.node_id, to = peer_id, error = ?e, "Received error response from peer");
                Err(e) // Propagate the error returned by the peer
            }
            Err(_) => {
                tracing::error!(
                    from = self.node_id,
                    to = peer_id,
                    "Response channel closed prematurely"
                );
                Err(TransportError::RecvError(peer_id).into())
            }
        }
    }

    // These methods will be called by the `Transport` trait implementation in lib.rs
    pub(crate) async fn send_append_entries_impl(
        &self,
        peer_id: NodeId,
        request: AppendEntriesRequest,
    ) -> Result<AppendEntriesResponse> {
        tracing::trace!(
            from = self.node_id,
            to = peer_id,
            ?request,
            "Sending AppendEntries"
        );
        let registry = REGISTRY.read().await;
        let peer_senders = registry
            .get(&peer_id)
            .ok_or(TransportError::PeerNotFound(peer_id))?;

        self.send_request(peer_id, request, &peer_senders.append_entries_tx)
            .await
    }

    pub(crate) async fn send_request_vote_impl(
        &self,
        peer_id: NodeId,
        request: RequestVoteRequest,
    ) -> Result<RequestVoteResponse> {
        tracing::trace!(
            from = self.node_id,
            to = peer_id,
            ?request,
            "Sending RequestVote"
        );
        let registry = REGISTRY.read().await;
        let peer_senders = registry
            .get(&peer_id)
            .ok_or(TransportError::PeerNotFound(peer_id))?;

        self.send_request(peer_id, request, &peer_senders.request_vote_tx)
            .await
    }
}
