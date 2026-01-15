//! creating proposals from wallet data

use tracing::instrument;
use zcash_client_backend::{
    data_api::wallet::{ConfirmationsPolicy, input_selection::GreedyInputSelector},
    fees::{DustAction, DustOutputPolicy},
    zip321::TransactionRequest,
};
use zcash_primitives::transaction::StakingAction;
use zcash_protocol::{
    ShieldedProtocol,
    consensus::{BlockHeight, Parameters},
    memo::{Memo, MemoBytes},
    value::Zatoshis,
};
use zcash_transparent::address::TransparentAddress;

use super::{
    LightWallet,
    error::{ProposeSendError, ProposeShieldError, WalletError},
};
use crate::{config::ChainType, data::proposal::ProportionalFeeProposal};
use pepper_sync::{keys::transparent::TransparentScope, sync::ScanPriority};

impl LightWallet {
    /// Creates a proposal from a transaction request.
    pub(crate) async fn create_send_proposal(
        &mut self,
        request: TransactionRequest,
        account_id: zip32::AccountId,
    ) -> Result<crate::data::proposal::ProportionalFeeProposal, ProposeSendError> {
        let refund_address_count = self
            .transparent_addresses
            .keys()
            .filter(|&address_id| address_id.scope() == TransparentScope::Refund)
            .count() as u32;
        let memo = self.change_memo_from_transaction_request(&request, refund_address_count);
        let input_selector = GreedyInputSelector::new();
        let change_strategy = zcash_client_backend::fees::zip317::SingleOutputChangeStrategy::new(
            zcash_primitives::transaction::fees::zip317::FeeRule::standard(),
            Some(memo),
            ShieldedProtocol::Orchard,
            DustOutputPolicy::new(DustAction::AddDustToFee, None),
        );
        let network = self.network;

        zcash_client_backend::data_api::wallet::propose_transfer::<
            LightWallet,
            ChainType,
            GreedyInputSelector<LightWallet>,
            zcash_client_backend::fees::zip317::SingleOutputChangeStrategy<
                zcash_primitives::transaction::fees::zip317::FeeRule,
                LightWallet,
            >,
            WalletError,
        >(
            self,
            &network,
            account_id,
            &input_selector,
            &change_strategy,
            request,
            // TODO: replace wallet min_confirmations field with confirmation policy to unify for all proposals
            ConfirmationsPolicy::new_symmetrical(self.wallet_settings.min_confirmations, false),
        )
        .map_err(ProposeSendError::Proposal)
    }

    /// The shield operation consumes a proposal that transfers value
    /// into the Orchard pool.
    ///
    /// The proposal is generated with this method, which operates on
    /// the balance transparent pool, without other input.
    /// In other words, shield does not take a user-specified amount
    /// to shield, rather it consumes all transparent value in the wallet that
    /// can be consumed without costing more in zip317 fees than is being transferred.
    #[instrument(name = "create_shield_proposal", skip(self), err, level = "info")]
    pub(crate) async fn create_shield_proposal(
        &mut self,
        account_id: zip32::AccountId,
    ) -> Result<crate::data::proposal::ProportionalFeeShieldProposal, ProposeShieldError> {
        let input_selector = GreedyInputSelector::new();
        let change_strategy = zcash_client_backend::fees::zip317::SingleOutputChangeStrategy::new(
            zcash_primitives::transaction::fees::zip317::FeeRule::standard(),
            None,
            ShieldedProtocol::Orchard,
            DustOutputPolicy::new(DustAction::AllowDustChange, None),
        );
        let network = self.network;

        // TODO: store t addrs as concrete types instead of encoded
        let transparent_addresses = self
            .transparent_addresses
            .values()
            .map(|address| {
                Ok(zcash_address::ZcashAddress::try_from_encoded(address)?
                    .convert_if_network::<TransparentAddress>(self.network.network_type())
                    .expect("incorrect network should be checked on wallet load"))
            })
            .collect::<Result<Vec<_>, zcash_address::ParseError>>()?;

        let proposed_shield = zcash_client_backend::data_api::wallet::propose_shielding::<
            LightWallet,
            ChainType,
            GreedyInputSelector<LightWallet>,
            zcash_client_backend::fees::zip317::SingleOutputChangeStrategy<
                zcash_primitives::transaction::fees::zip317::FeeRule,
                LightWallet,
            >,
            WalletError,
        >(
            self,
            &network,
            &input_selector,
            &change_strategy,
            Zatoshis::const_from_u64(1),
            &transparent_addresses,
            account_id,
            // TODO: replace wallet min_confirmations field with confirmation policy to unify for all proposals
            ConfirmationsPolicy::new_symmetrical(self.wallet_settings.min_confirmations, false),
        )
        .map_err(ProposeShieldError::Component)?;

        for step in proposed_shield.steps().iter() {
            if step
                .balance()
                .proposed_change()
                .iter()
                .fold(0, |total_out, output| total_out + output.value().into_u64())
                == 0
            {
                return Err(ProposeShieldError::Insufficient);
            }
        }

        Ok(proposed_shield)
    }

