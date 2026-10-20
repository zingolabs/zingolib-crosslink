use std::{collections::HashMap, convert::Infallible, num::NonZeroU32};

use secrecy::SecretVec;
use shardtree::{ShardTree, error::ShardTreeError, store::ShardStore};
use tracing::instrument;
use zcash_address::ZcashAddress;
use zcash_client_backend::{
    data_api::{
        Account, AccountBirthday, AccountPurpose, BlockMetadata, CoinbaseFilter, InputSource,
        NullifierQuery, ORCHARD_SHARD_HEIGHT, ReceivedNotes, SAPLING_SHARD_HEIGHT, TargetValue,
        TransactionDataRequest, TransparentBalances, WalletCommitmentTrees, WalletRead,
        WalletSummary, WalletWrite, Zip32Derivation,
        chain::CommitmentTreeRoot,
        locking::{LockError, LockFilter, LockOwner, OutputLockStore},
        wallet::{ConfirmationsPolicy, TargetHeight},
    },
    wallet::{Exposure, NoteId, ReceivedNote, TransparentAddressMetadata, WalletTransparentOutput},
};
use zcash_keys::{address::UnifiedAddress, keys::UnifiedFullViewingKey};
use zcash_primitives::{
    block::BlockHash,
    transaction::{Transaction, TxId},
};
use zcash_protocol::{
    PoolType, ShieldedPool,
    consensus::{self, BlockHeight, Parameters},
    memo::Memo,
};
use zcash_transparent::address::TransparentAddress;
use zcash_transparent::bundle::{OutPoint, TxOut};
use zcash_transparent::keys::TransparentKeyScope;

use super::{LightWallet, error::WalletError, output::OutputRef};
use crate::wallet::output::RemainingNeeded;
use pepper_sync::{
    error::SyncError,
    keys::transparent::{self, TransparentScope},
    wallet::{
        IronwoodNote, KeyIdInterface, NoteInterface, OrchardNote, OrchardShardStore, OutputId,
        OutputInterface, SaplingNote, SaplingShardStore, traits::SyncWallet,
    },
};
use zingo_status::confirmation_status::ConfirmationStatus;

pub struct ZingoAccount(zip32::AccountId, UnifiedFullViewingKey, BlockHeight);

impl Account for ZingoAccount {
    type AccountId = zip32::AccountId;

    fn id(&self) -> Self::AccountId {
        self.0
    }

    fn name(&self) -> Option<&str> {
        None
    }

    fn birthday_height(&self) -> BlockHeight {
        self.2
    }

    fn source(&self) -> &zcash_client_backend::data_api::AccountSource {
        unimplemented!()
    }

    fn ufvk(&self) -> Option<&UnifiedFullViewingKey> {
        Some(&self.1)
    }

    fn uivk(&self) -> zcash_keys::keys::UnifiedIncomingViewingKey {
        unimplemented!()
    }
}

impl WalletRead for LightWallet {
    type Error = WalletError;
    type AccountId = zip32::AccountId;
    type Account = ZingoAccount;

    fn get_account_ids(&self) -> Result<Vec<Self::AccountId>, Self::Error> {
        Ok(self.unified_key_store.keys().copied().collect())
    }

    fn get_account(
        &self,
        _account_id: Self::AccountId,
    ) -> Result<Option<Self::Account>, Self::Error> {
        unimplemented!()
    }

    fn get_derived_account(
        &self,
        _account_id: &Zip32Derivation,
    ) -> Result<Option<Self::Account>, Self::Error> {
        unimplemented!()
    }

    fn validate_seed(
        &self,
        _account_id: Self::AccountId,
        _seed: &secrecy::SecretVec<u8>,
    ) -> Result<bool, Self::Error> {
        unimplemented!()
    }

    fn seed_relevance_to_derived_accounts(
        &self,
        _seed: &secrecy::SecretVec<u8>,
    ) -> Result<zcash_client_backend::data_api::SeedRelevance<Self::AccountId>, Self::Error> {
        unimplemented!()
    }

    fn get_account_for_ufvk(
        &self,
        ufvk: &UnifiedFullViewingKey,
    ) -> Result<Option<Self::Account>, Self::Error> {
        let Some((account_id, unified_key)) =
            self.unified_key_store.iter().find(|(_, unified_key)| {
                UnifiedFullViewingKey::try_from(*unified_key).is_ok_and(|account_ufvk| {
                    account_ufvk.encode(&self.network) == *ufvk.encode(&self.network)
                })
            })
        else {
            return Ok(None);
        };

        Ok(Some(ZingoAccount(
            *account_id,
            unified_key.try_into()?,
            self.birthday,
        )))
    }

    fn list_addresses(
        &self,
        _account: Self::AccountId,
    ) -> Result<Vec<zcash_client_backend::data_api::AddressInfo>, Self::Error> {
        unimplemented!()
    }

