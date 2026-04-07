//! creating proposals from wallet data

use std::num::NonZeroU32;

use rand::rngs::OsRng;
use tonic::transport::Channel;
use tracing::{info, instrument};
use zcash_client_backend::{
    data_api::{
        WalletRead,
        wallet::{ConfirmationsPolicy, input_selection::GreedyInputSelector},
    },
    fees::{DustAction, DustOutputPolicy},
    proto::service::{
        BondInfoRequest, RawTransaction, compact_tx_streamer_client::CompactTxStreamerClient,
    },
    zip321::TransactionRequest,
};
use zcash_primitives::transaction::{
    StakingAction, StakingAction_WithdrawDelegationBond, builder::BuildConfig,
};
use zcash_proofs::prover::LocalTxProver;
use zcash_protocol::{
    ShieldedProtocol, TxId,
    consensus::{BlockHeight, Parameters},
    memo::{Memo, MemoBytes},
    value::Zatoshis,
};
use zcash_transparent::builder::TransparentSigningSet;
use zip32::AccountId;

use super::{
    LightWallet,
    error::{ProposeSendError, ProposeShieldError, WalletError},
};
use crate::{
    config::ChainType,
    data::proposal::{ExtraFee, ExtraFeeProposal, ProportionalFeeProposal, ZingoProposal},
    wallet::{error::WithdrawError, utils::ensure_sapling_params_on_disk},
};
use pepper_sync::{
    keys::transparent::TransparentScope,
    sync::ScanPriority,
    wallet::{NoteInterface, OrchardNote},
};
use zcash_primitives::transaction::builder::Builder as TxBuilder;

use zcash_primitives::transaction::fees::zip317::FeeError;

const EMPTY_MEMO_BYTES: [u8; 512] = {
    let mut bytes = [0; 512];
    bytes[0] = 0xf6;
    bytes
};