    /// Creates a proposal from a transaction request.
    pub(crate) async fn create_stake_proposal(
        &mut self,
        request: TransactionRequest,
        // staking_action: StakingAction,
        // challenge is not needed here. A simple [0u8; 32] will do
        // signature is not needed here. A simple [0u8; 32] will do
        amount: Zatoshis,
        unique_pubkey: [u8; 32],
        target_finalizer: [u8; 32],
        account_id: zip32::AccountId,
    ) -> Result<StakingProposal, ProposeSendError> {
        let refund_address_count = self
            .transparent_addresses
            .keys()
            .filter(|&address_id| address_id.scope() == TransparentScope::Refund)
            .count() as u32;
        let memo = self.change_memo_from_transaction_request(&request, refund_address_count);
        let input_selector = GreedyInputSelector::new();
        let change_strategy = zcash_client_backend::fees::zip317::SingleOutputChangeStrategy::new(
            zcash_primitives::transaction::fees::zip317::FeeRule::standard(),
            Some(memo),
            ShieldedProtocol::Orchard,
            DustOutputPolicy::new(DustAction::AddDustToFee, None),
        );
        let network = self.network;

        let proposal = match zcash_client_backend::data_api::wallet::propose_transfer::<
            LightWallet,
            ChainType,
            GreedyInputSelector<LightWallet>,
            zcash_client_backend::fees::zip317::SingleOutputChangeStrategy<
                zcash_primitives::transaction::fees::zip317::FeeRule,
                LightWallet,
            >,
            WalletError,
        >(
            self,
            &network,
            account_id,
            &input_selector,
            &change_strategy,
            request,
            // TODO: replace wallet min_confirmations field with confirmation policy to unify for all proposals
            ConfirmationsPolicy::new_symmetrical(self.wallet_settings.min_confirmations, false),
        )
        .map_err(ProposeSendError::Proposal)
        {
            Err(e) => return Err(e),
            Ok(proposal) => proposal,
        };

        let staking_action = StakingAction {
            kind: zcash_primitives::transaction::StakingActionKind::CreateNewDelegationBond,
            amount_zats: amount.into(),
            arg32_0: unique_pubkey,
            arg32_1: [0u8; 32],
            arg32_2: target_finalizer,
            arg32_3: [0u8; 32],
            arg64_0: [0u8; 64],
            arg64_1: [0u8; 64],
        };

        Ok(StakingProposal {
            proportional_fee_proposal: proposal,
            staking_action,
        })
    }

    fn change_memo_from_transaction_request(
        &self,
        request: &TransactionRequest,
        mut refund_address_count: u32,
    ) -> MemoBytes {
        let mut recipient_uas = Vec::new();
        let mut refund_address_indexes = Vec::new();
        for payment in request.payments().values() {
            if let Ok(address) = payment
                .recipient_address()
                .clone()
                .convert_if_network::<zcash_keys::address::Address>(self.network.network_type())
            {
                match address {
                    zcash_keys::address::Address::Unified(unified_address) => {
                        recipient_uas.push(unified_address);
                    }
                    zcash_keys::address::Address::Tex(_) => {
                        // TODO: rework: only need to encode highest used refund address index, not all of them
                        refund_address_indexes.push(refund_address_count);
                        refund_address_count += 1;
                    }
                    _ => (),
                }
            }
        }
        let uas_bytes = match zingo_memo::create_wallet_internal_memo_version_1(
            recipient_uas.as_slice(),
            refund_address_indexes.as_slice(),
        ) {
            Ok(bytes) => bytes,
            Err(e) => {
                log::error!(
                    "Could not write uas to memo field: {e}\n\
        Your wallet will display an incorrect sent-to address. This is a visual error only.\n\
        The correct address was sent to."
                );
                [0; 511]
            }
        };
        MemoBytes::from(Memo::Arbitrary(Box::new(uas_bytes)))
    }

    /// Returns the block height at which all blocks equal to and above this height are scanned.
    /// Returns `None` if `self.scan_ranges` is empty.
    ///
    /// Useful for determining which height all the nullifiers have been mapped from for guaranteeing if a note is
    /// unspent.
    pub(crate) fn spend_horizon(&self) -> Option<BlockHeight> {
        if let Some(scan_range) = self
            .sync_state
            .scan_ranges()
            .iter()
            .rev()
            .find(|scan_range| scan_range.priority() != ScanPriority::Scanned)
        {
            Some(scan_range.block_range().end)
        } else {
            self.sync_state
                .scan_ranges()
                .first()
                .map(|range| range.block_range().start)
        }
    }
}

#[derive(Debug, Clone)]
pub struct StakingProposal {
    proportional_fee_proposal: crate::data::proposal::ProportionalFeeProposal,
    staking_action: StakingAction,
}

impl StakingProposal {
    pub fn new(
        proportional_fee_proposal: ProportionalFeeProposal,
        staking_action: StakingAction,
    ) -> Self {
        Self {
            proportional_fee_proposal,
            staking_action,
        }
    }
    pub fn proportional_fee_proposal(&self) -> &ProportionalFeeProposal {
        &self.proportional_fee_proposal
    }
    pub fn staking_action(&self) -> &StakingAction {
        &self.staking_action
    }
}

#[cfg(test)]
mod test {
    use zcash_protocol::{PoolType, ShieldedProtocol};

    use crate::{
        testutils::lightclient::from_inputs::transaction_request_from_send_inputs,
        wallet::disk::testing::examples,
    };

    /// this test loads an example wallet with existing sapling finds
    #[ignore = "for some reason this is does not work without network, even though it should be possible"]
    #[tokio::test]
    async fn example_mainnet_hhcclaltpcckcsslpcnetblr_80b5594ac_propose_100_000_to_self() {
        let client = examples::NetworkSeedVersion::Mainnet(
            examples::MainnetSeedVersion::HotelHumor(examples::HotelHumorVersion::Latest),
        )
        .load_example_wallet_with_client()
        .await;
        let mut wallet = client.wallet.write().await;

        let pool = PoolType::Shielded(ShieldedProtocol::Orchard);
        let self_address = wallet.get_address(pool);

        let receivers = vec![(self_address.as_str(), 100_000, None)];
        let request = transaction_request_from_send_inputs(receivers)
            .expect("actually all of this logic oughta be internal to propose");

        wallet
            .create_send_proposal(request, zip32::AccountId::ZERO)
            .await
            .expect("can propose from existing data");
    }
}