    fn find_account_for_address<P: consensus::Parameters>(
        &self,
        _params: &P,
        _address: &zcash_keys::address::Address,
    ) -> Result<
        Option<Self::AccountId>,
        zcash_client_backend::data_api::error::FindAccountForAddressError<Self::Error>,
    > {
        unimplemented!()
    }

    fn get_last_generated_address_matching(
        &self,
        _account: Self::AccountId,
        _address_filter: zcash_keys::keys::UnifiedAddressRequest,
    ) -> Result<Option<UnifiedAddress>, Self::Error> {
        unimplemented!()
    }

    fn get_account_birthday(&self, _account: Self::AccountId) -> Result<BlockHeight, Self::Error> {
        unimplemented!()
    }

    fn get_wallet_birthday(&self) -> Result<Option<BlockHeight>, Self::Error> {
        Ok(Some(self.birthday))
    }

    fn get_wallet_recover_until(&self) -> Result<Option<BlockHeight>, Self::Error> {
        Ok(None)
    }

    fn get_wallet_summary(
        &self,
        _min_confirmations: ConfirmationsPolicy,
    ) -> Result<Option<WalletSummary<Self::AccountId>>, Self::Error> {
        unimplemented!()
    }

    fn chain_height(&self) -> Result<Option<BlockHeight>, Self::Error> {
        Ok(self.sync_state.last_known_chain_height())
    }

    fn get_block_hash(&self, _block_height: BlockHeight) -> Result<Option<BlockHash>, Self::Error> {
        unimplemented!()
    }

    fn block_metadata(&self, _height: BlockHeight) -> Result<Option<BlockMetadata>, Self::Error> {
        unimplemented!()
    }

    fn block_fully_scanned(&self) -> Result<Option<BlockMetadata>, Self::Error> {
        unimplemented!()
    }

    fn get_max_height_hash(&self) -> Result<Option<(BlockHeight, BlockHash)>, Self::Error> {
        unimplemented!()
    }

    fn block_max_scanned(&self) -> Result<Option<BlockMetadata>, Self::Error> {
        unimplemented!()
    }

    fn suggest_scan_ranges(
        &self,
    ) -> Result<Vec<zcash_client_backend::data_api::scanning::ScanRange>, Self::Error> {
        unimplemented!()
    }

    fn get_target_and_anchor_heights(
        &self,
        min_confirmations: NonZeroU32,
    ) -> Result<Option<(TargetHeight, BlockHeight)>, Self::Error> {
        let target_height = if let Some(height) = self.sync_state.last_known_chain_height() {
            height + 1
        } else {
            return Ok(None);
        };

        let max_checkpoint_height = self
            .shard_trees
            .sapling
            .store()
            .max_checkpoint_id()
            .expect("infallible")
            .expect("should be at least 1 checkpoint");

        let anchor_height = std::cmp::min(
            max_checkpoint_height,
            target_height - min_confirmations.get(),
        );

        Ok(Some((
            target_height.into(),
            std::cmp::max(1.into(), anchor_height),
        )))
    }

    fn get_tx_height(&self, txid: TxId) -> Result<Option<BlockHeight>, Self::Error> {
        Ok(self
            .wallet_transactions
            .get(&txid)
            .and_then(|transaction| transaction.status().get_confirmed_height()))
    }

    fn get_unified_full_viewing_keys(
        &self,
    ) -> Result<HashMap<Self::AccountId, UnifiedFullViewingKey>, Self::Error> {
        let mut out = HashMap::new();

        for (account_id, uks) in &self.unified_key_store {
            match UnifiedFullViewingKey::try_from(uks) {
                Ok(ufvk) => {
                    out.insert(*account_id, ufvk);
                }
                Err(_e) => {
                    continue;
                }
            }
        }

        Ok(out)
    }

    fn get_memo(&self, _note_id: NoteId) -> Result<Option<Memo>, Self::Error> {
        unimplemented!()
    }

    fn get_transaction(&self, _txid: TxId) -> Result<Option<Transaction>, Self::Error> {
        unimplemented!()
    }

    /// The action that created a bond this wallet knows about, found in the wallet's own
    /// transaction history. The backend derives the bond's signing key from the terms it carries,
    /// so a later action (unbond, retarget, withdraw) on the bond can be signed.
    fn get_bond_create_action(
        &self,
        bond_key: [u8; 32],
    ) -> Result<Option<zcash_primitives::transaction::StakingAction>, Self::Error> {
        Ok(self.bond_create_action(bond_key))
    }

    fn get_sapling_nullifiers(
        &self,
        _query: NullifierQuery,
    ) -> Result<Vec<(Self::AccountId, sapling_crypto::Nullifier)>, Self::Error> {
        unimplemented!()
    }

    fn get_orchard_nullifiers(
        &self,
        _query: NullifierQuery,
    ) -> Result<Vec<(Self::AccountId, orchard::note::Nullifier)>, Self::Error> {
        unimplemented!()
    }