impl LightWallet {
    /// Creates a proposal from a transaction request.
    pub(crate) fn create_send_proposal(
        &mut self,
        request: TransactionRequest,
        account_id: zip32::AccountId,
    ) -> Result<ProportionalFeeProposal, ProposeSendError> {
        let memo = self.change_memo_from_transaction_request(&request);
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
                    .convert_if_network::<zcash_transparent::address::TransparentAddress>(
                        self.network.network_type(),
                    )
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
                return Err(ProposeShieldError::InsufficientFunds);
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
    ) -> Result<StakingProposal<ExtraFeeProposal>, ProposeSendError> {
        let memo = self.change_memo_from_transaction_request(&request);
        let input_selector = GreedyInputSelector::new();

        let extra = amount;
        let fee_rule = ExtraFee {
            base: zcash_primitives::transaction::fees::zip317::FeeRule::standard(),
            extra,
        };
        let change_strategy = zcash_client_backend::fees::zip317::SingleOutputChangeStrategy::new(
            // zcash_primitives::transaction::fees::zip317::FeeRule::standard(),
            fee_rule,
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
                ExtraFee<zcash_primitives::transaction::fees::zip317::FeeRule>,
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

    //create_unbonding_start_proposal
    /// Creates a proposal from a transaction request.
    pub(crate) async fn create_unbonding_start_proposal(
        &mut self,
        request: TransactionRequest,
        // staking_action: StakingAction,
        // challenge is not needed here. A simple [0u8; 32] will do
        // signature is not needed here. A simple [0u8; 32] will do
        // amount: Zatoshis, amount is also not needed.
        unique_pubkey: [u8; 32],
        target_finalizer: [u8; 32],
        account_id: zip32::AccountId,
    ) -> Result<StakingProposal<ExtraFeeProposal>, ProposeSendError> {
        let refund_address_count = self
            .transparent_addresses
            .keys()
            .filter(|&address_id| address_id.scope() == TransparentScope::Refund)
            .count() as u32;
        let memo = self.change_memo_from_transaction_request(&request);
        let input_selector = GreedyInputSelector::new();

        let fee_rule = ExtraFee {
            base: zcash_primitives::transaction::fees::zip317::FeeRule::standard(),
            extra: Zatoshis::const_from_u64(0),
        };
        let change_strategy = zcash_client_backend::fees::zip317::SingleOutputChangeStrategy::new(
            // zcash_primitives::transaction::fees::zip317::FeeRule::standard(),
            fee_rule,
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
                ExtraFee<zcash_primitives::transaction::fees::zip317::FeeRule>,
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
            kind: zcash_primitives::transaction::StakingActionKind::BeginDelegationUnbonding,
            amount_zats: 0,
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

    pub(crate) async fn create_retarget_proposal(
        &mut self,
        request: TransactionRequest,
        // staking_action: StakingAction,
        // challenge is not needed here. A simple [0u8; 32] will do
        // signature is not needed here. A simple [0u8; 32] will do
        // amount: Zatoshis, amount is also not needed.
        bond_key: [u8; 32],
        new_target_finalizer: [u8; 32],
        account_id: zip32::AccountId,
    ) -> Result<StakingProposal<ExtraFeeProposal>, ProposeSendError> {
        let refund_address_count = self
            .transparent_addresses
            .keys()
            .filter(|&address_id| address_id.scope() == TransparentScope::Refund)
            .count() as u32;
        let memo = self.change_memo_from_transaction_request(&request);
        let input_selector = GreedyInputSelector::new();

        let fee_rule = ExtraFee {
            base: zcash_primitives::transaction::fees::zip317::FeeRule::standard(),
            extra: Zatoshis::const_from_u64(0),
        };
        let change_strategy = zcash_client_backend::fees::zip317::SingleOutputChangeStrategy::new(
            // zcash_primitives::transaction::fees::zip317::FeeRule::standard(),
            fee_rule,
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
                ExtraFee<zcash_primitives::transaction::fees::zip317::FeeRule>,
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
            kind: zcash_primitives::transaction::StakingActionKind::RetargetDelegationBond,
            amount_zats: 0,
            arg32_0: bond_key,
            arg32_1: [0u8; 32],
            arg32_2: new_target_finalizer,
            arg32_3: [0u8; 32],
            arg64_0: [0u8; 64],
            arg64_1: [0u8; 64],
        };

        Ok(StakingProposal {
            proportional_fee_proposal: proposal,
            staking_action,
        })
    }

    //create_unbonding_start_proposal
    /// Creates a proposal from a transaction request.
    #[instrument(
        level = "info",
        name = "create_withdraw_bond_proposal",
        skip(self, request, unique_pubkey, target_finalizer, amount, account_id),
        err
    )]
    pub(crate) async fn create_withdraw_bond_proposal(
        &mut self,
        request: TransactionRequest,
        // staking_action: StakingAction,
        // challenge is not needed here. A simple [0u8; 32] will do
        // signature is not needed here. A simple [0u8; 32] will do
        // amount: Zatoshis, amount is also not needed.
        unique_pubkey: [u8; 32],
        target_finalizer: [u8; 32],
        amount: Zatoshis,
        account_id: zip32::AccountId,
    ) -> Result<StakingProposal<ExtraFeeProposal>, ProposeSendError> {
        let refund_address_count = self
            .transparent_addresses
            .keys()
            .filter(|&address_id| address_id.scope() == TransparentScope::Refund)
            .count() as u32;
        let memo = self.change_memo_from_transaction_request(&request);
        let input_selector = GreedyInputSelector::new();

        let fee_rule = ExtraFee {
            base: zcash_primitives::transaction::fees::zip317::FeeRule::standard(),
            extra: Zatoshis::const_from_u64(0),
        };

        let change_strategy = zcash_client_backend::fees::zip317::SingleOutputChangeStrategy::new(
            fee_rule,
            Some(memo),
            ShieldedProtocol::Orchard,
            DustOutputPolicy::new(DustAction::AddDustToFee, None),
        );
        let network = self.network;

        info!("network: {}", network);
        info!("request: {:#?}", request.to_uri());

        let proposal = match zcash_client_backend::data_api::wallet::propose_transfer::<
            LightWallet,
            ChainType,
            GreedyInputSelector<LightWallet>,
            zcash_client_backend::fees::zip317::SingleOutputChangeStrategy<
                ExtraFee<zcash_primitives::transaction::fees::zip317::FeeRule>,
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
            kind: zcash_primitives::transaction::StakingActionKind::WithdrawDelegationBond,
            amount_zats: amount.into_u64(),
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

    pub async fn withdraw_bond_using_orchard<P: Parameters>(
        &mut self,
        network: P,
        client: &mut CompactTxStreamerClient<Channel>,
        bond_key: &[u8; 32],
    ) -> Result<TxId, WithdrawError> {
        let orchard_tree = &self.shard_trees.orchard;

        let bond_value: u64 = client
            .get_bond_info(BondInfoRequest {
                bond_key: bond_key.to_vec(),
            })
            .await
            .ok()
            .expect("bond should exist")
            .into_inner()
            .amount;

        let ufvks = self.get_unified_full_viewing_keys().unwrap();

        let orchard_fvk = ufvks.get(&AccountId::ZERO).unwrap().orchard().unwrap();

        let orchard_ovk = orchard_fvk.to_ovk(zip32::Scope::External);

        let my_orchard_receiver = self
            .unified_addresses()
            .first_key_value()
            .unwrap()
            .1
            .orchard()
            .unwrap();

        let unified_key_store = self.unified_key_store.first_key_value().unwrap().1;

        let orchard_spending_key = orchard::keys::SpendingKey::try_from(unified_key_store).unwrap();

        let tip = self
            .chain_height()
            .ok()
            .flatten()
            .expect("chain height should already be known");

        let (target_height, latest_orchard_anchor_height) = self
            .get_target_and_anchor_heights(NonZeroU32::new(3).unwrap())
            .unwrap()
            .expect("Orchard action did not exist");

        let orchard_anchor = match orchard_tree
            .root_at_checkpoint_id(&latest_orchard_anchor_height.clone())
            .expect("Infallible MemoryShardStore")
        {
            Some(root) => orchard::Anchor::from(root),
            None => return Err(WithdrawError::MissingAnchor),
        };

        let mut txb = TxBuilder::new(
            network,
            target_height.into(),
            BuildConfig::Standard {
                sapling_anchor: None,
                orchard_anchor: Some(orchard_anchor),
            },
        );

        // notes to only cover fee.
        let mut fee_inputs_sum: u64 = 0;
        let mut spend_count: usize = 0;

        let mut fee_est: u64 = 10_000;

        let spendable_notes: Vec<&OrchardNote> = self
            .spendable_notes(latest_orchard_anchor_height, &[], AccountId::ZERO, false)
            .unwrap();

        for note in spendable_notes.iter() {
            let witness = match orchard_tree.witness_at_checkpoint_id(
                note.position().unwrap(),
                &latest_orchard_anchor_height.clone(),
            ) {
                Ok(Some(w)) => w,
                _ => continue,
            };
            let merkle_path = orchard::tree::MerklePath::from(witness);

            if txb
                .add_orchard_spend::<FeeError>(orchard_fvk.clone(), *note.note(), merkle_path)
                .is_ok()
            {
                fee_inputs_sum += note.note().value().inner();
                spend_count += 1;

                let orchard_actions = spend_count + 1;
                fee_est = 5000 * (orchard_actions as u64).max(2);
                fee_est = fee_est.max(10_000);

                if fee_inputs_sum >= fee_est {
                    break;
                }
            }
        }

        if fee_inputs_sum < fee_est {
            return Err(WithdrawError::NotEnoughFundsForFee); // not enough to pay fee
        }

        match txb.put_staking_action(
            StakingAction_WithdrawDelegationBond {
                amount_zats: bond_value,
                unique_pubkey: *bond_key,
                challenge: [0u8; 32],
                signature: [0u8; 64],
            }
            .to_union(),
        ) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[withdraw] txb.put_staking_action failed: {e:?}");
                return Err(WithdrawError::PutStakingActionFailed);
            }
        };

        let out_value = bond_value + (fee_inputs_sum - fee_est);

        match txb.add_orchard_output::<FeeError>(
            Some(orchard_ovk),
            *my_orchard_receiver,
            out_value,
            MemoBytes::from_bytes(&EMPTY_MEMO_BYTES).unwrap(),
        ) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[withdraw] txb.add_orchard_output failed: {e:?}");
                return Err(WithdrawError::AddOrchardOutputFailed);
            }
        };

