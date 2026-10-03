//! Tests for `InclusionListsByIndices` req/resp protocol

use super::lookups::SimulateConfig;
use super::*;
use crate::sync::network_context::InclusionListsByIndicesRequestParams;
use eth2::types::ProposerPreparationData;
use types::{Address, Domain, EthSpec, InclusionList, SignedRoot};

impl TestRig {
    /// A signed inclusion list for the current slot from the first committee member.
    fn committee_inclusion_list(&self) -> Arc<SignedInclusionList> {
        let chain = &self.harness.chain;
        let slot = chain.slot().unwrap();
        let (committee, dependent_root) = chain
            .inclusion_list_committee(chain.head_beacon_block_root(), slot)
            .unwrap();
        let validator_index = committee[0];

        let message = InclusionList {
            slot,
            validator_index,
            dependent_root,
            transactions: vec![vec![0xaa].try_into().unwrap()].try_into().unwrap(),
        };
        let epoch = slot.epoch(E::slots_per_epoch());
        let domain = chain.spec.get_domain(
            epoch,
            Domain::InclusionListCommittee,
            &chain.spec.fork_at_epoch(epoch),
            chain.genesis_validators_root,
        );
        let signature = self.harness.validator_keypairs[validator_index as usize]
            .sk
            .sign(message.signing_root(domain));
        Arc::new(SignedInclusionList { message, signature })
    }

    /// Makes peers serve `inclusion_list` at every committee position.
    fn serve_inclusion_list(&mut self, inclusion_list: &Arc<SignedInclusionList>) {
        self.network_inclusion_lists =
            vec![inclusion_list.clone(); E::inclusion_list_committee_size()];
    }

    /// Requests the first `positions` committee positions of the current slot from a single peer.
    fn request_inclusion_lists(&mut self, positions: usize) {
        self.new_connected_peer();
        let chain = &self.harness.chain;
        let slot = chain.slot().unwrap();
        let (committee, dependent_root) = chain
            .inclusion_list_committee(chain.head_beacon_block_root(), slot)
            .unwrap();
        let params = InclusionListsByIndicesRequestParams {
            slot,
            dependent_root,
            requested: committee
                .iter()
                .enumerate()
                .take(positions)
                .map(|(position, validator_index)| (position, *validator_index))
                .collect(),
        };
        self.sync_manager
            .network_context()
            .inclusion_lists_by_indices_request(params)
            .unwrap();
    }

    fn stored_inclusion_lists(
        &self,
        inclusion_list: &SignedInclusionList,
    ) -> Vec<SignedInclusionList> {
        self.harness
            .chain
            .inclusion_list_store
            .read()
            .get_signed_inclusion_lists(
                inclusion_list.message.slot,
                inclusion_list.message.dependent_root,
                &[inclusion_list.message.validator_index],
            )
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
async fn inclusion_lists_by_indices_request_completes_and_imports() {
    let mut r = TestRig::default();
    let inclusion_list = r.committee_inclusion_list();
    r.serve_inclusion_list(&inclusion_list);
    r.request_inclusion_lists(3);
    r.simulate(SimulateConfig::happy_path()).await;
    r.assert_inclusion_lists_by_indices_request_completed();
    r.assert_no_penalties();
    assert_eq!(
        r.stored_inclusion_lists(&inclusion_list),
        vec![(*inclusion_list).clone()]
    );
}

#[tokio::test]
async fn inclusion_lists_for_another_slot_penalize_peer() {
    let mut r = TestRig::default();
    let inclusion_list = r.committee_inclusion_list();
    r.serve_inclusion_list(&inclusion_list);
    r.request_inclusion_lists(3);
    r.simulate(SimulateConfig::happy_path().with_wrong_inclusion_list_slot_n_times(1))
        .await;
    r.assert_inclusion_lists_by_indices_request_completed();
    r.assert_penalties(&["UnrequestedSlot"]);
    assert!(r.stored_inclusion_lists(&inclusion_list).is_empty());
}

#[tokio::test]
async fn inclusion_lists_by_indices_short_response_completes() {
    let mut r = TestRig::default();
    r.request_inclusion_lists(3);
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