    fn get_ironwood_nullifiers(
        &self,
        _query: NullifierQuery,
    ) -> Result<Vec<(Self::AccountId, orchard::note::Nullifier)>, Self::Error> {
        unimplemented!()
    }

    fn get_transparent_receivers(
        &self,
        account: Self::AccountId,
        // TODO: only get internal receivers if true
        _include_change: bool,
        _include_standalone_receivers: bool,
    ) -> Result<HashMap<TransparentAddress, TransparentAddressMetadata>, Self::Error> {
        self.transparent_addresses
            .iter()
            .filter(|(address_id, _)| {
                address_id.account_id() == account && address_id.scope() != TransparentScope::Refund
            })
            .filter_map(|(address_id, encoded_address)| {
                let address = ZcashAddress::try_from_encoded(encoded_address)
                    .ok()?
                    .convert_if_network::<TransparentAddress>(self.network.network_type())
                    .expect("incorrect network should be checked on wallet load");
                let address_metadata = TransparentAddressMetadata::derived(
                    address_id.scope().into(),
                    address_id.address_index(),
                    Exposure::CannotKnow, // TODO: add exposure to wallet transparent address metadata
                    None,
                );

                Some(Ok((address, address_metadata)))
            })
            .collect()
    }

    fn get_ephemeral_transparent_receivers(
        &self,
        account: Self::AccountId,
        _exposure_depth: u32,
        _exclude_used: bool,
    ) -> Result<HashMap<TransparentAddress, TransparentAddressMetadata>, Self::Error> {
        self.transparent_addresses
            .iter()
            .filter(|(address_id, _)| {
                address_id.account_id() == account && address_id.scope() == TransparentScope::Refund
            })
            .map(|(address_id, encoded_address)| {
                let address = ZcashAddress::try_from_encoded(encoded_address)?
                    .convert_if_network::<TransparentAddress>(self.network.network_type())
                    .expect("incorrect network should be checked on wallet load");
                let address_metadata = TransparentAddressMetadata::derived(
                    address_id.scope().into(),
                    address_id.address_index(),
                    Exposure::CannotKnow, // TODO: add exposure to wallet transparent address metadata
                    None,
                );

                Ok((address, address_metadata))
            })
            .collect()
    }

    #[instrument(
        name = "get_transparent_address_metadata",
        skip(self),
        err,
        ret,
        level = "info"
    )]
    fn get_transparent_address_metadata(
        &self,
        account: Self::AccountId,
        address: &TransparentAddress,
    ) -> Result<Option<TransparentAddressMetadata>, Self::Error> {
        // For now we only look at the "normal" transparent receivers.
        // This is slightly weaker than the doc's "equivalent" implementation
        // (it also checks ephemeral addresses), but it's enough to avoid the panic
        // and should work fine as long as QuickShield doesn't rely on refund
        // addresses' metadata.
        let receivers = self.get_transparent_receivers(account, true, true)?;

        Ok(receivers.get(address).cloned())
    }

    fn get_transparent_balances(
        &self,
        _account: Self::AccountId,
        _max_height: TargetHeight,
        _confirmations_policy: ConfirmationsPolicy,
    ) -> Result<TransparentBalances, Self::Error> {
        unimplemented!()
    }

    fn utxo_query_height(
        &self,
        _account: Self::AccountId,
    ) -> Result<zcash_protocol::consensus::BlockHeight, Self::Error> {
        unimplemented!()
    }

    fn transaction_data_requests(&self) -> Result<Vec<TransactionDataRequest>, Self::Error> {
        unimplemented!()
    }

    fn get_received_outputs(
        &self,
        _txid: TxId,
        _target_height: TargetHeight,
        _confirmations_policy: ConfirmationsPolicy,
    ) -> Result<Vec<zcash_client_backend::data_api::ReceivedTransactionOutput>, Self::Error> {
        unimplemented!()
    }
}

/// Zingolib keeps no persisted output locks: proposals are confirmed and sent within one session,
/// and the wallet is the only writer. Locking is therefore a no-op that reports success, and
/// every [`LockFilter`] passed to [`InputSource`] is ignored.
impl OutputLockStore for LightWallet {
    type Error = WalletError;
    type AccountId = zip32::AccountId;

    fn lock_outputs(
        &mut self,
        outputs: &[zcash_client_backend::wallet::OutputRef],
        _owner: LockOwner,
        _lock_expiry_height: BlockHeight,
    ) -> Result<usize, LockError<Self::Error>> {
        Ok(outputs.len())
    }

    fn unlock_output(
        &mut self,
        _output: &zcash_client_backend::wallet::OutputRef,
        _owner: LockOwner,
    ) -> Result<bool, Self::Error> {
        Ok(false)
    }

    fn clear_locked_outputs(&mut self, _account: Self::AccountId) -> Result<usize, Self::Error> {
        Ok(0)
    }

