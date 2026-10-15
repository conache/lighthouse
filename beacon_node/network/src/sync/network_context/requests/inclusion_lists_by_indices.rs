use lighthouse_network::rpc::methods::InclusionListsByIndicesRequest;
use std::collections::HashSet;
use std::sync::Arc;
use types::{EthSpec, Hash256, InclusionListBits, SignedInclusionList, Slot};

use super::{ActiveRequestItems, LookupVerifyError};

/// Inclusion list committee positions, each with the validator at that position.
pub type InclusionListCommitteePositions = Vec<(usize, u64)>;

#[derive(Debug, Clone)]
pub struct InclusionListsByIndicesRequestParams {
    pub slot: Slot,
    pub dependent_root: Hash256,
    /// Requested inclusion list committee positions, each with the validator index
    /// corresponding to that position.
    pub requested: InclusionListCommitteePositions,
}

impl InclusionListsByIndicesRequestParams {
    pub fn try_into_request<E: EthSpec>(
        self,
    ) -> Result<InclusionListsByIndicesRequest<E>, &'static str> {
        let mut indices = InclusionListBits::<E>::new();
        for (position, _) in &self.requested {
            indices
                .set(*position, true)
                .map_err(|_| "Position exceeds the inclusion list committee size")?;
        }
        Ok(InclusionListsByIndicesRequest {
            slot: self.slot,
            dependent_root: self.dependent_root,
            indices,
        })
    }
}

pub struct InclusionListsByIndicesRequestItems {
    request: InclusionListsByIndicesRequestParams,
    /// Distinct validators in `request`: a validator holding multiple inclusion list
    /// committee positions is served a single list.
    requested_validators: HashSet<u64>,
    items: Vec<Arc<SignedInclusionList>>,
}

impl InclusionListsByIndicesRequestItems {
    pub fn new(request: InclusionListsByIndicesRequestParams) -> Self {
        let requested_validators = request
            .requested
            .iter()
            .map(|(_, validator_index)| *validator_index)
            .collect();
        Self {
            request,
            requested_validators,
            items: vec![],
        }
    }
}

impl ActiveRequestItems for InclusionListsByIndicesRequestItems {
    type Item = Arc<SignedInclusionList>;

    /// Appends a chunk to this multi-item request. Returns `true` once an inclusion list has been
    /// received for every requested validator, resolving the request before the stream terminator.
    fn add(&mut self, inclusion_list: Self::Item) -> Result<bool, LookupVerifyError> {
        let slot = inclusion_list.message.slot;
        if slot != self.request.slot {
            return Err(LookupVerifyError::UnrequestedSlot(slot));
        }

        let dependent_root = inclusion_list.message.dependent_root;
        if dependent_root != self.request.dependent_root {
            return Err(LookupVerifyError::UnrequestedDependentRoot(dependent_root));
        }

        let validator_index = inclusion_list.message.validator_index;
        if !self.requested_validators.contains(&validator_index) {
            return Err(LookupVerifyError::UnrequestedIndex(validator_index));
        }

        if self
            .items
            .iter()
            .any(|item| item.message.validator_index == validator_index)
        {
            return Err(LookupVerifyError::DuplicatedData(slot, validator_index));
        }

        self.items.push(inclusion_list);

        Ok(self.items.len() >= self.requested_validators.len())
    }

