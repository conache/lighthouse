use lighthouse_network::rpc::methods::InclusionListsByIndicesRequest;
use std::sync::Arc;
use types::{EthSpec, SignedInclusionList};

use super::{ActiveRequestItems, LookupVerifyError};

pub struct InclusionListsByIndicesRequestItems<E: EthSpec> {
    request: InclusionListsByIndicesRequest<E>,
    /// Validator indices at the requested committee positions.
    validator_indices: Vec<u64>,
    items: Vec<Arc<SignedInclusionList>>,
}

impl<E: EthSpec> InclusionListsByIndicesRequestItems<E> {
    pub fn new(
        request: InclusionListsByIndicesRequest<E>,
        requested_validator_indices: Vec<u64>,
    ) -> Self {
        Self {
            request,
            validator_indices: requested_validator_indices,
            items: vec![],
        }
    }
}

impl<E: EthSpec> ActiveRequestItems for InclusionListsByIndicesRequestItems<E> {
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
        if !self.validator_indices.contains(&validator_index) {
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
        Ok(self.items.len() >= self.validator_indices.len())
    }

    fn consume(&mut self) -> Vec<Self::Item> {
        std::mem::take(&mut self.items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bls::Signature;
    use types::{
        Hash256, InclusionList, InclusionListBits, MinimalEthSpec as E, ProgressiveTransactions,
        Slot,
    };

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
    fn request_items() -> InclusionListsByIndicesRequestItems<E> {
        let mut indices = InclusionListBits::<E>::new();
        for position in 0..3 {
            indices.set(position, true).unwrap();
        }
        let request = InclusionListsByIndicesRequest {
            slot: SLOT,
            dependent_root: DEPENDENT_ROOT,
            indices,
        };
        InclusionListsByIndicesRequestItems::new(request, vec![10, 11, 12])
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
}
