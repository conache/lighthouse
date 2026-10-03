//! Tests for `InclusionListsByIndices` req/resp protocol

use super::lookups::SimulateConfig;
use super::*;
use crate::sync::network_context::InclusionListsByIndicesRequestParams;
use bls::Signature;
use eth2::types::ProposerPreparationData;
use types::{Address, EthSpec, InclusionList, ProgressiveTransactions};

const INCLUSION_LIST_SLOT: Slot = Slot::new(1);
const DEPENDENT_ROOT: Hash256 = Hash256::repeat_byte(1);

impl TestRig {
    /// Gives peers an inclusion list for each of the first `held` committee positions, then
    /// requests the first `requested` positions from a single peer.
    fn setup_inclusion_lists_by_indices_request(&mut self, requested: u64, held: u64) {
        self.new_connected_peer();
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
            .inclusion_lists_by_indices_request(params)
            .unwrap();
    }

    /// Registers the proposer of the next slot as one of this node's validators.
    async fn register_next_slot_proposer(&self) {
        let slot = self.harness.chain.slot().unwrap() + 1;
        let proposer_index = self
            .harness
            .chain
            .head_beacon_state_cloned()
            .get_beacon_proposer_index(slot, &self.harness.spec)
            .unwrap();

        self.harness
            .chain
            .execution_layer
            .as_ref()
            .unwrap()
            .update_proposer_preparation(
                slot.epoch(E::slots_per_epoch()),
                [(
                    &ProposerPreparationData {
                        validator_index: proposer_index as u64,
                        fee_recipient: Address::ZERO,
                    },
                    &None,
                )],
            )
            .await;
    }

    /// Asserts a single request was sent for every inclusion list committee position.
    fn assert_all_inclusion_lists_requested(&mut self) {
        let request = self
            .pop_received_network_event(|event| match event {
                NetworkMessage::SendRequest {
                    request: RequestType::InclusionListsByIndices(request),
                    ..
                } => Some(request.clone()),
                _ => None,
            })
            .unwrap();

        let chain = &self.harness.chain;
        let slot = chain.slot().unwrap();
        let (committee, dependent_root) = chain
            .inclusion_list_committee(chain.head_beacon_block_root(), slot)
            .unwrap();
        let requested_positions = request
            .indices
            .iter()
            .enumerate()
            .filter(|(_, requested)| *requested)
            .map(|(position, _)| position)
            .collect::<Vec<_>>();
        assert_eq!(request.slot, slot);
        assert_eq!(request.dependent_root, dependent_root);
        assert_eq!(
            requested_positions,
            (0..committee.len()).collect::<Vec<_>>()
        );
        self.assert_empty_network();
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

#[tokio::test]
async fn deadline_requests_missing_inclusion_lists() {
    let Some(mut r) = TestRig::new_after_heze() else {
        return;
    };
    r.new_connected_peer();
    r.register_next_slot_proposer().await;
    r.sync_manager.on_inclusion_list_deadline().await;
    r.assert_all_inclusion_lists_requested();
}

#[tokio::test]
async fn deadline_without_our_proposer_sends_nothing() {
    let Some(mut r) = TestRig::new_after_heze() else {
        return;
    };
    r.new_connected_peer();
    r.sync_manager.on_inclusion_list_deadline().await;
    r.assert_empty_network();
}