    fn consume(&mut self) -> Vec<Self::Item> {
        std::mem::take(&mut self.items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bls::Signature;
    use types::{InclusionList, MinimalEthSpec as E, ProgressiveTransactions};

    const SLOT: Slot = Slot::new(1);
    const DEPENDENT_ROOT: Hash256 = Hash256::repeat_byte(1);

    fn inclusion_list(
        slot: Slot,
        dependent_root: Hash256,
        validator_index: u64,
    ) -> Arc<SignedInclusionList> {
        Arc::new(SignedInclusionList {
            message: InclusionList {
                slot,
                validator_index,
                dependent_root,
                transactions: ProgressiveTransactions::new(vec![]).unwrap(),
            },
            signature: Signature::empty(),
        })
    }

    /// Build the request items corresponding to validators 10, 11, and 12
    /// at inclusion list committee positions 0, 1 and 2.
    fn request_items() -> InclusionListsByIndicesRequestItems {
        InclusionListsByIndicesRequestItems::new(InclusionListsByIndicesRequestParams {
            slot: SLOT,
            dependent_root: DEPENDENT_ROOT,
            requested: vec![(0, 10), (1, 11), (2, 12)],
        })
    }

    #[test]
    fn complete_response_resolves_request() {
        let mut items = request_items();
        assert_eq!(
            items.add(inclusion_list(SLOT, DEPENDENT_ROOT, 10)),
            Ok(false)
        );
        assert_eq!(
            items.add(inclusion_list(SLOT, DEPENDENT_ROOT, 11)),
            Ok(false)
        );
        assert_eq!(
            items.add(inclusion_list(SLOT, DEPENDENT_ROOT, 12)),
            Ok(true)
        );
        assert_eq!(items.consume().len(), 3);
    }

    #[test]
    fn rejects_lists_that_were_not_requested() {
        let other_slot = SLOT + 1;
        let other_dependent_root = Hash256::repeat_byte(2);
        let unrequested_validator = 13;

        assert_eq!(
            request_items().add(inclusion_list(other_slot, DEPENDENT_ROOT, 10)),
            Err(LookupVerifyError::UnrequestedSlot(other_slot))
        );
        assert_eq!(
            request_items().add(inclusion_list(SLOT, other_dependent_root, 10)),
            Err(LookupVerifyError::UnrequestedDependentRoot(
                other_dependent_root
            ))
        );
        assert_eq!(
            request_items().add(inclusion_list(SLOT, DEPENDENT_ROOT, unrequested_validator)),
            Err(LookupVerifyError::UnrequestedIndex(unrequested_validator))
        );
    }

    #[test]
    fn rejects_a_second_list_from_the_same_validator() {
        let mut items = request_items();
        assert_eq!(
            items.add(inclusion_list(SLOT, DEPENDENT_ROOT, 10)),
            Ok(false)
        );
        assert_eq!(
            items.add(inclusion_list(SLOT, DEPENDENT_ROOT, 10)),
            Err(LookupVerifyError::DuplicatedData(SLOT, 10))
        );
    }

    #[test]
    fn validator_holding_several_positions_completes_with_one_list() {
        let mut items =
            InclusionListsByIndicesRequestItems::new(InclusionListsByIndicesRequestParams {
                slot: SLOT,
                dependent_root: DEPENDENT_ROOT,
                requested: vec![(0, 10), (1, 10), (2, 11)],
            });
        assert_eq!(
            items.add(inclusion_list(SLOT, DEPENDENT_ROOT, 10)),
            Ok(false)
        );
        assert_eq!(
            items.add(inclusion_list(SLOT, DEPENDENT_ROOT, 11)),
            Ok(true)
        );
    }

    #[test]
    fn try_into_request_sets_the_requested_positions() {
        let params = InclusionListsByIndicesRequestParams {
            slot: SLOT,
            dependent_root: DEPENDENT_ROOT,
            requested: vec![(0, 10), (5, 11)],
        };
        let request = params.clone().try_into_request::<E>().unwrap();
        assert_eq!(request.slot, SLOT);
        assert_eq!(request.dependent_root, DEPENDENT_ROOT);
        assert_eq!(
            request
                .indices
                .iter()
                .enumerate()
                .filter(|(_, is_set)| *is_set)
                .map(|(position, _)| position)
                .collect::<Vec<_>>(),
            vec![0, 5]
        );

        // There are only 16 inclusion list committee positions
        let out_of_range = InclusionListsByIndicesRequestParams {
            requested: vec![(16, 10)],
            ..params
        };
        assert!(out_of_range.try_into_request::<E>().is_err());
    }
}
