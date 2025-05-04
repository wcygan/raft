use crate::error::TransportError;
use crate::network::NetworkOptions;
use anyhow::Result;
use raft_core::NodeId;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use tokio::sync::{mpsc, oneshot};
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

/// A shared registry for MockTransport instances within a test context.
#[derive(Debug, Clone, Default)]
pub struct TransportRegistry {
    // Use std::sync::RwLock for synchronous access in Drop
    endpoints: Arc<RwLock<HashMap<NodeId, PeerSenders>>>,
}

impl TransportRegistry {
    /// Creates a new, empty transport registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers the senders for a given node ID.
    /// Panics if the node ID is already registered (indicates test setup error).
    fn register(&self, id: NodeId, senders: PeerSenders) {
        let mut guard = self.endpoints.write().expect("Registry lock poisoned");
        if guard.insert(id, senders).is_some() {
            panic!("TransportRegistry: Node {} already registered!", id);
        }
        tracing::trace!(node_id = id, "Registered transport endpoint");
    }

    /// Unregisters the senders for a given node ID.
    fn unregister(&self, id: NodeId) {
        let mut guard = self.endpoints.write().expect("Registry lock poisoned");
        if guard.remove(&id).is_some() {
            tracing::trace!(node_id = id, "Unregistered transport endpoint");
        }
    }

    /// Looks up the senders for a given node ID.
    fn lookup(&self, id: NodeId) -> Option<PeerSenders> {
        self.endpoints
            .read()
            .expect("Registry lock poisoned")
            .get(&id)
            .cloned() // Clone the Senders (cheap Arc clones)
    }
}

/// A mock transport layer using Tokio MPSC channels for in-memory communication.
///
/// This transport simulates network conditions like delay, loss, and partitions.
/// It requires a shared `TransportRegistry` to discover peers.
#[derive(Debug, Clone)]
pub struct MockTransport {
    node_id: NodeId,
    network_options: Arc<RwLock<NetworkOptions>>,
    registry: Arc<TransportRegistry>,
}

impl Drop for MockTransport {
    fn drop(&mut self) {
        self.registry.unregister(self.node_id);
        // Note: Tracing in drop might be unreliable depending on shutdown order
        // tracing::debug!(node_id = self.node_id, "MockTransport dropped and unregistered");
    }
}

impl MockTransport {
    /// Creates a new MockTransport with specific network simulation options
    /// and registers it with the provided registry.
    pub async fn create_with_options(
        node_id: NodeId,
        options: NetworkOptions,
        registry: Arc<TransportRegistry>,
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

        registry.register(node_id, senders);

        let transport = MockTransport {
            node_id,
            network_options: Arc::new(RwLock::new(options)),
            registry,
        };

        tracing::debug!(node_id, "MockTransport created and registered");
        (transport, receivers)
    }

    /// Updates the network simulation options for this transport.
    pub async fn update_network_options(&self, options: NetworkOptions) {
        let mut current_options = self.network_options.write().expect("Options lock poisoned");
        *current_options = options;
        tracing::debug!(
            node_id = self.node_id,
            ?current_options,
            "Updated network options"
        );
    }

    /// Partitions this node from the specified peer (one-way).
    pub async fn partition_from(&self, peer_id: NodeId) {
        let mut options = self.network_options.write().expect("Options lock poisoned");
        options.partitioned_links.insert((self.node_id, peer_id));
        tracing::info!(
            from = self.node_id,
            to = peer_id,
            "Network partition created"
        );
    }

    /// Removes a previously created partition from this node to the specified peer.
    pub async fn heal_partition_from(&self, peer_id: NodeId) {
        let mut options = self.network_options.write().expect("Options lock poisoned");
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
    ) -> Result<Resp>
    where
        Req: std::fmt::Debug,
    {
        let options = self
            .network_options
            .read()
            .expect("Options lock poisoned")
            .clone();

        if options.is_partitioned(self.node_id, peer_id) {
            tracing::warn!(
                from = self.node_id,
                to = peer_id,
                ?request,
                "Message blocked by partition"
            );
            return Err(TransportError::Partitioned(self.node_id, peer_id).into());
        }

        options.simulate_delay().await;

        if options.should_drop_message() {
            tracing::warn!(
                from = self.node_id,
                to = peer_id,
                ?request,
                "Message dropped due to simulated loss"
            );
            return Err(TransportError::MessageDropped(peer_id).into());
        }

        let (resp_tx, resp_rx) = oneshot::channel();

        if let Err(e) = sender_channel.send((request, resp_tx)).await {
            tracing::error!(from = self.node_id, to = peer_id, error = ?e, "Failed to send request via channel");
            return Err(TransportError::SendError(peer_id, e.to_string()).into());
        }

        match resp_rx.await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(e)) => {
                tracing::error!(from = self.node_id, to = peer_id, error = ?e, "Received error response from peer");
                Err(e)
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

    pub(crate) async fn send_append_entries_impl(
        &self,
        peer_id: NodeId,
        request: AppendEntriesRequest,
    ) -> Result<AppendEntriesResponse> {
        tracing::trace!(
            from = self.node_id,
            to = peer_id,
            req_term = request.term,
            req_prev_log_index = request.prev_log_index,
            req_entries_len = request.entries.len(),
            "Sending AppendEntries"
        );
        let peer_senders = self
            .registry
            .lookup(peer_id)
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
            req_term = request.term,
            req_last_log_index = request.last_log_index,
            "Sending RequestVote"
        );
        let peer_senders = self
            .registry
            .lookup(peer_id)
            .ok_or(TransportError::PeerNotFound(peer_id))?;

        self.send_request(peer_id, request, &peer_senders.request_vote_tx)
            .await
    }
}