    fn get_locked_outputs(
        &self,
        _account: Self::AccountId,
    ) -> Result<Vec<zcash_client_backend::wallet::OutputRef>, Self::Error> {
        Ok(Vec::new())
    }
}

impl WalletWrite for LightWallet {
    type UtxoRef = u32;

    fn create_account(
        &mut self,
        _account_name: &str,
        _seed: &SecretVec<u8>,
        _birthday: &AccountBirthday,
        _key_source: Option<&str>,
    ) -> Result<
        (
            <Self as WalletRead>::AccountId,
            zcash_keys::keys::UnifiedSpendingKey,
        ),
        <Self as WalletRead>::Error,
    > {
        unimplemented!()
    }

    fn import_account_hd(
        &mut self,
        _account_name: &str,
        _seed: &SecretVec<u8>,
        _account_index: zip32::AccountId,
        _birthday: &AccountBirthday,
        _key_source: Option<&str>,
    ) -> Result<(Self::Account, zcash_keys::keys::UnifiedSpendingKey), <Self as WalletRead>::Error>
    {
        unimplemented!()
    }

    fn import_account_ufvk(
        &mut self,
        _account_name: &str,
        _unified_key: &UnifiedFullViewingKey,
        _birthday: &AccountBirthday,
        _purpose: AccountPurpose,
        _key_source: Option<&str>,
    ) -> Result<Self::Account, <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn delete_account(
        &mut self,
        _account: <Self as WalletRead>::AccountId,
    ) -> Result<(), <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn get_next_available_address(
        &mut self,
        _account: <Self as WalletRead>::AccountId,
        _request: zcash_keys::keys::UnifiedAddressRequest,
    ) -> Result<Option<(UnifiedAddress, zip32::DiversifierIndex)>, <Self as WalletRead>::Error>
    {
        unimplemented!()
    }

    fn get_address_for_index(
        &mut self,
        _account: <Self as WalletRead>::AccountId,
        _diversifier_index: zip32::DiversifierIndex,
        _request: zcash_keys::keys::UnifiedAddressRequest,
    ) -> Result<Option<UnifiedAddress>, <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn update_chain_tip(
        &mut self,
        _tip_height: BlockHeight,
    ) -> Result<(), <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn prune_scan_queue_below(
        &mut self,
        _height: BlockHeight,
        _retain_with_priority: Option<zcash_client_backend::data_api::scanning::ScanPriority>,
    ) -> Result<u64, <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn put_blocks(
        &mut self,
        _from_state: &zcash_client_backend::data_api::chain::ChainState,
        _blocks: Vec<zcash_client_backend::data_api::ScannedBlock<<Self as WalletRead>::AccountId>>,
    ) -> Result<(), <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn put_received_transparent_utxo(
        &mut self,
        _output: &WalletTransparentOutput<<Self as WalletRead>::AccountId>,
    ) -> Result<Self::UtxoRef, <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn store_decrypted_tx(
        &mut self,
        _received_tx: zcash_client_backend::data_api::DecryptedTransaction<
            Transaction,
            <Self as WalletRead>::AccountId,
        >,
    ) -> Result<(), <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn set_tx_trust(
        &mut self,
        _txid: TxId,
        _trusted: bool,
    ) -> Result<(), <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn store_transactions_to_be_sent(
        &mut self,
        transactions: &[zcash_client_backend::data_api::SentTransaction<
            <Self as WalletRead>::AccountId,
        >],
    ) -> Result<(), <Self as WalletRead>::Error> {
        let network = self.network;

        for sent_transaction in transactions {
            // this is a workaround as Transaction does not implement Clone
            let mut transaction_bytes = vec![];
            sent_transaction
                .tx()
                .write(&mut transaction_bytes)
                .map_err(WalletError::TransactionWrite)?;
            let transaction = Transaction::read(
                transaction_bytes.as_slice(),
                consensus::BranchId::for_height(
                    &self.network,
                    sent_transaction.target_height().into(),
                ),
            )
            .map_err(WalletError::TransactionRead)?;

            match pepper_sync::scan_pending_transaction(
                &network,
                &SyncWallet::get_unified_full_viewing_keys(self)?,
                self,
                transaction,
                ConfirmationStatus::Calculated(sent_transaction.target_height().into()),
                sent_transaction.created().unix_timestamp() as u32,
            ) {
                Ok(()) => (),
                Err(SyncError::ScanError(e)) => return Err(e.into()),
                Err(SyncError::WalletError(e)) => return Err(e),
                Err(_) => {
                    panic!("`scan_pending_transactions` should only return scan or wallet errors")
                }
            }
        }

        Ok(())
    }