        let signing_set = TransparentSigningSet::new();

        let params_dir = std::env::temp_dir().join("zingo-params");
        let (spend, output) = ensure_sapling_params_on_disk(params_dir)
            .expect("could not materialize embedded zcash params");

        let prover = LocalTxProver::new(&spend, &output);

        let rng = OsRng;

        let orchard_ask: orchard::keys::SpendAuthorizingKey = (&orchard_spending_key).into();

        let tx_res = match txb.build(
            &signing_set,
            &[],
            &[orchard_ask],
            rng,
            &prover,
            &prover,
            &zcash_primitives::transaction::fees::zip317::FeeRule::standard(),
        ) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[withdraw] txb.build failed: {e:?}");
                return Err(WithdrawError::BuildError);
            }
        };

        let tx = tx_res.transaction();
        let mut tx_bytes = vec![];
        match tx.write(&mut tx_bytes) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[withdraw] tx.write failed: {e:?}");
                return Err(WithdrawError::TxWrite);
            }
        };

        match client
            .send_transaction(RawTransaction {
                data: tx_bytes,
                height: 0,
            })
            .await
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[withdraw] client.send_transaction failed: {e:?}");
                return Err(WithdrawError::TxSend);
            }
        };
        Ok(tx.txid())
    }

    /// Stores a proposal in the `send_proposal` field.
    /// This field must be populated in order to then construct and transmit transactions.
    pub(crate) fn store_proposal(&mut self, proposal: ZingoProposal) {
        self.send_proposal = Some(proposal);
    }

    /// Takes the proposal from the `send_proposal` field, leaving the field empty.
    pub(crate) fn take_proposal(&mut self) -> Option<ZingoProposal> {
        self.send_proposal.take()
    }

    fn change_memo_from_transaction_request(&self, request: &TransactionRequest) -> MemoBytes {
        let mut recipient_uas = Vec::new();
        let mut refund_address_indexes = Vec::new();
        let mut refund_address_count = self
            .transparent_addresses
            .keys()
            .filter(|&address_id| address_id.scope() == TransparentScope::Refund)
            .count() as u32;
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
                        refund_address_indexes.push(refund_address_count);
                        refund_address_count += 1;
                    }
                    _ => (),
                }
            }
        }
        let uas_bytes = match zingo_memo::create_wallet_internal_memo_version_1(
            &self.network,
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

    /// Returns the block height at which all blocks equal to and above this height are scanned (scan ranges set to
    /// `Scanned`, `ScannedWithoutMapping` or `RefetchingNullifiers` priority).
    /// Returns `None` if `self.scan_ranges` is empty.
    ///
    /// Useful for determining which height all the nullifiers have been mapped from for guaranteeing if a note is
    /// unspent.
    ///
    /// `all_spends_known` may be set if all the spend locations are already known before scanning starts. For example,
    /// the location of all transparent spends are known due to the pre-scan gRPC calls. In this case, the height returned
    /// is the lowest height where there are no higher scan ranges with `FoundNote` or higher scan priority.
    pub(crate) fn spend_horizon(&self, all_spends_known: bool) -> Option<BlockHeight> {
        if let Some(scan_range) = self
            .sync_state
            .scan_ranges()
            .iter()
            .rev()
            .find(|scan_range| {
                if all_spends_known {
                    scan_range.priority() >= ScanPriority::FoundNote
                        || scan_range.priority() == ScanPriority::Scanning
                } else {
                    scan_range.priority() != ScanPriority::Scanned
                        && scan_range.priority() != ScanPriority::ScannedWithoutMapping
                        && scan_range.priority() != ScanPriority::RefetchingNullifiers
                }
            })
        {
            Some(scan_range.block_range().end)
        } else {
            self.sync_state
                .scan_ranges()
                .first()
                .map(|range| range.block_range().start)
        }
    }

    /// Returns `true` if all nullifiers above `note_height` have been checked for this note's spend status.
    ///
    /// Requires that `note_height >= spend_horizon` (all ranges above the note are scanned) and that every
    /// `refetch_nullifier_range` recorded on the note is fully contained within a `Scanned` scan range
    /// (nullifiers that were discarded due to memory constraints have since been re-fetched).
    pub(crate) fn note_spends_confirmed(
        &self,
        note_height: BlockHeight,
        spend_horizon: BlockHeight,
        refetch_nullifier_ranges: &[std::ops::Range<BlockHeight>],
    ) -> bool {
        note_height >= spend_horizon
            && refetch_nullifier_ranges.iter().all(|refetch_range| {
                self.sync_state.scan_ranges().iter().any(|scan_range| {
                    scan_range.priority() == ScanPriority::Scanned
                        && scan_range.block_range().contains(&refetch_range.start)
                        && scan_range.block_range().contains(&(refetch_range.end - 1))
                })
            })
    }
}

#[derive(Debug, Clone)]
pub struct StakingProposal<R: Clone> {
    proportional_fee_proposal: R,
    staking_action: StakingAction,
}

impl<R: Clone> StakingProposal<R> {
    pub fn new(proportional_fee_proposal: R, staking_action: StakingAction) -> Self {
        Self {
            proportional_fee_proposal,
            staking_action,
        }
    }
    pub fn proportional_fee_proposal(&self) -> &R {
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
            .expect("can propose from existing data");
    }
}
