//! Mock transport implementation for Raft testing.

pub mod error;
pub mod mock;
pub mod network;

pub use error::TransportError;
pub use mock::MockTransport;
pub use mock::PeerReceivers; // Expose PeerReceivers for test setup
pub use mock::TransportRegistry; // Export the new registry
pub use network::NetworkOptions;

use raft_core::NodeId;
pub use raft_core::Transport; // Re-export core trait // Re-add missing NodeId import

use async_trait::async_trait;
use wcygan_raft_community_neoeinstein_prost::raft::v1::{
    AppendEntriesRequest, AppendEntriesResponse, RequestVoteRequest, RequestVoteResponse,
};

#[async_trait]
impl Transport for MockTransport {
    async fn send_append_entries(
        &self,
        peer_id: NodeId,
        request: AppendEntriesRequest,
    ) -> anyhow::Result<AppendEntriesResponse> {
        self.send_append_entries_impl(peer_id, request).await
    }

    async fn send_request_vote(
        &self,
        peer_id: NodeId,
        request: RequestVoteRequest,
    ) -> anyhow::Result<RequestVoteResponse> {
        self.send_request_vote_impl(peer_id, request).await
    }

    // TODO: Add method for InstallSnapshot RPC if needed
    // async fn send_install_snapshot(...)
}

// --- Original Placeholder Code (to be removed) ---
// pub fn add(left: u64, right: u64) -> u64 {
//     left + right
// }

#[cfg(test)]
mod tests {
    // use super::*;
    // Temporarily comment out tests until implementation is ready

    // #[test]
    // fn it_works() {
    //     let result = add(2, 2);
    //     assert_eq!(result, 4);
    // }

    // TODO: Add integration tests for MockTransport here or in a separate tests/ directory
}