    fn truncate_to_height(
        &mut self,
        _max_height: BlockHeight,
    ) -> Result<BlockHeight, <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn truncate_to_chain_state(
        &mut self,
        _chain_state: zcash_client_backend::data_api::chain::ChainState,
    ) -> Result<(), <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn rewind_to_chain_state(
        &mut self,
        _chain_state: zcash_client_backend::data_api::chain::ChainState,
        _reset_account_birthdays: std::collections::HashSet<<Self as WalletRead>::AccountId>,
    ) -> Result<
        (),
        zcash_client_backend::data_api::error::RewindError<
            <Self as WalletRead>::AccountId,
            <Self as WalletRead>::Error,
        >,
    > {
        unimplemented!()
    }

    fn reserve_next_n_ephemeral_addresses(
        &mut self,
        account_id: <Self as WalletRead>::AccountId,
        n: usize,
    ) -> Result<Vec<(TransparentAddress, TransparentAddressMetadata)>, <Self as WalletRead>::Error>
    {
        Ok(self
            .generate_refund_addresses(n, account_id)?
            .into_iter()
            .map(|(address_id, address)| {
                (
                    address,
                    TransparentAddressMetadata::derived(
                        TransparentKeyScope::EPHEMERAL,
                        address_id.address_index(),
                        Exposure::CannotKnow, // TODO: add exposure to wallet transparent address metadata
                        None,
                    ),
                )
            })
            .collect())
    }

    fn set_transaction_status(
        &mut self,
        _txid: TxId,
        _status: zcash_client_backend::data_api::TransactionStatus,
    ) -> Result<(), <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn notify_address_checked(
        &mut self,
        _request: zcash_client_backend::data_api::TransactionsInvolvingAddress,
        _as_of_height: BlockHeight,
    ) -> Result<(), <Self as WalletRead>::Error> {
        unimplemented!()
    }
}

impl WalletCommitmentTrees for LightWallet {
    type Error = Infallible;
    type SaplingShardStore<'a> = SaplingShardStore;
    type OrchardShardStore<'a> = OrchardShardStore;

    fn with_sapling_tree_mut<F, A, E>(&mut self, mut callback: F) -> Result<A, E>
    where
        for<'a> F: FnMut(
            &'a mut ShardTree<
                Self::SaplingShardStore<'a>,
                { sapling_crypto::NOTE_COMMITMENT_TREE_DEPTH },
                { SAPLING_SHARD_HEIGHT },
            >,
        ) -> Result<A, E>,
        E: From<ShardTreeError<Self::Error>>,
    {
        callback(&mut self.shard_trees.sapling)
    }

    fn put_sapling_subtree_roots(
        &mut self,
        start_index: u64,
        roots: &[CommitmentTreeRoot<sapling_crypto::Node>],
    ) -> Result<(), ShardTreeError<Self::Error>> {
        self.with_sapling_tree_mut(|t| {
            for (root, i) in roots.iter().zip(0u64..) {
                let root_addr = incrementalmerkletree::Address::from_parts(
                    SAPLING_SHARD_HEIGHT.into(),
                    start_index + i,
                );
                t.insert(root_addr, *root.root_hash())?;
            }
            Ok::<_, ShardTreeError<Self::Error>>(())
        })?;

        Ok(())
    }

    fn get_sapling_subtree_root(
        &mut self,
        index: u64,
    ) -> Result<Option<sapling_crypto::Node>, ShardTreeError<Self::Error>> {
        Ok(subtree_root(
            self.shard_trees.sapling.store(),
            SAPLING_SHARD_HEIGHT,
            index,
        ))
    }

    fn with_orchard_tree_mut<F, A, E>(&mut self, mut callback: F) -> Result<A, E>
    where
        for<'a> F: FnMut(
            &'a mut ShardTree<
                Self::OrchardShardStore<'a>,
                { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 },
                { ORCHARD_SHARD_HEIGHT },
            >,
        ) -> Result<A, E>,
        E: From<ShardTreeError<Self::Error>>,
    {
        callback(&mut self.shard_trees.orchard)
    }

    fn put_orchard_subtree_roots(
        &mut self,
        start_index: u64,
        roots: &[CommitmentTreeRoot<orchard::tree::MerkleHashOrchard>],
    ) -> Result<(), ShardTreeError<Self::Error>> {
        self.with_orchard_tree_mut(|t| {
            for (root, i) in roots.iter().zip(0u64..) {
                let root_addr = incrementalmerkletree::Address::from_parts(
                    ORCHARD_SHARD_HEIGHT.into(),
                    start_index + i,
                );
                t.insert(root_addr, *root.root_hash())?;
            }
            Ok::<_, ShardTreeError<Self::Error>>(())
        })?;

        Ok(())
    }

    fn get_orchard_subtree_root(
        &mut self,
        index: u64,
    ) -> Result<Option<orchard::tree::MerkleHashOrchard>, ShardTreeError<Self::Error>> {
        Ok(subtree_root(
            self.shard_trees.orchard.store(),
            ORCHARD_SHARD_HEIGHT,
            index,
        ))
    }

    /// The Ironwood pool has its own note commitment tree, kept by the sync engine next to the
    /// Orchard tree. Returning `Some` here is what lets the backend spend Ironwood notes.
    fn with_ironwood_tree_mut<F, A, E>(&mut self, mut callback: F) -> Result<Option<A>, E>
    where
        for<'a> F: FnMut(
            &'a mut ShardTree<
                Self::OrchardShardStore<'a>,
                { ORCHARD_SHARD_HEIGHT * 2 },
                ORCHARD_SHARD_HEIGHT,
            >,
        ) -> Result<A, E>,
        E: From<ShardTreeError<Self::Error>>,
    {
        callback(&mut self.shard_trees.ironwood).map(Some)
    }

    fn put_ironwood_subtree_roots(
        &mut self,
        start_index: u64,
        roots: &[CommitmentTreeRoot<orchard::tree::MerkleHashOrchard>],
    ) -> Result<(), ShardTreeError<Self::Error>> {
        self.with_ironwood_tree_mut(|t| {
            for (root, i) in roots.iter().zip(0u64..) {
                let root_addr = incrementalmerkletree::Address::from_parts(
                    ORCHARD_SHARD_HEIGHT.into(),
                    start_index + i,
                );
                t.insert(root_addr, *root.root_hash())?;
            }
            Ok::<_, ShardTreeError<Self::Error>>(())
        })?;

        Ok(())
    }

    fn get_ironwood_subtree_root(
        &mut self,
        index: u64,
    ) -> Result<Option<orchard::tree::MerkleHashOrchard>, ShardTreeError<Self::Error>> {
        Ok(subtree_root(
            self.shard_trees.ironwood.store(),
            ORCHARD_SHARD_HEIGHT,
            index,
        ))
    }
}

