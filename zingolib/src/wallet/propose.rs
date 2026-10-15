//! creating proposals from wallet data

use rand::rngs::OsRng;
use tonic::transport::Channel;
use tracing::instrument;
use zcash_address::ZcashAddress;
use zcash_client_backend::{
    data_api::{
        CoinbaseFilter, WalletRead,
        wallet::{
            ConfirmationsPolicy,
            input_selection::{GreedyInputSelector, SpendPolicy},
        },
    },
    fees::{DustAction, DustOutputPolicy},
    proto::service::{
        BondInfoRequest, RawTransaction, compact_tx_streamer_client::CompactTxStreamerClient,
    },
    zip321::TransactionRequest,
};
use zcash_primitives::{
    bft::FinalizerAddress,
    transaction::{
        StakingAction,
        builder::{BuildConfig, BundlePadding},
    },
};
use zcash_proofs::prover::LocalTxProver;
use zcash_protocol::{
    PoolType, ShieldedPool, TxId,
    consensus::{self, BlockHeight, NetworkConstants, Parameters},
    memo::{Memo, MemoBytes},
    value::Zatoshis,
};
use zcash_transparent::builder::TransparentSigningSet;

use super::{
    LightWallet,
    error::{KeyError, ProposeSendError, ProposeShieldError, WalletError},
};
use crate::{
    config::ChainType,
    data::proposal::{ExtraFee, ExtraFeeProposal, ProportionalFeeProposal, ZingoProposal},
    wallet::{error::WithdrawError, utils::ensure_sapling_params_on_disk},
};
use pepper_sync::{
    keys::transparent::TransparentScope,
    sync::ScanPriority,
    wallet::{IronwoodNote, KeyIdInterface, NoteInterface, traits::SyncWallet},
};
use zcash_primitives::transaction::builder::Builder as TxBuilder;
use zingo_status::confirmation_status::ConfirmationStatus;

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
            ShieldedPool::Ironwood,
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
            &SpendPolicy::default(),
            None,
            None,
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
            ShieldedPool::Ironwood,
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
            CoinbaseFilter::AllTransparentOutputs,
            None,
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

    /// Builds a proposal that moves `amount` from one of this wallet's pools into another, by
    /// paying one of the wallet's own addresses.
    ///
    /// `from` restricts input selection to a single shielded pool; `to` picks the receiver the
    /// payment is made to. On a network where the Ironwood pool is active, a payment to the
    /// wallet's Orchard receiver is built in the Ironwood bundle, so `Orchard` and `Ironwood`
    /// destinations both land in Ironwood: this is the ZIP 318 style Orchard -> Ironwood migration
    /// in its simplest form, one transaction at a time. To move transparent funds into a shielded
    /// pool use [`LightWallet::create_shield_proposal`] instead.
    ///
    /// Change returns to the `from` pool, so exactly `amount` crosses into `to`.
    #[instrument(
        name = "create_pool_transfer_proposal",
        skip(self),
        err,
        level = "info"
    )]
    pub(crate) fn create_pool_transfer_proposal(
        &mut self,
        from: ShieldedPool,
        to: PoolType,
        amount: Zatoshis,
        account_id: zip32::AccountId,
    ) -> Result<ProportionalFeeProposal, ProposeSendError> {
        if PoolType::Shielded(from) == to {
            return Err(ProposeSendError::SamePool);
        }
        let destination = self.own_address_for_pool(to, account_id)?;
        let request =
            TransactionRequest::new(vec![zcash_client_backend::zip321::Payment::without_memo(
                destination,
                amount,
            )])?;

        let memo = self.change_memo_from_transaction_request(&request);
        let input_selector = GreedyInputSelector::new();
        let change_strategy = zcash_client_backend::fees::zip317::SingleOutputChangeStrategy::new(
            zcash_primitives::transaction::fees::zip317::FeeRule::standard(),
            Some(memo),
            from,
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
            ConfirmationsPolicy::new_symmetrical(self.wallet_settings.min_confirmations, false),
            &SpendPolicy::shielded_pools([from]),
            None,
            None,
        )
        .map_err(ProposeSendError::Proposal)
    }

    /// One of this wallet's own addresses that carries a receiver for `pool`, as a
    /// [`ZcashAddress`] the proposal machinery routes into that pool.
    ///
    /// Ironwood shares the Orchard receiver: a unified address with only an Orchard receiver is
    /// returned for both, and the backend places the payment in the pool that is active.
    fn own_address_for_pool(
        &self,
        pool: PoolType,
        account_id: zip32::AccountId,
    ) -> Result<ZcashAddress, ProposeSendError> {
        let network_type = self.network.network_type();
        let unified_address = self
            .unified_addresses
            .iter()
            .find(|(id, _)| id.account_id == account_id)
            .map(|(_, address)| address)
            .ok_or(ProposeSendError::NoReceiverForPool(pool))?;

        let encoded = match pool {
            PoolType::Transparent => {
                let address = self
                    .transparent_addresses
                    .iter()
                    .find(|(id, _)| {
                        id.account_id() == account_id && id.scope() == TransparentScope::External
                    })
                    .map(|(_, address)| address.clone())
                    .ok_or(ProposeSendError::NoReceiverForPool(pool))?;
                return Ok(ZcashAddress::try_from_encoded(&address)?);
            }
            PoolType::Shielded(ShieldedPool::Sapling) => {
                let receiver = unified_address
                    .sapling()
                    .ok_or(ProposeSendError::NoReceiverForPool(pool))?;
                zcash_keys::encoding::encode_payment_address(
                    self.network.hrp_sapling_payment_address(),
                    receiver,
                )
            }
            PoolType::Shielded(ShieldedPool::Orchard | ShieldedPool::Ironwood) => {
                let receiver = unified_address
                    .orchard()
                    .ok_or(ProposeSendError::NoReceiverForPool(pool))?;
                pepper_sync::keys::encode_orchard_receiver(&self.network, receiver)?
            }
        };
        let _ = network_type;
        Ok(ZcashAddress::try_from_encoded(&encoded)?)
    }

    /// Builds a proposal that funds a staking action from this wallet's shielded notes.
    ///
    /// The proposal pays nobody. Its fee is the standard ZIP 317 fee plus `extra`: the bond amount
    /// for a `CreateNewDelegationBond`, zero for every other action. The staking action is
    /// attached when the transaction is built; the builder then moves the bond amount into the
    /// staking pool and signs the action with the bond key it derives from the account's spending
    /// key.
    async fn create_staking_proposal(
        &mut self,
        extra: Zatoshis,
        account_id: zip32::AccountId,
    ) -> Result<ExtraFeeProposal, ProposeSendError> {
        let request = TransactionRequest::empty();
        let memo = self.change_memo_from_transaction_request(&request);
        let input_selector = GreedyInputSelector::new();
        let fee_rule = ExtraFee {
            base: zcash_primitives::transaction::fees::zip317::FeeRule::standard(),
            extra,
        };
        let change_strategy = zcash_client_backend::fees::zip317::SingleOutputChangeStrategy::new(
            fee_rule,
            Some(memo),
            ShieldedPool::Ironwood,
            DustOutputPolicy::new(DustAction::AddDustToFee, None),
        );
        let network = self.network;

        zcash_client_backend::data_api::wallet::propose_transfer::<
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
            ConfirmationsPolicy::new_symmetrical(self.wallet_settings.min_confirmations, false),
            &SpendPolicy::default(),
            None,
            None,
        )
        .map_err(ProposeSendError::Proposal)
    }

    /// The height the next transaction targets: one above the last known chain height.
    fn next_target_height(&self) -> Result<BlockHeight, ProposeSendError> {
        self.sync_state
            .last_known_chain_height()
            .map(|height| height + 1)
            .ok_or(ProposeSendError::NoSyncData)
    }

    /// The account's bond root key, from which every bond key of the account derives.
    fn bond_spending_key(
        &self,
        account_id: zip32::AccountId,
    ) -> Result<zcash_keys::keys::UnifiedSpendingKey, ProposeSendError> {
        let usk: zcash_keys::keys::UnifiedSpendingKey = self
            .unified_key_store
            .get(&account_id)
            .ok_or(KeyError::NoAccountKeys)?
            .try_into()?;
        Ok(usk)
    }

    /// Creates a proposal that stakes `amount` to `target_finalizer` in a new delegation bond.
    ///
    /// The bond key is derived from the account's bond root key, the target, the amount and a
    /// fresh salt, exactly as the node's wallet does, so a wallet restored from seed recovers
    /// the key from the mined action alone.
    pub(crate) async fn create_stake_proposal(
        &mut self,
        amount: Zatoshis,
        target_finalizer: FinalizerAddress,
        account_id: zip32::AccountId,
    ) -> Result<StakingProposal<ExtraFeeProposal>, ProposeSendError> {
        if !target_finalizer.verify() {
            return Err(ProposeSendError::InvalidFinalizerAddress);
        }

        let usk = self.bond_spending_key(account_id)?;
        let mut bond_salt = [0u8; 32];
        rand::RngCore::fill_bytes(&mut OsRng, &mut bond_salt);
        let signing_key = usk.bond().derive_signing_key(
            &target_finalizer.pub_key.0,
            amount.into_u64(),
            &bond_salt,
        );
        let unique_pubkey = <[u8; 32]>::from(
            zcash_primitives::ed25519_zebra::VerificationKeyBytes::from(&signing_key),
        );

        let staking_action = StakingAction::CreateNewDelegationBond {
            amount_zats: amount.into_u64(),
            unique_pubkey,
            bond_salt,
            target_finalizer,
            signature: [0u8; 64],
        };
        let target_height = self.next_target_height()?;
        if !staking_action.amount_is_valid_at(&self.network, target_height.into()) {
            return Err(ProposeSendError::BondAmountNotPowerOfTen(amount.into_u64()));
        }

        let proposal = self.create_staking_proposal(amount, account_id).await?;

        Ok(StakingProposal {
            proportional_fee_proposal: proposal,
            staking_action,
        })
    }

    /// Creates a proposal that begins unbonding the bond `bond_key`, which this wallet created.
    pub(crate) async fn create_unbonding_start_proposal(
        &mut self,
        bond_key: [u8; 32],
        account_id: zip32::AccountId,
    ) -> Result<StakingProposal<ExtraFeeProposal>, ProposeSendError> {
        self.bond_create_action(bond_key)
            .ok_or(ProposeSendError::UnknownBond(bond_key))?;

        let staking_action = StakingAction::BeginDelegationUnbonding {
            unique_pubkey: bond_key,
            signature: [0u8; 64],
        };
        let proposal = self
            .create_staking_proposal(Zatoshis::ZERO, account_id)
            .await?;

        Ok(StakingProposal {
            proportional_fee_proposal: proposal,
            staking_action,
        })
    }

    /// Creates a proposal that moves the bond `bond_key` to `to_finalizer`.
    ///
    /// Consensus validates the action's `from_finalizer` against the bond's current target, so
    /// it is replayed from this wallet's own mined create and retarget actions.
    pub(crate) async fn create_retarget_proposal(
        &mut self,
        bond_key: [u8; 32],
        to_finalizer: FinalizerAddress,
        account_id: zip32::AccountId,
    ) -> Result<StakingProposal<ExtraFeeProposal>, ProposeSendError> {
        if !to_finalizer.verify() {
            return Err(ProposeSendError::InvalidFinalizerAddress);
        }
        let from_finalizer = self
            .current_bond_target_address(bond_key)
            .ok_or(ProposeSendError::UnknownBond(bond_key))?;

        let staking_action = StakingAction::RetargetDelegationBond {
            unique_pubkey: bond_key,
            signature: [0u8; 64],
            from_finalizer,
            to_finalizer,
        };
        let proposal = self
            .create_staking_proposal(Zatoshis::ZERO, account_id)
            .await?;

        Ok(StakingProposal {
            proportional_fee_proposal: proposal,
            staking_action,
        })
    }

    /// The action that created bond `bond_key`, if this wallet created it.
    ///
    /// Both `CreateNewDelegationBond` and `ConvertFinalizerRewardToDelegationBond` create bonds
    /// and carry the terms the bond key derives from.
    pub(crate) fn bond_create_action(&self, bond_key: [u8; 32]) -> Option<StakingAction> {
        self.wallet_transactions
            .values()
            .filter_map(|transaction| transaction.staking_data())
            .find(|action| action.bond_terms().is_some() && action.unique_pubkey() == bond_key)
    }

    /// The bond's current target as a full finalizer address, replayed from this wallet's own
    /// transactions: the latest create or retarget action for the bond carries the address.
    ///
    /// Confirmed actions are ordered by height; pending ones count as newest.
    pub fn current_bond_target_address(&self, bond_key: [u8; 32]) -> Option<FinalizerAddress> {
        let mut candidates = self
            .wallet_transactions
            .values()
            .filter(|transaction| !transaction.status().is_failed())
            .filter_map(|transaction| {
                let action = transaction.staking_data()?;
                if action.unique_pubkey() != bond_key {
                    return None;
                }
                let address = crate::crosslink::action_target_address(&action)?;
                let order = transaction
                    .status()
                    .get_confirmed_height()
                    .map_or(u32::MAX, u32::from);
                Some((order, transaction.datetime(), address))
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|(height, time, _)| (*height, *time));
        candidates.pop().map(|(_, _, address)| address)
    }

    /// Withdraws the unbonded bond `bond_key` back into this wallet's Ironwood pool, building and
    /// broadcasting the transaction directly.
    ///
    /// A withdrawal is the one staking action the proposal machinery cannot express: the bond's
    /// value enters the transaction from the staking pool, not from a note. So this selects
    /// Ironwood notes to cover the fee, adds a single Ironwood output worth the bond plus the
    /// change, attaches the action, and signs it with the bond key recovered from the action
    /// that created the bond. The transaction is recorded in the wallet as pending once sent.
    pub async fn withdraw_bond(
        &mut self,
        client: &mut CompactTxStreamerClient<Channel>,
        bond_key: [u8; 32],
        account_id: zip32::AccountId,
    ) -> Result<TxId, WithdrawError> {
        let network = self.network;
        let create_action = self
            .bond_create_action(bond_key)
            .ok_or(WithdrawError::UnknownBond(bond_key))?;
        let (target_finalizer, created_amount, bond_salt) = create_action
            .bond_terms()
            .ok_or(WithdrawError::UnknownBond(bond_key))?;

        let usk: zcash_keys::keys::UnifiedSpendingKey = self
            .unified_key_store
            .get(&account_id)
            .ok_or(KeyError::NoAccountKeys)?
            .try_into()?;
        let bond_signing_key = usk
            .bond()
            .recover_signing_key(&target_finalizer, created_amount, &bond_salt, bond_key)
            .ok_or(WithdrawError::BondKeyNotRecoverable(bond_key))?;

        // The amount must equal the bond's current balance, which rewards have grown.
        let bond_value: u64 = client
            .get_bond_info(BondInfoRequest {
                bond_key: bond_key.to_vec(),
            })
            .await
            .map_err(|_| WithdrawError::BondInfoUnavailable(bond_key))?
            .into_inner()
            .amount;

        let orchard_fvk: orchard::keys::FullViewingKey = usk.orchard().into();
        let orchard_ovk = orchard_fvk.to_ovk(zip32::Scope::Internal);
        let my_receiver = *self
            .unified_addresses
            .iter()
            .find(|(id, _)| id.account_id == account_id)
            .and_then(|(_, address)| address.orchard())
            .ok_or(WithdrawError::NoSyncData)?;

        let (target_height, anchor_height) = self
            .get_target_and_anchor_heights(self.wallet_settings.min_confirmations)
            .map_err(|_| WithdrawError::NoSyncData)?
            .ok_or(WithdrawError::NoSyncData)?;

        let ironwood_tree = &self.shard_trees.ironwood;
        let ironwood_anchor = match ironwood_tree
            .root_at_checkpoint_id(&anchor_height)
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
                orchard_anchor: None,
                ironwood_anchor: Some(ironwood_anchor),
                orchard_padding: BundlePadding::DEFAULT,
                ironwood_padding: BundlePadding::DEFAULT,
            },
        );

        // Notes to cover the fee only; the bond value arrives from the staking pool.
        let mut fee_inputs_sum: u64 = 0;
        let mut spend_count: usize = 0;
        let mut fee_est: u64 = 10_000;

        let spendable_notes: Vec<&IronwoodNote> = self
            .spendable_notes(anchor_height, &[], account_id, false)
            .map_err(|_| WithdrawError::NoSyncData)?;

        for note in spendable_notes.iter() {
            let Some(position) = note.position() else {
                continue;
            };
            let witness = match ironwood_tree.witness_at_checkpoint_id(position, &anchor_height) {
                Ok(Some(w)) => w,
                _ => continue,
            };
            let merkle_path = orchard::tree::MerklePath::from(witness);

            txb.add_ironwood_spend::<FeeError>(orchard_fvk.clone(), *note.note(), merkle_path)
                .map_err(|e| WithdrawError::AddIronwoodSpendFailed(format!("{e:?}")))?;
            fee_inputs_sum += note.note().value().inner();
            spend_count += 1;

            let ironwood_actions = spend_count + 1;
            fee_est = (5000 * (ironwood_actions as u64).max(2)).max(10_000);

            if fee_inputs_sum >= fee_est {
                break;
            }
        }

        if fee_inputs_sum < fee_est {
            return Err(WithdrawError::NotEnoughFundsForFee);
        }

        txb.put_staking_action(StakingAction::WithdrawDelegationBond {
            amount_zats: bond_value,
            unique_pubkey: bond_key,
            signature: [0u8; 64],
        })
        .map_err(|_| WithdrawError::PutStakingActionFailed)?;

        let out_value = Zatoshis::from_u64(bond_value + (fee_inputs_sum - fee_est))
            .map_err(|_| WithdrawError::BuildError)?;
        txb.add_ironwood_output::<FeeError>(
            Some(orchard_ovk),
            my_receiver,
            out_value,
            MemoBytes::from_bytes(&EMPTY_MEMO_BYTES).unwrap(),
        )
        .map_err(|_| WithdrawError::AddOrchardOutputFailed)?;

        let params_dir = std::env::temp_dir().join("zingo-params");
        let (spend, output) = ensure_sapling_params_on_disk(params_dir)
            .expect("could not materialize embedded zcash params");
        let prover = LocalTxProver::new(&spend, &output);

        let orchard_ask: orchard::keys::SpendAuthorizingKey = usk.orchard().into();
        let tx_res = txb
            .build(
                &TransparentSigningSet::new(),
                &[],
                &[orchard_ask],
                Some(bond_signing_key),
                OsRng,
                &prover,
                &prover,
                &zcash_primitives::transaction::fees::zip317::FeeRule::standard(),
            )
            .map_err(|e| {
                tracing::error!("withdraw_bond: build failed: {e:?}");
                WithdrawError::BuildError
            })?;

        let tx = tx_res.transaction();
        let mut tx_bytes = vec![];
        tx.write(&mut tx_bytes)
            .map_err(|_| WithdrawError::TxWrite)?;

        client
            .send_transaction(RawTransaction {
                data: tx_bytes.clone(),
                height: 0,
            })
            .await
            .map_err(|_| WithdrawError::TxSend)?;

        // Record the pending withdrawal so the wallet tracks its status and the bond's state.
        let transaction = zcash_primitives::transaction::Transaction::read(
            tx_bytes.as_slice(),
            consensus::BranchId::for_height(&network, target_height.into()),
        )
        .map_err(|e| WithdrawError::RecordFailed(e.to_string()))?;
        let datetime = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as u32);
        let ufvks = SyncWallet::get_unified_full_viewing_keys(self)
            .map_err(|e| WithdrawError::RecordFailed(e.to_string()))?;
        pepper_sync::scan_pending_transaction(
            &network,
            &ufvks,
            self,
            transaction,
            ConfirmationStatus::Calculated(target_height.into()),
            datetime,
        )
        .map_err(|e| WithdrawError::RecordFailed(e.to_string()))?;

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
    use zcash_protocol::{PoolType, ShieldedPool};

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

        let pool = PoolType::Shielded(ShieldedPool::Orchard);
        let self_address = wallet.get_address(pool);

        let receivers = vec![(self_address.as_str(), 100_000, None)];
        let request = transaction_request_from_send_inputs(receivers)
            .expect("actually all of this logic oughta be internal to propose");

        wallet
            .create_send_proposal(request, zip32::AccountId::ZERO)
            .expect("can propose from existing data");
    }
}
