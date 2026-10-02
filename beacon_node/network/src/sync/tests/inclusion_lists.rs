//! Tests for `InclusionListsByIndices` req/resp protocol

use super::lookups::SimulateConfig;
use super::*;
use crate::sync::network_context::InclusionListsByIndicesRequestParams;
use bls::Signature;
use tracing::Span;
use types::{InclusionList, ProgressiveTransactions};

const INCLUSION_LIST_SLOT: Slot = Slot::new(1);
const DEPENDENT_ROOT: Hash256 = Hash256::repeat_byte(1);

impl TestRig {
    /// Gives peers an inclusion list for each of the first `held` committee positions, then
    /// requests the first `requested` positions from a single peer.
    fn setup_inclusion_lists_by_indices_request(&mut self, requested: u64, held: u64) {
        let peer_id = self.new_connected_peer();
        self.network_inclusion_lists = (0..held)
            .map(|validator_index| {
                Arc::new(SignedInclusionList {
                    message: InclusionList {
                        slot: INCLUSION_LIST_SLOT,
                        validator_index,
                        dependent_root: DEPENDENT_ROOT,
                        transactions: ProgressiveTransactions::new(vec![]).unwrap(),
                    },
                    signature: Signature::empty(),
                })
            })
            .collect();

        let params = InclusionListsByIndicesRequestParams {
            slot: INCLUSION_LIST_SLOT,
            dependent_root: DEPENDENT_ROOT,
            requested: (0..requested)
                .map(|validator_index| (validator_index as usize, validator_index))
                .collect(),
        };
        self.sync_manager
            .network_context()
            .send_inclusion_lists_by_indices_request(peer_id, params, Span::none())
            .unwrap();
    }

    fn assert_inclusion_lists_by_indices_request_completed(&mut self) {
        assert_eq!(
            self.sync_manager
                .network_context()
                .inclusion_lists_by_indices_request_count(),
            0,
            "inclusion lists by indices request should no longer be active"
        );
    }
}

#[tokio::test]
async fn inclusion_lists_by_indices_request_completes() {
    let mut r = TestRig::default();
    r.setup_inclusion_lists_by_indices_request(3, 3);
    r.simulate(SimulateConfig::happy_path()).await;
    r.assert_inclusion_lists_by_indices_request_completed();
    r.assert_no_penalties();
}

#[tokio::test]
async fn inclusion_lists_for_another_slot_penalize_peer() {
    let mut r = TestRig::default();
    r.setup_inclusion_lists_by_indices_request(3, 3);
    r.simulate(SimulateConfig::happy_path().with_wrong_inclusion_list_slot_n_times(1))
        .await;
    r.assert_inclusion_lists_by_indices_request_completed();
    r.assert_penalties(&["UnrequestedSlot"]);
}

#[tokio::test]
async fn inclusion_lists_by_indices_short_response_completes() {
    let mut r = TestRig::default();
    r.setup_inclusion_lists_by_indices_request(3, 2);
    r.simulate(SimulateConfig::happy_path()).await;
    r.assert_inclusion_lists_by_indices_request_completed();
    r.assert_no_penalties();
}