/// The root hash of the complete shard at `index`, if the store holds it.
fn subtree_root<H: incrementalmerkletree::Hashable + Clone + PartialEq, C: Clone + Ord>(
    store: &shardtree::store::memory::MemoryShardStore<H, C>,
    shard_height: u8,
    index: u64,
) -> Option<H> {
    store
        .get_shard(incrementalmerkletree::Address::from_parts(
            shard_height.into(),
            index,
        ))
        .expect("infallible")
        .and_then(|shard| shard.root().node_value().cloned())
}

impl InputSource for LightWallet {
    type Error = WalletError;
    type AccountId = zip32::AccountId;
    type NoteRef = OutputRef;

    fn get_spendable_note(
        &self,
        _txid: &TxId,
        _protocol: ShieldedPool,
        _index: u32,
        _target_height: TargetHeight,
        _lock_filter: LockFilter<'_>,
    ) -> Result<
        Option<
            zcash_client_backend::wallet::ReceivedNote<
                Self::NoteRef,
                zcash_client_backend::wallet::Note,
            >,
        >,
        Self::Error,
    > {
        unimplemented!()
    }

    /// A pool's anchor at `height` can be computed when the pool's tree holds a checkpoint there.
    fn anchor_computable(
        &self,
        protocol: ShieldedPool,
        height: BlockHeight,
    ) -> Result<bool, Self::Error> {
        let has_checkpoint = match protocol {
            ShieldedPool::Sapling => self
                .shard_trees
                .sapling
                .store()
                .get_checkpoint(&height)
                .expect("infallible")
                .is_some(),
            ShieldedPool::Orchard => self
                .shard_trees
                .orchard
                .store()
                .get_checkpoint(&height)
                .expect("infallible")
                .is_some(),
            ShieldedPool::Ironwood => self
                .shard_trees
                .ironwood
                .store()
                .get_checkpoint(&height)
                .expect("infallible")
                .is_some(),
        };
        Ok(has_checkpoint)
    }

    fn select_spendable_notes(
        &self,
        account: Self::AccountId,
        target_value: TargetValue,
        sources: &[ShieldedPool],
        _target_height: TargetHeight,
        confirmations_policy: ConfirmationsPolicy,
        exclude: &[Self::NoteRef],
        _lock_filter: LockFilter<'_>,
    ) -> Result<ReceivedNotes<Self::NoteRef>, Self::Error> {
        let (_, anchor_height) = self
            .get_target_and_anchor_heights(confirmations_policy.trusted())
            .expect("infallible")
            .ok_or(WalletError::NoSyncData)?;

        let mut exclude_sapling = exclude
            .iter()
            .filter(|&note_id| note_id.pool_type() == PoolType::SAPLING)
            .map(|note_id| OutputId::new(note_id.txid(), note_id.output_index()))
            .collect::<Vec<_>>();
        let mut exclude_orchard = exclude
            .iter()
            .filter(|&note_id| note_id.pool_type() == PoolType::ORCHARD)
            .map(|note_id| OutputId::new(note_id.txid(), note_id.output_index()))
            .collect::<Vec<_>>();
        let mut exclude_ironwood = exclude
            .iter()
            .filter(|&note_id| note_id.pool_type() == PoolType::IRONWOOD)
            .map(|note_id| OutputId::new(note_id.txid(), note_id.output_index()))
            .collect::<Vec<_>>();

        let (selected_sapling_notes, selected_orchard_notes, selected_ironwood_notes) =
            match target_value {
                TargetValue::AtLeast(at_least_value) => {
                    let mut remaining_value_needed = RemainingNeeded::Positive(at_least_value);

                    // prioritises selecting spendable notes that are guaranteed to be unspent first
                    let mut selected_sapling_notes = Vec::new();
                    let mut selected_orchard_notes = Vec::new();
                    let mut selected_ironwood_notes = Vec::new();
                    for include_potentially_spent_notes in [false, true] {
                        // prioritise note selection for the given `sources`
                        if sources.contains(&ShieldedPool::Sapling) {
                            let notes = self
                                .select_spendable_notes_by_pool::<SaplingNote>(
                                    &mut remaining_value_needed,
                                    anchor_height,
                                    &exclude_sapling,
                                    account,
                                    include_potentially_spent_notes,
                                )?
                                .into_iter()
                                .cloned()
                                .collect::<Vec<_>>();
                            exclude_sapling.extend(notes.iter().map(OutputInterface::output_id));
                            selected_sapling_notes.extend(notes);
                        }
                        if sources.contains(&ShieldedPool::Orchard) {
                            let notes = self
                                .select_spendable_notes_by_pool::<OrchardNote>(
                                    &mut remaining_value_needed,
                                    anchor_height,
                                    &exclude_orchard,
                                    account,
                                    include_potentially_spent_notes,
                                )?
                                .into_iter()
                                .cloned()
                                .collect::<Vec<_>>();
                            exclude_orchard.extend(notes.iter().map(OutputInterface::output_id));
                            selected_orchard_notes.extend(notes);
                        }
                        if sources.contains(&ShieldedPool::Ironwood) {
                            let notes = self
                                .select_spendable_notes_by_pool::<IronwoodNote>(
                                    &mut remaining_value_needed,
                                    anchor_height,
                                    &exclude_ironwood,
                                    account,
                                    include_potentially_spent_notes,
                                )?
                                .into_iter()
                                .cloned()
                                .collect::<Vec<_>>();
                            exclude_ironwood.extend(notes.iter().map(OutputInterface::output_id));
                            selected_ironwood_notes.extend(notes);
                        }
                    }
                    (
                        selected_sapling_notes,
                        selected_orchard_notes,
                        selected_ironwood_notes,
                    )
                }
                TargetValue::AllFunds(max_spend_mode) => {
                    // FIXME: this is not the criteria for `MaxSpendMode::Everything`. this should return an error if sync is not complete in this case.
                    let include_potentially_spent_notes = matches!(
                        max_spend_mode,
                        zcash_client_backend::data_api::MaxSpendMode::Everything
                    );
                    (
                        // FIXME: note filters implemented in `spendable_notes_by_pool` have been missed here such as filtering dust
                        self.spendable_notes::<SaplingNote>(
                            anchor_height,
                            &exclude_sapling,
                            account,
                            include_potentially_spent_notes,
                        )?
                        .into_iter()
                        .cloned()
                        .collect::<Vec<_>>(),
                        self.spendable_notes::<OrchardNote>(
                            anchor_height,
                            &exclude_orchard,
                            account,
                            include_potentially_spent_notes,
                        )?
                        .into_iter()
                        .filter(|_| sources.contains(&ShieldedPool::Orchard))
                        .cloned()
                        .collect::<Vec<_>>(),
                        self.spendable_notes::<IronwoodNote>(
                            anchor_height,
                            &exclude_ironwood,
                            account,
                            include_potentially_spent_notes,
                        )?
                        .into_iter()
                        .filter(|_| sources.contains(&ShieldedPool::Ironwood))
                        .cloned()
                        .collect::<Vec<_>>(),
                    )
                }
            };

        /* TODO: Priority
        if selected
            .iter()
            .filter(|n| n.0.protocol() == ShieldedPool::Sapling)
            .count()
            == 1
            || selected
                .iter()
                .filter(|n| n.0.protocol() == ShieldedPool::Orchard)
                .count()
                == 1
        {
            // since we maxed out the target value with only one note in at least one Shielded Pool
            //  we have an option to sweep a dust note into a grace input.
            // we will sweep the biggest dust note we can
            if !dust_notes.is_empty() {
                sweep_dust_into_grace(&mut selected, dust_notes);
            }
            // TODO: re-introduce this optimisation, current bug is that we don't select a note from the same pool as the single selected note
            // (and we don't have information about the pool(s) the outputs are being created for)
            // this is ok for dust as it is excluded if the dust is from a pool where grace inputs are available. however, this doesn't work for
            // non-dust
            //
            // } else {
            //     // we have no extra dust, but we can still save a marginal fee by adding the next smallest note to change
            //     if let Some(smallest_note) = unselected.pop() {
            //         selected.push(smallest_note);
            //     };
            // }
        }
        */

        let sapling_recieved_notes = selected_sapling_notes
            .iter()
            .map(|note| {
                ReceivedNote::from_parts(
                    OutputRef::new(
                        OutputId::new(note.output_id().txid(), note.output_id().output_index()),
                        PoolType::SAPLING,
                    ),
                    note.output_id().txid(),
                    note.output_id().output_index(),
                    note.note().clone(),
                    note.key_id().scope,
                    note.position()
                        .expect("note selection should filter on notes with positions"),
                    None, // mined_height. TODO: How should we use this here?
                    None, // max_shielding_input_height. TODO: How should we use this here?
                )
            })
            .collect::<Vec<_>>();
        let orchard_recieved_notes = selected_orchard_notes
            .iter()
            .map(|note| {
                ReceivedNote::from_parts(
                    OutputRef::new(
                        OutputId::new(note.output_id().txid(), note.output_id().output_index()),
                        PoolType::ORCHARD,
                    ),
                    note.output_id().txid(),
                    note.output_id().output_index(),
                    *note.note(),
                    note.key_id().scope,
                    note.position()
                        .expect("note selection should filter on notes with positions"),
                    None, // mined_height. TODO: How should we use this here?
                    None, // max_shielding_input_height. TODO: How should we use this here?
                )
            })
            .collect::<Vec<_>>();
        let ironwood_recieved_notes = selected_ironwood_notes
            .iter()
            .map(|note| {
                ReceivedNote::from_parts(
                    OutputRef::new(
                        OutputId::new(note.output_id().txid(), note.output_id().output_index()),
                        PoolType::IRONWOOD,
                    ),
                    note.output_id().txid(),
                    note.output_id().output_index(),
                    *note.note(),
                    note.key_id().scope,
                    note.position()
                        .expect("note selection should filter on notes with positions"),
                    None, // mined_height. TODO: How should we use this here?
                    None, // max_shielding_input_height. TODO: How should we use this here?
                )
            })
            .collect::<Vec<_>>();

        Ok(ReceivedNotes::new(
            sapling_recieved_notes,
            orchard_recieved_notes,
            ironwood_recieved_notes,
        ))
    }

    fn get_account_metadata(
        &self,
        _account: Self::AccountId,
        _selector: &zcash_client_backend::data_api::NoteFilter,
        _target_height: TargetHeight,
        _exclude: &[Self::NoteRef],
        _lock_filter: LockFilter<'_>,
    ) -> Result<zcash_client_backend::data_api::AccountMeta, Self::Error> {
        unimplemented!()
    }

    fn get_unspent_transparent_output(
        &self,
        _outpoint: &OutPoint,
        _target_height: TargetHeight,
    ) -> Result<Option<WalletTransparentOutput<Self::AccountId>>, Self::Error> {
        unimplemented!()
    }

    #[instrument(
        name = "get_spendable_transparent_outputs",
        skip(self),
        err,
        ret,
        level = "info"
    )]
    fn get_spendable_transparent_outputs(
        &self,
        address: &TransparentAddress,
        target_height: TargetHeight,
        confirmations_policy: ConfirmationsPolicy,
        output_filter: CoinbaseFilter,
        _lock_filter: LockFilter<'_>,
    ) -> Result<Vec<WalletTransparentOutput<Self::AccountId>>, Self::Error> {
        let address = transparent::encode_address(&self.network, *address);

        Ok(self
            .spendable_transparent_coins(
                target_height.into(),
                confirmations_policy.allow_zero_conf_shielding(),
                false,
            )
            .into_iter()
            .filter(|&output| output.address() == address)
            .filter(|&output| {
                let is_coinbase = self
                    .output_transaction(output)
                    .transaction()
                    .transparent_bundle()
                    .is_some_and(|bundle| bundle.is_coinbase());
                match output_filter {
                    CoinbaseFilter::AllTransparentOutputs => true,
                    CoinbaseFilter::CoinbaseOnly => is_coinbase,
                    CoinbaseFilter::NonCoinbaseOnly => !is_coinbase,
                }
            })
            .filter_map(|output| {
                WalletTransparentOutput::from_parts(
                    output.output_id().into(),
                    TxOut::new(
                        output.value().try_into().expect("value from checked type"),
                        output.script().clone(),
                    ),
                    Some(
                        self.output_transaction(output)
                            .status()
                            .get_confirmed_height()
                            .expect("output must be confirmed in this scope"),
                    ),
                    Some(output.key_id().account_id()),
                    Some(output.key_id().scope().into()),
                    None,
                )
            })
            .collect())
    }

    fn select_unspent_notes(
        &self,
        _account: Self::AccountId,
        _sources: &[ShieldedPool],
        _target_height: TargetHeight,
        _exclude: &[Self::NoteRef],
        _lock_filter: LockFilter<'_>,
    ) -> Result<ReceivedNotes<Self::NoteRef>, Self::Error> {
        unimplemented!()
    }
}
