//! Entrypoint for sync engine

use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{self, AtomicBool, AtomicU8};
use std::time::{Duration, SystemTime};

use tokio::sync::{RwLock, mpsc};

use incrementalmerkletree::{Marking, Retention};
use orchard::tree::MerkleHashOrchard;
use shardtree::ShardTree;
use shardtree::store::ShardStore;
use shardtree::store::memory::MemoryShardStore;
use tonic::transport::Channel;
use zcash_client_backend::proto::service::RawTransaction;
use zcash_client_backend::proto::service::compact_tx_streamer_client::CompactTxStreamerClient;
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::transaction::{Transaction, TxId};
use zcash_protocol::ShieldedPool;
use zcash_protocol::consensus::{self, BlockHeight};
use zip32::AccountId;

use zingo_status::confirmation_status::ConfirmationStatus;

use crate::client::{self, FetchRequest, IronwoodSizes};
use crate::config::{PerformanceLevel, SyncConfig};
use crate::error::{
    ContinuityError, MempoolError, ScanError, ServerError, SyncError, SyncModeError,
    SyncStatusError,
};
use crate::keys::transparent::TransparentAddressId;
use crate::scan::ScanResults;
use crate::scan::task::{Scanner, ScannerState};
use crate::scan::transactions::scan_transaction;
use crate::shardtree_ext::{RollbackOutcome, ShardTreeExt};
use crate::sync::state::truncate_scan_ranges;
use crate::wallet::traits::{
    SyncBlocks, SyncNullifiers, SyncOutPoints, SyncShardTrees, SyncTransactions, SyncWallet,
};
use crate::wallet::{
    KeyIdInterface, NoteInterface, NullifierMap, OutputId, OutputInterface, ScanTarget, SyncMode,
    SyncState, WalletBlock, WalletTransaction,
};
use crate::witness::LocatedTreeData;

#[cfg(not(feature = "darkside_test"))]
use crate::witness;

pub(crate) mod spend;
pub(crate) mod state;
pub(crate) mod transparent;
pub mod truncate;

const UNCONFIRMED_SPEND_INVALIDATION_THRESHOLD: u32 = 3;
pub(crate) const MAX_REORG_ALLOWANCE: u32 = 100;
const VERIFY_BLOCK_RANGE_SIZE: u32 = 10;

/// A snapshot of the current state of sync. Useful for displaying the status of sync to a user / consumer.
///
/// `percentage_outputs_scanned` is a much more accurate indicator of sync completion than `percentage_blocks_scanned`.
/// `percentage_total_outputs_scanned` is the percentage of outputs scanned from birthday to chain height.
#[derive(Debug, Clone)]
#[allow(missing_docs)]
pub struct SyncStatus {
    pub scan_ranges: Vec<ScanRange>,
    pub sync_start_height: BlockHeight,
    pub session_blocks_scanned: u32,
    pub total_blocks_scanned: u32,
    pub percentage_session_blocks_scanned: f32,
    pub percentage_total_blocks_scanned: f32,
    pub session_sapling_outputs_scanned: u32,
    pub total_sapling_outputs_scanned: u32,
    pub session_orchard_outputs_scanned: u32,
    pub total_orchard_outputs_scanned: u32,
    pub session_ironwood_outputs_scanned: u32,
    pub total_ironwood_outputs_scanned: u32,
    pub percentage_session_outputs_scanned: f32,
    pub percentage_total_outputs_scanned: f32,
}

// TODO: complete display, scan ranges in raw form are too verbose
impl std::fmt::Display for SyncStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "percentage complete: {}",
            self.percentage_total_outputs_scanned
        )
    }
}

impl From<SyncStatus> for json::JsonValue {
    fn from(value: SyncStatus) -> Self {
        let scan_ranges: Vec<json::JsonValue> = value
            .scan_ranges
            .iter()
            .map(|range| {
                json::object! {
                    "priority" => format!("{:?}", range.priority()),
                    "start_block" => range.block_range().start.to_string(),
                    "end_block" => (range.block_range().end - 1).to_string(),
                }
            })
            .collect();

        json::object! {
            "scan_ranges" => scan_ranges,
            "sync_start_height" => u32::from(value.sync_start_height),
            "session_blocks_scanned" => value.session_blocks_scanned,
            "total_blocks_scanned" => value.total_blocks_scanned,
            "percentage_session_blocks_scanned" => value.percentage_session_blocks_scanned,
            "percentage_total_blocks_scanned" => value.percentage_total_blocks_scanned,
            "session_sapling_outputs_scanned" => value.session_sapling_outputs_scanned,
            "total_sapling_outputs_scanned" => value.total_sapling_outputs_scanned,
            "session_orchard_outputs_scanned" => value.session_orchard_outputs_scanned,
            "total_orchard_outputs_scanned" => value.total_orchard_outputs_scanned,
            "session_ironwood_outputs_scanned" => value.session_ironwood_outputs_scanned,
            "total_ironwood_outputs_scanned" => value.total_ironwood_outputs_scanned,
            "percentage_session_outputs_scanned" => value.percentage_session_outputs_scanned,
            "percentage_total_outputs_scanned" => value.percentage_total_outputs_scanned,
        }
    }
}

/// Returned when [`crate::sync::sync`] successfully completes.
#[derive(Debug, Clone)]
#[allow(missing_docs)]
pub struct SyncResult {
    pub sync_start_height: BlockHeight,
    pub sync_end_height: BlockHeight,
    pub blocks_scanned: u32,
    pub sapling_outputs_scanned: u32,
    pub orchard_outputs_scanned: u32,
    pub ironwood_outputs_scanned: u32,
    pub percentage_total_outputs_scanned: f32,
}

impl std::fmt::Display for SyncResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Sync completed succesfully:
{{
    sync start height: {}
    sync end height: {}
    blocks scanned: {}
    sapling outputs scanned: {}
    orchard outputs scanned: {}
    ironwood outputs scanned: {}
    percentage total outputs scanned: {}
}}",
            self.sync_start_height,
            self.sync_end_height,
            self.blocks_scanned,
            self.sapling_outputs_scanned,
            self.orchard_outputs_scanned,
            self.ironwood_outputs_scanned,
            self.percentage_total_outputs_scanned,
        )
    }
}

impl From<SyncResult> for json::JsonValue {
    fn from(value: SyncResult) -> Self {
        json::object! {
            "sync_start_height" => u32::from(value.sync_start_height),
            "sync_end_height" => u32::from(value.sync_end_height),
            "blocks_scanned" => value.blocks_scanned,
            "sapling_outputs_scanned" => value.sapling_outputs_scanned,
            "orchard_outputs_scanned" => value.orchard_outputs_scanned,
            "ironwood_outputs_scanned" => value.ironwood_outputs_scanned,
            "percentage_total_outputs_scanned" => value.percentage_total_outputs_scanned,
        }
    }
}

/// Scanning range priority levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScanPriority {
    /// Block ranges that are currently refetching nullifiers.
    RefetchingNullifiers,
    /// Block ranges that are currently being scanned.
    Scanning,
    /// Block ranges that have already been scanned will not be re-scanned.
    Scanned,
    /// Block ranges that have already been scanned. The nullifiers from this range were not mapped after scanning and
    /// spend detection to reduce memory consumption and/or storage for non-linear scanning. These nullifiers will need
    /// to be re-fetched for final spend detection when this range is the lowest unscanned range in the wallet's list
    /// of scan ranges.
    ScannedWithoutMapping,
    /// Block ranges to be scanned to advance the fully-scanned height.
    Historic,
    /// Block ranges adjacent to heights at which the user opened the wallet.
    OpenAdjacent,
    /// Blocks that must be scanned to complete note commitment tree shards adjacent to found notes.
    FoundNote,
    /// Blocks that must be scanned to complete the latest note commitment tree shard.
    ChainTip,
    /// A previously scanned range that must be verified to check it is still in the
    /// main chain, has highest priority.
    Verify,
}

impl ScanPriority {
    /// Returns the priority the wallet holds a range selected at this priority while its scan task is in flight.
    #[must_use]
    pub fn in_flight(self) -> ScanPriority {
        if self == ScanPriority::ScannedWithoutMapping {
            ScanPriority::RefetchingNullifiers
        } else {
            ScanPriority::Scanning
        }
    }
}

/// Returns true when the two ranges share at least one block.
pub(crate) fn overlaps(first: &Range<BlockHeight>, second: &Range<BlockHeight>) -> bool {
    first.start < second.end && second.start < first.end
}

/// Returns the scan ranges the wallet holds at `priority`, in wallet order.
pub(crate) fn held_at(
    scan_ranges: &[ScanRange],
    priority: ScanPriority,
) -> impl Iterator<Item = &ScanRange> {
    scan_ranges
        .iter()
        .filter(move |scan_range| scan_range.priority() == priority)
}

/// A range of blocks to be scanned, along with its associated priority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanRange {
    block_range: Range<BlockHeight>,
    priority: ScanPriority,
}

impl std::fmt::Display for ScanRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?}({}..{})",
            self.priority, self.block_range.start, self.block_range.end,
        )
    }
}

impl ScanRange {
    /// Constructs a scan range from its constituent parts.
    #[must_use]
    pub fn from_parts(block_range: Range<BlockHeight>, priority: ScanPriority) -> Self {
        assert!(
            block_range.end >= block_range.start,
            "{block_range:?} is invalid for ScanRange({priority:?})",
        );
        ScanRange {
            block_range,
            priority,
        }
    }

    /// Returns the range of block heights to be scanned.
    #[must_use]
    pub fn block_range(&self) -> &Range<BlockHeight> {
        &self.block_range
    }

    /// Returns the priority with which the scan range should be scanned.
    #[must_use]
    pub fn priority(&self) -> ScanPriority {
        self.priority
    }

    /// Returns whether or not the scan range is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.block_range.is_empty()
    }

    /// Returns true when this scan range and `block_range` share at least one block.
    #[must_use]
    pub fn overlaps(&self, block_range: &Range<BlockHeight>) -> bool {
        overlaps(&self.block_range, block_range)
    }

    /// Returns the number of blocks in the scan range.
    #[must_use]
    pub fn len(&self) -> usize {
        usize::try_from(u32::from(self.block_range.end) - u32::from(self.block_range.start))
            .expect("due to number of max blocks should always be valid usize")
    }

    /// Shifts the start of the block range to the right if `block_height >
    /// self.block_range().start`. Returns `None` if the resulting range would
    /// be empty (or the range was already empty).
    #[must_use]
    pub fn truncate_start(&self, block_height: BlockHeight) -> Option<Self> {
        if block_height >= self.block_range.end || self.is_empty() {
            None
        } else {
            Some(ScanRange {
                block_range: self.block_range.start.max(block_height)..self.block_range.end,
                priority: self.priority,
            })
        }
    }

    /// Shifts the end of the block range to the left if `block_height <
    /// self.block_range().end`. Returns `None` if the resulting range would
    /// be empty (or the range was already empty).
    #[must_use]
    pub fn truncate_end(&self, block_height: BlockHeight) -> Option<Self> {
        if block_height <= self.block_range.start || self.is_empty() {
            None
        } else {
            Some(ScanRange {
                block_range: self.block_range.start..self.block_range.end.min(block_height),
                priority: self.priority,
            })
        }
    }

    /// Splits this scan range at the specified height, such that the provided height becomes the
    /// end of the first range returned and the start of the second. Returns `None` if
    /// `p <= self.block_range().start || p >= self.block_range().end`.
    #[must_use]
    pub fn split_at(&self, p: BlockHeight) -> Option<(Self, Self)> {
        (p > self.block_range.start && p < self.block_range.end).then_some((
            ScanRange {
                block_range: self.block_range.start..p,
                priority: self.priority,
            },
            ScanRange {
                block_range: p..self.block_range.end,
                priority: self.priority,
            },
        ))
    }
}

/// Syncs a wallet to the latest state of the blockchain.
///
/// `sync_mode` is intended to be stored in a struct that owns the wallet(s) (i.e. lightclient) and has a non-atomic
/// counterpart [`crate::wallet::SyncMode`]. The sync engine will set the `sync_mode` to `Running` at the start of sync.
/// However, the consumer is required to set the `sync_mode` back to `NotRunning` when sync is succussful or returns an
/// error. This allows more flexibility and safety with sync task handles etc.
/// `sync_mode` may also be set to `Paused` externally to pause scanning so the wallet lock can be acquired multiple
/// times in quick sucession without the sync engine interrupting.
/// Set `sync_mode` back to `Running` to resume scanning.
/// Set `sync_mode` to `Shutdown` to stop the sync process.
pub async fn sync<P, W>(
    client: CompactTxStreamerClient<Channel>,
    consensus_parameters: &P,
    wallet: Arc<RwLock<W>>,
    sync_mode: Arc<AtomicU8>,
    config: SyncConfig,
) -> Result<SyncResult, SyncError<W::Error>>
where
    P: consensus::Parameters + Sync + Send + 'static,
    W: SyncWallet
        + SyncBlocks
        + SyncTransactions
        + SyncNullifiers
        + SyncOutPoints
        + SyncShardTrees
        + Send,
{
    let mut sync_mode_enum = SyncMode::from_atomic_u8(sync_mode.clone())?;
    if sync_mode_enum == SyncMode::NotRunning {
        sync_mode_enum = SyncMode::Running;
        sync_mode.store(sync_mode_enum as u8, atomic::Ordering::Release);
    } else {
        return Err(SyncModeError::SyncAlreadyRunning.into());
    }

    tracing::info!("Starting sync...");

    // create channel for sending fetch requests and launch fetcher task
    let (fetch_request_sender, fetch_request_receiver) = mpsc::unbounded_channel();
    let client_clone = client.clone();
    let fetcher_handle =
        tokio::spawn(
            async move { client::fetch::fetch(fetch_request_receiver, client_clone).await },
        );

    // create channel for receiving mempool transactions and launch mempool monitor
    let (mempool_transaction_sender, mut mempool_transaction_receiver) = mpsc::channel(100);
    let shutdown_mempool = Arc::new(AtomicBool::new(false));
    let shutdown_mempool_clone = shutdown_mempool.clone();
    let unprocessed_mempool_transactions_count = Arc::new(AtomicU8::new(0));
    let unprocessed_mempool_transactions_count_clone =
        unprocessed_mempool_transactions_count.clone();
    let mempool_handle = tokio::spawn(async move {
        mempool_monitor(
            client,
            mempool_transaction_sender,
            unprocessed_mempool_transactions_count_clone,
            shutdown_mempool_clone,
        )
        .await
    });

    // pre-scan initialisation
    let mut wallet_guard = wallet.write().await;

    // Repair the shard trees before anything reads or truncates them: a session that ended partway through a
    // wallet update leaves tree state above the highest scanned height, which the re-org truncation in
    // `checked_wallet_height` cannot roll back. Subtree roots the rollback removes are fetched again by
    // `update_subtree_roots` below, in this same session.
    rollback_unscanned_tree_states(&mut *wallet_guard)?;

    let chain_height = client::get_chain_height(fetch_request_sender.clone()).await?;
    if chain_height == 0.into() {
        return Err(SyncError::ServerError(ServerError::GenesisBlockOnly));
    }
    let last_known_chain_height =
        checked_wallet_height(&mut *wallet_guard, chain_height, consensus_parameters)?;

    // Ironwood tree sizes for servers that serve no Ironwood tree state, seeded from the wallet's stored blocks
    // now that any re-org truncation has removed the blocks the chain no longer holds.
    let ironwood_sizes = IronwoodSizes::new();
    seed_ironwood_sizes(&mut *wallet_guard, &ironwood_sizes)?;

    let ufvks = wallet_guard
        .get_unified_full_viewing_keys()
        .map_err(SyncError::WalletError)?;

    transparent::update_addresses_and_scan_targets(
        consensus_parameters,
        &mut *wallet_guard,
        fetch_request_sender.clone(),
        &ufvks,
        last_known_chain_height,
        chain_height,
        config.transparent_address_discovery,
    )
    .await?;

    #[cfg(not(feature = "darkside_test"))]
    update_subtree_roots(
        consensus_parameters,
        fetch_request_sender.clone(),
        &mut *wallet_guard,
    )
    .await?;

    add_initial_frontier(
        consensus_parameters,
        fetch_request_sender.clone(),
        &mut *wallet_guard,
    )
    .await?;

    let initial_reorg_detection_start_height = state::update_scan_ranges(
        consensus_parameters,
        last_known_chain_height,
        chain_height,
        wallet_guard
            .get_sync_state_mut()
            .map_err(SyncError::WalletError)?,
    );

    state::set_initial_state(
        consensus_parameters,
        fetch_request_sender.clone(),
        &ironwood_sizes,
        &mut *wallet_guard,
        chain_height,
    )
    .await?;

    expire_transactions(&mut *wallet_guard)?;

    drop(wallet_guard);

    // create channel for receiving scan results and launch scanner
    let (scan_results_sender, mut scan_results_receiver) = mpsc::unbounded_channel();
    let mut scanner = Scanner::new(
        consensus_parameters.clone(),
        scan_results_sender,
        fetch_request_sender.clone(),
        ironwood_sizes.clone(),
        ufvks.clone(),
    );
    scanner.launch(config.performance_level);

    // TODO: implement an option for continuous scanning where it doesnt exit when complete

    let mut nullifier_map_limit_exceeded = false;
    let mut interval = tokio::time::interval(Duration::from_millis(50));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            Some((load, scan_results)) = scan_results_receiver.recv() => {
                for (load, scan_results) in scanner.admit_scan_results(load, scan_results) {
                    let mut wallet_guard = wallet.write().await;
                    process_scan_results(
                        consensus_parameters,
                        &mut *wallet_guard,
                        fetch_request_sender.clone(),
                        &ironwood_sizes,
                        &ufvks,
                        load.scan_range,
                        scan_results,
                        initial_reorg_detection_start_height,
                        config.performance_level,
                        &mut nullifier_map_limit_exceeded,
                    )
                    .await?;
                    scanner.retire_finished_tasks(
                        wallet_guard
                            .get_sync_state()
                            .map_err(SyncError::WalletError)?
                            .scan_ranges(),
                    );
                    wallet_guard.set_save_flag().map_err(SyncError::WalletError)?;
                    drop(wallet_guard);
                }
            }

            Some(raw_transaction) = mempool_transaction_receiver.recv() => {
                let mut wallet_guard = wallet.write().await;
                process_mempool_transaction(
                    consensus_parameters,
                    &ufvks,
                    &mut *wallet_guard,
                    raw_transaction,
                )
                .await?;
                unprocessed_mempool_transactions_count.fetch_sub(1, atomic::Ordering::Release);
                drop(wallet_guard);
            }

            _update_scanner = interval.tick() => {
                sync_mode_enum = SyncMode::from_atomic_u8(sync_mode.clone())?;
                match sync_mode_enum {
                    SyncMode::Paused => {
                        let mut pause_interval = tokio::time::interval(Duration::from_secs(1));
                        pause_interval.tick().await;
                        while sync_mode_enum == SyncMode::Paused {
                            pause_interval.tick().await;
                            sync_mode_enum = SyncMode::from_atomic_u8(sync_mode.clone())?;
                        }
                    },
                    SyncMode::Shutdown => {
                        let mut wallet_guard = wallet.write().await;
                        let sync_status = match sync_status(&*wallet_guard).await {
                            Ok(status) => status,
                            Err(SyncStatusError::WalletError(e)) => {
                                return Err(SyncError::WalletError(e));
                            }
                            Err(SyncStatusError::NoSyncData) => {
                                panic!("sync data must exist!");
                            }
                        };
                        wallet_guard
                            .set_save_flag()
                            .map_err(SyncError::WalletError)?;
                        drop(wallet_guard);
                        tracing::info!("Sync successfully shutdown.");

                        return Ok(SyncResult {
                            sync_start_height: sync_status.sync_start_height,
                            sync_end_height: (sync_status
                                .scan_ranges
                                .last()
                                .expect("should be non-empty after syncing")
                                .block_range()
                                .end
                                - 1),
                            blocks_scanned: sync_status.session_blocks_scanned,
                            sapling_outputs_scanned: sync_status.session_sapling_outputs_scanned,
                            orchard_outputs_scanned: sync_status.session_orchard_outputs_scanned,
                            ironwood_outputs_scanned: sync_status
                                .session_ironwood_outputs_scanned,
                            percentage_total_outputs_scanned: sync_status.percentage_total_outputs_scanned,
                        });
                    }
                    SyncMode::Running => (),
                    SyncMode::NotRunning => {
                        panic!("sync mode should not be manually set to NotRunning!");
                    },
                }

                scanner.update(&mut *wallet.write().await, shutdown_mempool.clone(), nullifier_map_limit_exceeded).await?;

                if matches!(scanner.state, ScannerState::Shutdown) {
                    // wait for mempool monitor to receive mempool transactions
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    if is_shutdown(&scanner, unprocessed_mempool_transactions_count.clone())
                    {
                        tracing::info!("Sync successfully shutdown.");
                        break;
                    }
                }
            }
        }
    }

    let mut wallet_guard = wallet.write().await;
    let sync_status = match sync_status(&*wallet_guard).await {
        Ok(status) => status,
        Err(SyncStatusError::WalletError(e)) => {
            return Err(SyncError::WalletError(e));
        }
        Err(SyncStatusError::NoSyncData) => {
            panic!("sync data must exist!");
        }
    };
    // once sync is complete, all nullifiers will have been re-fetched so this note metadata can be discarded.
    for transaction in wallet_guard
        .get_wallet_transactions_mut()
        .map_err(SyncError::WalletError)?
        .values_mut()
    {
        for note in transaction.sapling_notes.as_mut_slice() {
            note.refetch_nullifier_ranges = Vec::new();
        }
        for note in transaction.orchard_notes.as_mut_slice() {
            note.refetch_nullifier_ranges = Vec::new();
        }
        for note in transaction.ironwood_notes.as_mut_slice() {
            note.refetch_nullifier_ranges = Vec::new();
        }
    }
    wallet_guard
        .set_save_flag()
        .map_err(SyncError::WalletError)?;

    drop(wallet_guard);
    drop(scanner);
    drop(fetch_request_sender);

    match mempool_handle.await.expect("task panicked") {
        Ok(()) => (),
        Err(e @ MempoolError::ShutdownWithoutStream) => tracing::warn!("{e}"),
        Err(e) => return Err(e.into()),
    }
    fetcher_handle.await.expect("task panicked");

    Ok(SyncResult {
        sync_start_height: sync_status.sync_start_height,
        sync_end_height: (sync_status
            .scan_ranges
            .last()
            .expect("should be non-empty after syncing")
            .block_range()
            .end
            - 1),
        blocks_scanned: sync_status.session_blocks_scanned,
        sapling_outputs_scanned: sync_status.session_sapling_outputs_scanned,
        orchard_outputs_scanned: sync_status.session_orchard_outputs_scanned,
        ironwood_outputs_scanned: sync_status.session_ironwood_outputs_scanned,
        percentage_total_outputs_scanned: sync_status.percentage_total_outputs_scanned,
    })
}

/// This ensures that the wallet height used to calculate the lower bound for scan range creation is valid.
/// The comparison takes two input heights and uses several constants to select the correct height.
///
/// The input parameter heights are:
///
///   (1) chain_height:
///       * the best block-height reported by the proxy (zainod or lwd)
///   (2) last_known_chain_height
///       * the last max height the wallet recorded from earlier scans
///
/// The constants are:
///   (1) MAX_REORG_ALLOWANCE:
///       * the maximum number of blocks the wallet can truncate during re-org detection
///   (2) Sapling Activation Height:
///       * the lower bound on the wallet birthday
fn checked_wallet_height<W, P>(
    wallet: &mut W,
    chain_height: BlockHeight,
    consensus_parameters: &P,
) -> Result<BlockHeight, SyncError<W::Error>>
where
    W: SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints + SyncShardTrees,
    P: zcash_protocol::consensus::Parameters,
{
    let sync_state = wallet.get_sync_state().map_err(SyncError::WalletError)?;
    if let Some(last_known_chain_height) = sync_state.last_known_chain_height() {
        if last_known_chain_height > chain_height {
            if last_known_chain_height - chain_height >= MAX_REORG_ALLOWANCE {
                // There's a human attention requiring problem, the wallet supplied
                // last_known_chain_height is more than MAX_REORG_ALLOWANCE **above**
                // the proxy's reported height.
                return Err(SyncError::ChainError(
                    u32::from(last_known_chain_height),
                    MAX_REORG_ALLOWANCE,
                    u32::from(chain_height),
                ));
            }
            // The wallet reported height is above the current proxy height
            // reset to the proxy height.
            truncate_wallet_data(wallet, chain_height)?;
            return Ok(chain_height);
        }
        // The last wallet reported height is equal or below the proxy height.
        Ok(last_known_chain_height)
    } else {
        // This is the wallet's first sync. Use [birthday - 1] as wallet height.
        let sapling_activation_height = consensus_parameters
            .activation_height(consensus::NetworkUpgrade::Sapling)
            .expect("sapling activation height should always return Some");
        let birthday = wallet.get_birthday().map_err(SyncError::WalletError)?;
        if birthday > chain_height {
            // Human attention requiring error, a birthday *above* the proxy reported
            // chain height has been provided.
            return Err(SyncError::ChainError(
                u32::from(birthday),
                MAX_REORG_ALLOWANCE,
                u32::from(chain_height),
            ));
        } else if birthday < sapling_activation_height {
            return Err(SyncError::BirthdayBelowSapling(
                u32::from(birthday),
                u32::from(sapling_activation_height),
            ));
        }

        Ok(birthday - 1)
    }
}

/// Creates a [`self::SyncStatus`] from the wallet's current [`crate::wallet::SyncState`].
/// If there is still nullifiers to be re-fetched when scanning is complete, the percentages will be overrided to 99%
/// until sync is complete.
///
/// Intended to be called while [`self::sync`] is running in a separate task.
pub async fn sync_status<W>(wallet: &W) -> Result<SyncStatus, SyncStatusError<W::Error>>
where
    W: SyncWallet + SyncBlocks,
{
    let (
        total_sapling_outputs_scanned,
        total_orchard_outputs_scanned,
        total_ironwood_outputs_scanned,
    ) = state::calculate_scanned_outputs(wallet).map_err(SyncStatusError::WalletError)?;
    let total_outputs_scanned = total_sapling_outputs_scanned
        + total_orchard_outputs_scanned
        + total_ironwood_outputs_scanned;

    let sync_state = wallet
        .get_sync_state()
        .map_err(SyncStatusError::WalletError)?;
    if sync_state.initial_sync_state.sync_start_height == 0.into() {
        return Ok(SyncStatus {
            scan_ranges: sync_state.scan_ranges.clone(),
            sync_start_height: 0.into(),
            session_blocks_scanned: 0,
            total_blocks_scanned: 0,
            percentage_session_blocks_scanned: 0.0,
            percentage_total_blocks_scanned: 0.0,
            session_sapling_outputs_scanned: 0,
            session_orchard_outputs_scanned: 0,
            session_ironwood_outputs_scanned: 0,
            total_sapling_outputs_scanned: 0,
            total_orchard_outputs_scanned: 0,
            total_ironwood_outputs_scanned: 0,
            percentage_session_outputs_scanned: 0.0,
            percentage_total_outputs_scanned: 0.0,
        });
    }
    let total_blocks_scanned = state::calculate_scanned_blocks(sync_state);

    let birthday = sync_state
        .wallet_birthday()
        .ok_or(SyncStatusError::NoSyncData)?;
    let last_known_chain_height = sync_state
        .last_known_chain_height()
        .ok_or(SyncStatusError::NoSyncData)?;
    let total_blocks = last_known_chain_height - birthday + 1;
    let total_sapling_outputs = sync_state
        .initial_sync_state
        .wallet_tree_bounds
        .sapling_final_tree_size
        - sync_state
            .initial_sync_state
            .wallet_tree_bounds
            .sapling_initial_tree_size;
    let total_orchard_outputs = sync_state
        .initial_sync_state
        .wallet_tree_bounds
        .orchard_final_tree_size
        - sync_state
            .initial_sync_state
            .wallet_tree_bounds
            .orchard_initial_tree_size;
    let total_ironwood_outputs = sync_state
        .initial_sync_state
        .wallet_tree_bounds
        .ironwood_final_tree_size
        - sync_state
            .initial_sync_state
            .wallet_tree_bounds
            .ironwood_initial_tree_size;
    let total_outputs = total_sapling_outputs + total_orchard_outputs + total_ironwood_outputs;

    let session_blocks_scanned =
        total_blocks_scanned - sync_state.initial_sync_state.previously_scanned_blocks;
    let mut percentage_session_blocks_scanned = ((session_blocks_scanned as f32
        / (total_blocks - sync_state.initial_sync_state.previously_scanned_blocks) as f32)
        * 100.0)
        .clamp(0.0, 100.0);
    let mut percentage_total_blocks_scanned =
        ((total_blocks_scanned as f32 / total_blocks as f32) * 100.0).clamp(0.0, 100.0);

    let session_sapling_outputs_scanned = total_sapling_outputs_scanned
        - sync_state
            .initial_sync_state
            .previously_scanned_sapling_outputs;
    let session_orchard_outputs_scanned = total_orchard_outputs_scanned
        - sync_state
            .initial_sync_state
            .previously_scanned_orchard_outputs;
    let session_ironwood_outputs_scanned = total_ironwood_outputs_scanned
        - sync_state
            .initial_sync_state
            .previously_scanned_ironwood_outputs;
    let session_outputs_scanned = session_sapling_outputs_scanned
        + session_orchard_outputs_scanned
        + session_ironwood_outputs_scanned;
    let previously_scanned_outputs = sync_state
        .initial_sync_state
        .previously_scanned_sapling_outputs
        + sync_state
            .initial_sync_state
            .previously_scanned_orchard_outputs
        + sync_state
            .initial_sync_state
            .previously_scanned_ironwood_outputs;
    let mut percentage_session_outputs_scanned = ((session_outputs_scanned as f32
        / (total_outputs - previously_scanned_outputs) as f32)
        * 100.0)
        .clamp(0.0, 100.0);
    let mut percentage_total_outputs_scanned =
        ((total_outputs_scanned as f32 / total_outputs as f32) * 100.0).clamp(0.0, 100.0);

    if sync_state.scan_ranges().iter().any(|scan_range| {
        scan_range.priority() == ScanPriority::ScannedWithoutMapping
            || scan_range.priority() == ScanPriority::RefetchingNullifiers
    }) {
        if percentage_session_blocks_scanned == 100.0 {
            percentage_session_blocks_scanned = 99.0;
        }
        if percentage_total_blocks_scanned == 100.0 {
            percentage_total_blocks_scanned = 99.0;
        }
        if percentage_session_outputs_scanned == 100.0 {
            percentage_session_outputs_scanned = 99.0;
        }
        if percentage_total_outputs_scanned == 100.0 {
            percentage_total_outputs_scanned = 99.0;
        }
    }

    Ok(SyncStatus {
        scan_ranges: sync_state.scan_ranges.clone(),
        sync_start_height: sync_state.initial_sync_state.sync_start_height,
        session_blocks_scanned,
        total_blocks_scanned,
        percentage_session_blocks_scanned,
        percentage_total_blocks_scanned,
        session_sapling_outputs_scanned,
        total_sapling_outputs_scanned,
        session_orchard_outputs_scanned,
        total_orchard_outputs_scanned,
        session_ironwood_outputs_scanned,
        total_ironwood_outputs_scanned,
        percentage_session_outputs_scanned,
        percentage_total_outputs_scanned,
    })
}

/// Scans a pending `transaction` of a given `status`, adding to the wallet and updating output spend statuses.
///
/// Used both internally for scanning mempool transactions and externally for scanning calculated and transmitted
/// transactions during send.
///
/// Panics if `status` is of `Confirmed` variant.
pub fn scan_pending_transaction<W>(
    consensus_parameters: &impl consensus::Parameters,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    wallet: &mut W,
    transaction: Transaction,
    status: ConfirmationStatus,
    datetime: u32,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints + SyncShardTrees,
{
    if matches!(status, ConfirmationStatus::Confirmed(_)) {
        panic!("this fn is for unconfirmed transactions only");
    }

    let mut pending_transaction_nullifiers = NullifierMap::new();
    let mut pending_transaction_outpoints = BTreeMap::new();
    let transparent_addresses: HashMap<String, TransparentAddressId> = wallet
        .get_transparent_addresses()
        .map_err(SyncError::WalletError)?
        .iter()
        .map(|(id, address)| (address.clone(), *id))
        .collect();
    let pending_transaction = scan_transaction(
        consensus_parameters,
        ufvks,
        transaction.txid(),
        transaction,
        status,
        None,
        &mut pending_transaction_nullifiers,
        &mut pending_transaction_outpoints,
        &transparent_addresses,
        datetime,
    )?;

    let wallet_transactions = wallet
        .get_wallet_transactions()
        .map_err(SyncError::WalletError)?;
    let transparent_output_ids = spend::collect_transparent_output_ids(wallet_transactions);
    let transparent_spend_scan_targets = spend::detect_transparent_spends(
        &mut pending_transaction_outpoints,
        transparent_output_ids,
    );
    let (sapling_derived_nullifiers, orchard_derived_nullifiers, ironwood_derived_nullifiers) =
        spend::collect_derived_nullifiers(wallet_transactions);
    let (sapling_spend_scan_targets, orchard_spend_scan_targets, ironwood_spend_scan_targets) =
        spend::detect_shielded_spends(
            &mut pending_transaction_nullifiers,
            sapling_derived_nullifiers,
            orchard_derived_nullifiers,
            ironwood_derived_nullifiers,
        );

    // return if transaction is not relevant to the wallet
    if pending_transaction.transparent_coins().is_empty()
        && pending_transaction.sapling_notes().is_empty()
        && pending_transaction.orchard_notes().is_empty()
        && pending_transaction.ironwood_notes().is_empty()
        && pending_transaction.outgoing_orchard_notes().is_empty()
        && pending_transaction.outgoing_ironwood_notes().is_empty()
        && pending_transaction.outgoing_sapling_notes().is_empty()
        && transparent_spend_scan_targets.is_empty()
        && sapling_spend_scan_targets.is_empty()
        && orchard_spend_scan_targets.is_empty()
        && ironwood_spend_scan_targets.is_empty()
    {
        return Ok(());
    }

    wallet
        .insert_wallet_transaction(pending_transaction)
        .map_err(SyncError::WalletError)?;
    spend::update_spent_coins(
        wallet
            .get_wallet_transactions_mut()
            .map_err(SyncError::WalletError)?,
        transparent_spend_scan_targets,
    );
    spend::update_spent_notes(
        wallet,
        sapling_spend_scan_targets,
        orchard_spend_scan_targets,
        ironwood_spend_scan_targets,
        false,
    )
    .map_err(SyncError::WalletError)?;

    Ok(())
}

/// API for targetted scanning.
///
/// Allows `scan_targets` to be added externally to the wallet's `sync_state` and be prioritised for scanning. Each
/// scan target must include the block height which will be used to prioritise the block range containing the note
/// commitments to the surrounding orchard shard(s). If the block height is pre-orchard then the surrounding sapling
/// shard(s) will be prioritised instead. The txid in each scan target may be omitted and set to [0u8; 32] in order to
/// prioritise the surrounding blocks for scanning but be ignored when fetching specific relevant transactions to the
/// wallet. However, in the case where a relevant spending transaction at a given height contains no decryptable
/// incoming notes (change), only the nullifier will be mapped and this transaction will be scanned when the
/// transaction containing the spent notes is scanned instead.
pub fn add_scan_targets(sync_state: &mut SyncState, scan_targets: &[ScanTarget]) {
    for scan_target in scan_targets {
        sync_state.scan_targets.insert(*scan_target);
    }
}

/// Resets the spending transaction field of all outputs that were previously spent but became unspent due to a
/// spending transactions becoming invalid.
///
/// `invalid_txids` are the id's of the invalidated spending transactions. Any outputs in the `wallet_transactions`
/// matching these spending transactions will be reset back to `None`.
pub fn reset_spends(
    wallet_transactions: &mut HashMap<TxId, WalletTransaction>,
    invalid_txids: Vec<TxId>,
) {
    wallet_transactions
        .values_mut()
        .flat_map(|transaction| transaction.ironwood_notes_mut())
        .filter(|output| {
            output
                .spending_transaction
                .is_some_and(|spending_txid| invalid_txids.contains(&spending_txid))
        })
        .for_each(|output| {
            output.set_spending_transaction(None);
        });
    wallet_transactions
        .values_mut()
        .flat_map(|transaction| transaction.orchard_notes_mut())
        .filter(|output| {
            output
                .spending_transaction
                .is_some_and(|spending_txid| invalid_txids.contains(&spending_txid))
        })
        .for_each(|output| {
            output.set_spending_transaction(None);
        });
    wallet_transactions
        .values_mut()
        .flat_map(|transaction| transaction.sapling_notes_mut())
        .filter(|output| {
            output
                .spending_transaction
                .is_some_and(|spending_txid| invalid_txids.contains(&spending_txid))
        })
        .for_each(|output| {
            output.set_spending_transaction(None);
        });
    wallet_transactions
        .values_mut()
        .flat_map(|transaction| transaction.transparent_coins_mut())
        .filter(|output| {
            output
                .spending_transaction
                .is_some_and(|spending_txid| invalid_txids.contains(&spending_txid))
        })
        .for_each(|output| {
            output.set_spending_transaction(None);
        });
}

/// Sets transactions associated with list of `failed_txids` in `wallet_transactions` to `Failed` status.
///
/// Sets the `spending_transaction` fields of any outputs spent in these transactions to `None`.
pub fn set_transactions_failed(
    wallet_transactions: &mut HashMap<TxId, WalletTransaction>,
    failed_txids: Vec<TxId>,
) {
    for failed_txid in failed_txids.iter() {
        if let Some(transaction) = wallet_transactions.get_mut(failed_txid) {
            let height = transaction.status().get_height();
            transaction.update_status(
                ConfirmationStatus::Failed(height),
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .expect("infalliable for such long time periods")
                    .as_secs() as u32,
            );
        }
    }
    reset_spends(wallet_transactions, failed_txids);
}

/// Returns true if the scanner and mempool are shutdown.
fn is_shutdown<P>(
    scanner: &Scanner<P>,
    mempool_unprocessed_transactions_count: Arc<AtomicU8>,
) -> bool
where
    P: consensus::Parameters + Sync + Send + 'static,
{
    scanner.worker_poolsize() == 0
        && mempool_unprocessed_transactions_count.load(atomic::Ordering::Acquire) == 0
}

/// Scan post-processing
#[allow(clippy::too_many_arguments)]
async fn process_scan_results<P, W>(
    consensus_parameters: &P,
    wallet: &mut W,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ironwood_sizes: &IronwoodSizes,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    scan_range: ScanRange,
    scan_results: Result<ScanResults, ScanError>,
    initial_reorg_detection_start_height: BlockHeight,
    performance_level: PerformanceLevel,
    nullifier_map_limit_exceeded: &mut bool,
) -> Result<(), SyncError<W::Error>>
where
    P: consensus::Parameters + Sync,
    W: SyncWallet
        + SyncBlocks
        + SyncTransactions
        + SyncNullifiers
        + SyncOutPoints
        + SyncShardTrees
        + Send,
{
    match scan_results {
        Ok(results) => {
            let ScanResults {
                mut nullifiers,
                mut outpoints,
                scanned_blocks,
                wallet_transactions,
                sapling_located_trees,
                orchard_located_trees,
                ironwood_located_trees,
            } = results;

            if scan_range.priority() == ScanPriority::ScannedWithoutMapping {
                // add missing block bounds in the case that nullifier batch limit was reached and the fetch nullifier
                // scan range was split.
                let full_refetching_nullifiers_range = wallet
                    .get_sync_state()
                    .map_err(SyncError::WalletError)?
                    .scan_ranges
                    .iter()
                    .find(|&wallet_scan_range| {
                        wallet_scan_range
                            .block_range()
                            .contains(&scan_range.block_range().start)
                            && wallet_scan_range
                                .block_range()
                                .contains(&(scan_range.block_range().end - 1))
                    })
                    .expect("wallet scan range containing scan range should exist!");
                if scan_range.block_range().start
                    != full_refetching_nullifiers_range.block_range().start
                    || scan_range.block_range().end
                        != full_refetching_nullifiers_range.block_range().end
                {
                    let mut missing_block_bounds = BTreeMap::new();
                    for block_bound in [
                        scan_range.block_range().start - 1,
                        scan_range.block_range().start,
                        scan_range.block_range().end - 1,
                        scan_range.block_range().end,
                    ] {
                        if block_bound < full_refetching_nullifiers_range.block_range().start
                            || block_bound >= full_refetching_nullifiers_range.block_range().end
                        {
                            continue;
                        }
                        if wallet.get_wallet_block(block_bound).is_err() {
                            missing_block_bounds.insert(
                                block_bound,
                                WalletBlock::from_compact_block(
                                    consensus_parameters,
                                    fetch_request_sender.clone(),
                                    ironwood_sizes,
                                    &client::get_compact_block(
                                        fetch_request_sender.clone(),
                                        block_bound,
                                    )
                                    .await?,
                                )
                                .await?,
                            );
                        }
                    }
                    if !missing_block_bounds.is_empty() {
                        wallet
                            .append_wallet_blocks(missing_block_bounds)
                            .map_err(SyncError::WalletError)?;
                    }
                }

                let first_unscanned_range = wallet
                    .get_sync_state()
                    .map_err(SyncError::WalletError)?
                    .scan_ranges
                    .iter()
                    .find(|scan_range| scan_range.priority() != ScanPriority::Scanned)
                    .expect("the scan range being processed is not yet set to scanned so at least one unscanned range must exist");
                if !first_unscanned_range
                    .block_range()
                    .contains(&scan_range.block_range().start)
                    || !first_unscanned_range
                        .block_range()
                        .contains(&(scan_range.block_range().end - 1))
                {
                    // in this rare edge case, a scanned `ScannedWithoutMapping` range was the highest priority yet it was not the first unscanned range so it must be discarded to avoid missing spends

                    // reset scan range from `RefetchingNullifiers` to `ScannedWithoutMapping`
                    state::reset_refetching_nullifiers_scan_range(
                        wallet
                            .get_sync_state_mut()
                            .map_err(SyncError::WalletError)?,
                        scan_range.block_range().clone(),
                    );
                    tracing::debug!(
                        "Nullifiers discarded and will be re-fetched to avoid missing spends."
                    );

                    return Ok(());
                }

                spend::update_shielded_spends(
                    consensus_parameters,
                    wallet,
                    fetch_request_sender.clone(),
                    ironwood_sizes,
                    ufvks,
                    &scanned_blocks,
                    Some(&mut nullifiers),
                )
                .await?;

                state::set_scanned_scan_range(
                    wallet
                        .get_sync_state_mut()
                        .map_err(SyncError::WalletError)?,
                    scan_range.block_range().clone(),
                    true, // NOTE: although nullifiers are not actually added to the wallet's nullifier map for efficiency, there is effectively no difference as spends are still updated using the `additional_nullifier_map` and would be removed on the following cleanup (`remove_irrelevant_data`) due to `ScannedWithoutMapping` ranges always being the first non-scanned range and therefore always raise the wallet's fully scanned height after processing.
                );
            } else {
                // nullifiers are not mapped if nullifier map size limit will be exceeded
                if !*nullifier_map_limit_exceeded {
                    let nullifier_map = wallet.get_nullifiers().map_err(SyncError::WalletError)?;
                    if max_nullifier_map_size(performance_level).is_some_and(|max| {
                        nullifier_map.orchard.len()
                            + nullifier_map.ironwood.len()
                            + nullifier_map.sapling.len()
                            + nullifiers.orchard.len()
                            + nullifiers.ironwood.len()
                            + nullifiers.sapling.len()
                            > max
                    }) {
                        *nullifier_map_limit_exceeded = true;
                    }
                }
                let mut map_nullifiers = !*nullifier_map_limit_exceeded;

                // all transparent spend locations are known before scanning so there is no need to map outpoints from untargetted ranges.
                // outpoints of untargetted ranges will still be checked before being discarded.
                let map_outpoints = scan_range.priority() >= ScanPriority::FoundNote;

                // always map nullifiers if scanning the lowest range to be scanned for final spend detection.
                // this will set the range to `Scanned` (as oppose to `ScannedWithoutMapping`) and prevent immediate
                // re-fetching of the nullifiers in this range. these will be immediately cleared after cleanup so will not
                // have an impact on memory or wallet file size.
                // the selected range is not the lowest range to be scanned unless all ranges before it are scanned or
                // scanning.
                for query_scan_range in wallet
                    .get_sync_state()
                    .map_err(SyncError::WalletError)?
                    .scan_ranges()
                {
                    let scan_priority = query_scan_range.priority();
                    if scan_priority != ScanPriority::Scanned
                        && scan_priority != ScanPriority::Scanning
                        && scan_priority != ScanPriority::RefetchingNullifiers
                    {
                        break;
                    }

                    if scan_priority == ScanPriority::Scanning
                        && query_scan_range
                            .block_range()
                            .contains(&scan_range.block_range().start)
                        && query_scan_range
                            .block_range()
                            .contains(&(scan_range.block_range().end - 1))
                    {
                        map_nullifiers = true;
                        break;
                    }
                }

                update_wallet_data(
                    consensus_parameters,
                    wallet,
                    fetch_request_sender.clone(),
                    ufvks,
                    &scan_range,
                    &scanned_blocks,
                    if map_nullifiers {
                        Some(&mut nullifiers)
                    } else {
                        None
                    },
                    if map_outpoints {
                        Some(&mut outpoints)
                    } else {
                        None
                    },
                    wallet_transactions,
                    sapling_located_trees,
                    orchard_located_trees,
                    ironwood_located_trees,
                )
                .await?;
                spend::update_transparent_spends(
                    wallet,
                    if map_outpoints {
                        None
                    } else {
                        Some(&mut outpoints)
                    },
                )
                .map_err(SyncError::WalletError)?;
                spend::update_shielded_spends(
                    consensus_parameters,
                    wallet,
                    fetch_request_sender,
                    ironwood_sizes,
                    ufvks,
                    &scanned_blocks,
                    if map_nullifiers {
                        None
                    } else {
                        Some(&mut nullifiers)
                    },
                )
                .await?;
                add_scanned_blocks(wallet, scanned_blocks, &scan_range)
                    .map_err(SyncError::WalletError)?;

                state::set_scanned_scan_range(
                    wallet
                        .get_sync_state_mut()
                        .map_err(SyncError::WalletError)?,
                    scan_range.block_range().clone(),
                    map_nullifiers,
                );
                state::merge_scan_ranges(
                    wallet
                        .get_sync_state_mut()
                        .map_err(SyncError::WalletError)?,
                    ScanPriority::ScannedWithoutMapping,
                );
            }

            state::merge_scan_ranges(
                wallet
                    .get_sync_state_mut()
                    .map_err(SyncError::WalletError)?,
                ScanPriority::Scanned,
            );
            remove_irrelevant_data(wallet).map_err(SyncError::WalletError)?;
            tracing::debug!("Scan results processed.");
        }
        Err(ScanError::ContinuityError(ContinuityError::HashDiscontinuity { height, .. })) => {
            tracing::warn!("Hash discontinuity detected before block {height}.");
            if height == scan_range.block_range().start
                && scan_range.priority() == ScanPriority::Verify
            {
                tracing::info!("Re-org detected.");
                let sync_state = wallet
                    .get_sync_state_mut()
                    .map_err(SyncError::WalletError)?;
                let last_known_chain_height = sync_state
                    .last_known_chain_height()
                    .expect("scan ranges should be non-empty in this scope");

                // reset scan range from `Scanning` to `Verify`
                state::set_scan_priority(
                    sync_state,
                    scan_range.block_range(),
                    ScanPriority::Verify,
                );

                // extend verification range to VERIFY_BLOCK_RANGE_SIZE blocks below current verification range
                let current_reorg_detection_start_height = state::set_verify_scan_range(
                    sync_state,
                    height - 1,
                    state::VerifyEnd::VerifyHighest,
                )
                .block_range()
                .start;
                state::merge_scan_ranges(sync_state, ScanPriority::Verify);

                if initial_reorg_detection_start_height - current_reorg_detection_start_height
                    > MAX_REORG_ALLOWANCE
                {
                    clear_wallet_data(wallet)?;

                    return Err(ServerError::ChainVerificationError.into());
                }

                truncate_wallet_data(wallet, current_reorg_detection_start_height - 1)?;
                ironwood_sizes.truncate_above(current_reorg_detection_start_height - 1);

                state::set_initial_state(
                    consensus_parameters,
                    fetch_request_sender.clone(),
                    ironwood_sizes,
                    wallet,
                    last_known_chain_height,
                )
                .await?;
            } else {
                scan_results?;
            }
        }
        Err(e) => return Err(e.into()),
    }

    Ok(())
}

/// Rolls the shard trees back to the wallet's highest scanned height where they hold state from blocks above it.
///
/// A sync session that ends partway through a wallet update leaves the note commitments and checkpoints of blocks
/// in the shard trees while the scan ranges of these blocks are still to be scanned. Scanning the blocks again
/// inserts the same note commitments, unless a re-org has replaced the blocks since. The note commitments of the
/// replacing blocks then conflict with the stale ones on every sync session, as re-org handling only truncates
/// blocks the wallet records as scanned.
///
/// A rollback also removes the pool's subtree roots above the highest scanned height. [`update_subtree_roots`]
/// fetches them again in the same sync session.
fn rollback_unscanned_tree_states<W>(wallet: &mut W) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncShardTrees,
{
    let Some(highest_scanned_height) = wallet
        .get_sync_state()
        .map_err(SyncError::WalletError)?
        .highest_scanned_height()
    else {
        return Ok(());
    };

    for pool in [
        ShieldedPool::Sapling,
        ShieldedPool::Orchard,
        ShieldedPool::Ironwood,
    ] {
        let shard_trees = wallet
            .get_shard_trees_mut()
            .map_err(SyncError::WalletError)?;
        let rolled_back = match pool {
            ShieldedPool::Sapling => {
                rollback_unscanned_tree_state(highest_scanned_height, &mut shard_trees.sapling)
            }
            ShieldedPool::Orchard => {
                rollback_unscanned_tree_state(highest_scanned_height, &mut shard_trees.orchard)
            }
            ShieldedPool::Ironwood => {
                rollback_unscanned_tree_state(highest_scanned_height, &mut shard_trees.ironwood)
            }
        }?;
        if rolled_back {
            tracing::warn!(
                "{pool:?} shard tree held state above the highest scanned height \
                 {highest_scanned_height} and was rolled back to its checkpoint at this height."
            );
            wallet.set_save_flag().map_err(SyncError::WalletError)?;
        }
    }

    Ok(())
}

/// Rolls `tree` back to its checkpoint at `highest_scanned_height` where [`truncate::plan_unscanned_state_rollback`]
/// finds state above this height. Returns whether the tree was rolled back.
///
/// shardtree refuses to roll back to a checkpoint whose note commitment the tree is yet to hold or has pruned into
/// a larger subtree. Such a tree is left as it is.
fn rollback_unscanned_tree_state<H, const DEPTH: u8, const SHARD_HEIGHT: u8>(
    highest_scanned_height: BlockHeight,
    tree: &mut ShardTree<MemoryShardStore<H, BlockHeight>, DEPTH, SHARD_HEIGHT>,
) -> Result<bool, shardtree::error::ShardTreeError<Infallible>>
where
    H: incrementalmerkletree::Hashable + Clone + PartialEq,
{
    let Some(checkpoint) = truncate::plan_unscanned_state_rollback(
        truncate::tree_facts(tree, highest_scanned_height),
        highest_scanned_height,
    ) else {
        return Ok(false);
    };

    match tree.rollback_to_checkpoint(checkpoint) {
        Ok(RollbackOutcome::RolledBack) => Ok(true),
        Ok(RollbackOutcome::NoSuchCheckpoint) => panic!("checkpoint must exist in this scope"),
        Err(shardtree::error::ShardTreeError::Query(
            shardtree::error::QueryError::CheckpointPruned,
        )) => {
            tracing::warn!(
                "shard tree holds state above the highest scanned height {highest_scanned_height} \
                 and cannot roll back to its checkpoint at this height."
            );
            Ok(false)
        }
        Err(e) => Err(e),
    }
}

/// Processes mempool transaction.
///
/// Scan the transaction and add to the wallet if relevant.
async fn process_mempool_transaction<W>(
    consensus_parameters: &impl consensus::Parameters,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    wallet: &mut W,
    raw_transaction: RawTransaction,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints + SyncShardTrees,
{
    // does not use raw transaction height due to lightwalletd off-by-one bug and potential to be zero
    let mempool_height = wallet
        .get_sync_state()
        .map_err(SyncError::WalletError)?
        .last_known_chain_height()
        .expect("wallet height must exist after sync is initialised")
        + 1;

    let transaction = zcash_primitives::transaction::Transaction::read(
        &raw_transaction.data[..],
        consensus::BranchId::for_height(consensus_parameters, mempool_height),
    )
    .map_err(ServerError::InvalidTransaction)?;

    tracing::debug!(
        "mempool received txid {} at height {}",
        transaction.txid(),
        mempool_height
    );

    if let Some(tx) = wallet
        .get_wallet_transactions_mut()
        .map_err(SyncError::WalletError)?
        .get_mut(&transaction.txid())
    {
        tx.update_status(
            ConfirmationStatus::Mempool(mempool_height),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("infalliable for such long time periods")
                .as_secs() as u32,
        );

        return Ok(());
    }

    scan_pending_transaction(
        consensus_parameters,
        ufvks,
        wallet,
        transaction,
        ConfirmationStatus::Mempool(mempool_height),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("infalliable for such long time periods")
            .as_secs() as u32,
    )?;

    Ok(())
}

/// Removes wallet blocks, transactions, nullifiers, outpoints and shard tree data above the given `truncate_height`.
///
/// The decision of what a correct truncation does is made purely by
/// [`truncate::plan_truncation`] from the wallet state, the shard-tree
/// state, and the truncation target; this function only applies the
/// returned plan.
fn truncate_wallet_data<W>(
    wallet: &mut W,
    truncate_height: BlockHeight,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints + SyncShardTrees,
{
    let sync_state = wallet
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?;
    let wallet_state = truncate::WalletTruncationState {
        birthday: sync_state
            .wallet_birthday()
            .expect("should be non-empty in this scope"),
        highest_scanned_height: sync_state
            .highest_scanned_height()
            .expect("should be non-empty in this scope"),
    };
    // The scan ranges roll back to the plan's effective height alongside
    // the stores: a no-op still trims ranges above the target, and a full
    // clear resets the sync state.
    match truncate::plan_truncation(wallet_state, truncate_height) {
        truncate::TruncationPlan::NoOp => {
            truncate_scan_ranges(truncate_height, sync_state);
            Ok(())
        }
        truncate::TruncationPlan::ClearAll => {
            truncate_scan_ranges(consensus::H0, sync_state);
            truncate_stores(wallet, consensus::H0)?;
            wallet.clear_shard_trees()
        }
        truncate::TruncationPlan::Truncate { height } => {
            truncate_scan_ranges(height, sync_state);
            truncate_stores(wallet, height)?;
            match wallet.truncate_shard_trees(height) {
                Ok(()) => Ok(()),
                Err(SyncError::TruncationError(height, pooltype)) => {
                    clear_wallet_data(wallet)?;

                    Err(SyncError::TruncationError(height, pooltype))
                }
                Err(e) => Err(e),
            }
        }
    }
}

/// Removes wallet blocks, transactions, nullifiers and outpoints above the
/// given `truncate_height`.
fn truncate_stores<W>(
    wallet: &mut W,
    truncate_height: BlockHeight,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints,
{
    wallet
        .truncate_wallet_blocks(truncate_height)
        .map_err(SyncError::WalletError)?;
    wallet
        .truncate_wallet_transactions(truncate_height)
        .map_err(SyncError::WalletError)?;
    wallet
        .truncate_nullifiers(truncate_height)
        .map_err(SyncError::WalletError)?;
    wallet
        .truncate_outpoints(truncate_height)
        .map_err(SyncError::WalletError)?;

    Ok(())
}

fn clear_wallet_data<W>(wallet: &mut W) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints + SyncShardTrees,
{
    let scan_targets = wallet
        .get_wallet_transactions()
        .map_err(SyncError::WalletError)?
        .values()
        .filter_map(|transaction| {
            transaction
                .status()
                .get_confirmed_height()
                .map(|height| ScanTarget {
                    block_height: height,
                    txid: transaction.txid(),
                    narrow_scan_area: true,
                })
        })
        .collect::<Vec<_>>();
    truncate_wallet_data(wallet, consensus::H0)?;
    wallet
        .get_wallet_transactions_mut()
        .map_err(SyncError::WalletError)?
        .clear();
    let sync_state = wallet
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?;
    add_scan_targets(sync_state, &scan_targets);
    wallet.set_save_flag().map_err(SyncError::WalletError)?;

    Ok(())
}

/// Updates the wallet with data from `scan_results`
#[allow(clippy::too_many_arguments)]
async fn update_wallet_data<P, W>(
    consensus_parameters: &P,
    wallet: &mut W,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    scan_range: &ScanRange,
    scanned_blocks: &BTreeMap<BlockHeight, WalletBlock>,
    nullifiers: Option<&mut NullifierMap>,
    outpoints: Option<&mut BTreeMap<OutputId, ScanTarget>>,
    mut transactions: HashMap<TxId, WalletTransaction>,
    sapling_located_trees: Vec<LocatedTreeData<sapling_crypto::Node>>,
    orchard_located_trees: Vec<LocatedTreeData<MerkleHashOrchard>>,
    ironwood_located_trees: Vec<LocatedTreeData<MerkleHashOrchard>>,
) -> Result<(), SyncError<W::Error>>
where
    P: consensus::Parameters + Sync,
    W: SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints + SyncShardTrees + Send,
{
    let sync_state = wallet
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?;
    let highest_scanned_height = sync_state
        .highest_scanned_height()
        .expect("scan ranges should not be empty in this scope");
    for transaction in transactions.values() {
        state::update_found_note_shard_priority(
            consensus_parameters,
            sync_state,
            ShieldedPool::Sapling,
            transaction,
        );
        state::update_found_note_shard_priority(
            consensus_parameters,
            sync_state,
            ShieldedPool::Orchard,
            transaction,
        );
        state::update_found_note_shard_priority(
            consensus_parameters,
            sync_state,
            ShieldedPool::Ironwood,
            transaction,
        );
    }
    // add all block ranges of scan ranges with `ScannedWithoutMapping` or `RefetchingNullifiers` priority above the
    // current scan range to each note to track which ranges need the nullifiers to be re-fetched before the note is
    // known to be unspent (in addition to all other ranges above the notes height being `Scanned`,
    // `ScannedWithoutMapping` or `RefetchingNullifiers` priority). this information is necessary as these ranges have been scanned but the
    // nullifiers have been discarded so must be re-fetched. if ranges are scanned but the nullifiers are discarded
    // (set to `ScannedWithoutMapping` priority) *after* this note has been added to the wallet, this is sufficient to
    // know this note has not been spent, even if this range is not set to `Scanned` priority.
    let refetch_nullifier_ranges = {
        let block_ranges: Vec<Range<BlockHeight>> = sync_state
            .scan_ranges()
            .iter()
            .filter(|&scan_range| {
                scan_range.priority() == ScanPriority::ScannedWithoutMapping
                    || scan_range.priority() == ScanPriority::RefetchingNullifiers
            })
            .map(|scan_range| scan_range.block_range().clone())
            .collect();

        block_ranges
            [block_ranges.partition_point(|range| range.start < scan_range.block_range().end)..]
            .to_vec()
    };
    for transaction in transactions.values_mut() {
        for note in transaction.sapling_notes.as_mut_slice() {
            note.refetch_nullifier_ranges = refetch_nullifier_ranges.clone();
        }
        for note in transaction.orchard_notes.as_mut_slice() {
            note.refetch_nullifier_ranges = refetch_nullifier_ranges.clone();
        }
        for note in transaction.ironwood_notes.as_mut_slice() {
            note.refetch_nullifier_ranges = refetch_nullifier_ranges.clone();
        }
    }
    for transaction in transactions.values() {
        discover_unified_addresses(wallet, ufvks, transaction).map_err(SyncError::WalletError)?;
    }

    wallet
        .extend_wallet_transactions(transactions)
        .map_err(SyncError::WalletError)?;
    if let Some(nullifiers) = nullifiers {
        wallet
            .append_nullifiers(nullifiers)
            .map_err(SyncError::WalletError)?;
    }
    if let Some(outpoints) = outpoints {
        wallet
            .append_outpoints(outpoints)
            .map_err(SyncError::WalletError)?;
    }
    wallet
        .update_shard_trees(
            consensus_parameters,
            fetch_request_sender,
            scan_range,
            highest_scanned_height,
            scanned_blocks,
            sapling_located_trees,
            orchard_located_trees,
            ironwood_located_trees,
        )
        .await?;

    Ok(())
}

fn discover_unified_addresses<W>(
    wallet: &mut W,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    transaction: &WalletTransaction,
) -> Result<(), W::Error>
where
    W: SyncWallet,
{
    for note in transaction
        .orchard_notes()
        .iter()
        .filter(|&note| note.key_id().scope == zip32::Scope::External)
    {
        let ivk = ufvks
            .get(&note.key_id().account_id())
            .expect("ufvk must exist to decrypt this note")
            .orchard()
            .expect("fvk must exist to decrypt this note")
            .to_ivk(zip32::Scope::External);

        wallet.add_orchard_address(
            note.key_id().account_id(),
            note.note().recipient(),
            ivk.diversifier_index(&note.note().recipient())
                .expect("must be key used to create this address"),
        )?;
    }
    // Ironwood notes are received at Orchard receivers, so they discover Orchard addresses too.
    for note in transaction
        .ironwood_notes()
        .iter()
        .filter(|&note| note.key_id().scope == zip32::Scope::External)
    {
        let ivk = ufvks
            .get(&note.key_id().account_id())
            .expect("ufvk must exist to decrypt this note")
            .orchard()
            .expect("fvk must exist to decrypt this note")
            .to_ivk(zip32::Scope::External);

        wallet.add_orchard_address(
            note.key_id().account_id(),
            note.note().recipient(),
            ivk.diversifier_index(&note.note().recipient())
                .expect("must be key used to create this address"),
        )?;
    }
    for note in transaction
        .sapling_notes()
        .iter()
        .filter(|&note| note.key_id().scope == zip32::Scope::External)
    {
        let ivk = ufvks
            .get(&note.key_id().account_id())
            .expect("ufvk must exist to decrypt this note")
            .sapling()
            .expect("fvk must exist to decrypt this note")
            .to_external_ivk();

        wallet.add_sapling_address(
            note.key_id().account_id(),
            note.note().recipient(),
            ivk.decrypt_diversifier(&note.note().recipient())
                .expect("must be key used to create this address"),
        )?;
    }

    Ok(())
}

fn remove_irrelevant_data<W>(wallet: &mut W) -> Result<(), W::Error>
where
    W: SyncWallet + SyncBlocks + SyncOutPoints + SyncNullifiers + SyncTransactions,
{
    let fully_scanned_height = wallet
        .get_sync_state()?
        .fully_scanned_height()
        .expect("scan ranges must be non-empty");

    wallet
        .get_outpoints_mut()?
        .retain(|_, scan_target| scan_target.block_height > fully_scanned_height);
    wallet
        .get_nullifiers_mut()?
        .sapling
        .retain(|_, scan_target| scan_target.block_height > fully_scanned_height);
    wallet
        .get_nullifiers_mut()?
        .orchard
        .retain(|_, scan_target| scan_target.block_height > fully_scanned_height);
    wallet
        .get_nullifiers_mut()?
        .ironwood
        .retain(|_, scan_target| scan_target.block_height > fully_scanned_height);
    wallet
        .get_sync_state_mut()?
        .scan_targets
        .retain(|scan_target| scan_target.block_height > fully_scanned_height);
    remove_irrelevant_blocks(wallet)?;

    Ok(())
}

fn remove_irrelevant_blocks<W>(wallet: &mut W) -> Result<(), W::Error>
where
    W: SyncWallet + SyncBlocks + SyncTransactions,
{
    let sync_state = wallet.get_sync_state()?;
    let highest_scanned_height = sync_state
        .highest_scanned_height()
        .expect("should be non-empty");
    let scanned_range_bounds = sync_state
        .scan_ranges()
        .iter()
        .filter(|scan_range| {
            scan_range.priority() == ScanPriority::Scanned
                || scan_range.priority() == ScanPriority::ScannedWithoutMapping
                || scan_range.priority() == ScanPriority::RefetchingNullifiers
        })
        .flat_map(|scanned_range| {
            vec![
                scanned_range.block_range().start,
                scanned_range.block_range().end - 1,
            ]
        })
        .collect::<Vec<_>>();
    let wallet_transaction_heights = wallet
        .get_wallet_transactions()?
        .values()
        .filter_map(|tx| tx.status().get_confirmed_height())
        .collect::<Vec<_>>();

    wallet.get_wallet_blocks_mut()?.retain(|height, _| {
        *height >= highest_scanned_height.saturating_sub(MAX_REORG_ALLOWANCE)
            || scanned_range_bounds.contains(height)
            || wallet_transaction_heights.contains(height)
    });

    Ok(())
}

fn add_scanned_blocks<W>(
    wallet: &mut W,
    mut scanned_blocks: BTreeMap<BlockHeight, WalletBlock>,
    scan_range: &ScanRange,
) -> Result<(), W::Error>
where
    W: SyncWallet + SyncBlocks + SyncTransactions,
{
    let sync_state = wallet.get_sync_state()?;
    let highest_scanned_height = sync_state
        .highest_scanned_height()
        .expect("scan ranges must be non-empty");

    let wallet_transaction_heights = wallet
        .get_wallet_transactions()?
        .values()
        .filter_map(|tx| tx.status().get_confirmed_height())
        .collect::<Vec<_>>();

    scanned_blocks.retain(|height, _| {
        *height >= highest_scanned_height.saturating_sub(MAX_REORG_ALLOWANCE)
            || *height == scan_range.block_range().start
            || *height == scan_range.block_range().end - 1
            || wallet_transaction_heights.contains(height)
    });

    wallet.append_wallet_blocks(scanned_blocks)?;

    Ok(())
}

#[cfg(not(feature = "darkside_test"))]
async fn update_subtree_roots<W>(
    consensus_parameters: &impl consensus::Parameters,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    wallet: &mut W,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncShardTrees,
{
    // Resume from the stored-root count, except that a newest root which
    // is still bare (never scanned into) is refetched every session: no
    // checkpoint witnesses it, so this refetch is the only mechanism that
    // heals it after a reorg (see `subtree_fetch_start_index`). When a
    // refetch happens, the pool's newest stored shard range is dropped
    // before the fetched roots are accounted, so the range accounting is
    // rebuilt from the refetched root — never duplicated, and corrected
    // if a reorg moved the subtree's completing height.
    let shard_trees = wallet.get_shard_trees().map_err(SyncError::WalletError)?;
    let stored_sapling_roots = witness::stored_subtree_root_count(&shard_trees.sapling);
    let stored_orchard_roots = witness::stored_subtree_root_count(&shard_trees.orchard);
    let stored_ironwood_roots = witness::stored_subtree_root_count(&shard_trees.ironwood);
    let sapling_start_index = witness::subtree_fetch_start_index(&shard_trees.sapling);
    let orchard_start_index = witness::subtree_fetch_start_index(&shard_trees.orchard);
    let ironwood_start_index = witness::subtree_fetch_start_index(&shard_trees.ironwood);
    let (sapling_subtree_roots, orchard_subtree_roots, ironwood_subtree_roots) = futures::join!(
        client::get_subtree_roots(fetch_request_sender.clone(), sapling_start_index, 0, 0),
        client::get_subtree_roots(fetch_request_sender.clone(), orchard_start_index, 1, 0),
        client::get_subtree_roots(fetch_request_sender, ironwood_start_index, 2, 0)
    );

    let sapling_subtree_roots = sapling_subtree_roots?;
    let orchard_subtree_roots = orchard_subtree_roots?;
    let ironwood_subtree_roots = ironwood_subtree_roots?;

    let sync_state = wallet
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?;
    if (sapling_start_index as usize) < stored_sapling_roots && !sapling_subtree_roots.is_empty() {
        state::pop_newest_shard_range(sync_state, ShieldedPool::Sapling);
    }
    state::add_shard_ranges(
        consensus_parameters,
        ShieldedPool::Sapling,
        sync_state,
        &sapling_subtree_roots,
    );
    if (orchard_start_index as usize) < stored_orchard_roots && !orchard_subtree_roots.is_empty() {
        state::pop_newest_shard_range(sync_state, ShieldedPool::Orchard);
    }
    state::add_shard_ranges(
        consensus_parameters,
        ShieldedPool::Orchard,
        sync_state,
        &orchard_subtree_roots,
    );
    if (ironwood_start_index as usize) < stored_ironwood_roots && !ironwood_subtree_roots.is_empty()
    {
        state::pop_newest_shard_range(sync_state, ShieldedPool::Ironwood);
    }
    state::add_shard_ranges(
        consensus_parameters,
        ShieldedPool::Ironwood,
        sync_state,
        &ironwood_subtree_roots,
    );

    let shard_trees = wallet
        .get_shard_trees_mut()
        .map_err(SyncError::WalletError)?;
    witness::add_subtree_roots(
        sapling_start_index as usize,
        sapling_subtree_roots,
        &mut shard_trees.sapling,
    )?;
    witness::add_subtree_roots(
        orchard_start_index as usize,
        orchard_subtree_roots,
        &mut shard_trees.orchard,
    )?;
    witness::add_subtree_roots(
        ironwood_start_index as usize,
        ironwood_subtree_roots,
        &mut shard_trees.ironwood,
    )?;

    Ok(())
}

async fn add_initial_frontier<W>(
    consensus_parameters: &impl consensus::Parameters,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    wallet: &mut W,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncShardTrees,
{
    let birthday = wallet.get_birthday().map_err(SyncError::WalletError)?;
    if birthday
        == consensus_parameters
            .activation_height(consensus::NetworkUpgrade::Sapling)
            .expect("sapling activation height should always return Some")
    {
        return Ok(());
    }

    // if the shard store only contains the first checkpoint added on initialisation, add frontiers to complete the
    // shard trees.
    let shard_trees = wallet
        .get_shard_trees_mut()
        .map_err(SyncError::WalletError)?;
    if shard_trees
        .sapling
        .store()
        .checkpoint_count()
        .expect("infallible")
        == 1
    {
        let tree_state = client::get_tree_state(fetch_request_sender, birthday).await?;
        for pool in [ShieldedPool::Sapling, ShieldedPool::Orchard] {
            if client::tree_state_omitted(consensus_parameters, &tree_state, pool, birthday) {
                return Err(SyncError::ServerError(ServerError::TreeStateNotServed {
                    pool: zcash_protocol::PoolType::Shielded(pool),
                    height: birthday,
                }));
            }
        }
        // the Ironwood frontier cannot be rebuilt from a count of actions, so a birthday above the pool's
        // activation height needs the server to serve the pool.
        if client::tree_state_omitted(
            consensus_parameters,
            &tree_state,
            ShieldedPool::Ironwood,
            birthday,
        ) {
            return Err(SyncError::ServerError(
                ServerError::IronwoodTreeStateUnavailable(birthday),
            ));
        }
        let frontiers = tree_state
            .to_chain_state()
            .map_err(|e| SyncError::ServerError(ServerError::InvalidFrontier(e)))?;
        shard_trees
            .sapling
            .insert_frontier(
                frontiers.final_sapling_tree().clone(),
                Retention::Checkpoint {
                    id: birthday,
                    marking: Marking::None,
                },
            )
            .expect("infallible");
        shard_trees
            .orchard
            .insert_frontier(
                frontiers.final_orchard_tree().clone(),
                Retention::Checkpoint {
                    id: birthday,
                    marking: Marking::None,
                },
            )
            .expect("infallible");
        shard_trees
            .ironwood
            .insert_frontier(
                frontiers.final_ironwood_tree().clone(),
                Retention::Checkpoint {
                    id: birthday,
                    marking: Marking::None,
                },
            )
            .expect("infallible");
    }

    Ok(())
}

/// Sets up mempool stream.
///
/// If there is some raw transaction, send to be scanned.
/// If the mempool stream message is `None` (a block was mined) or the request failed, setup a new mempool stream.
async fn mempool_monitor(
    mut client: CompactTxStreamerClient<Channel>,
    mempool_transaction_sender: mpsc::Sender<RawTransaction>,
    unprocessed_transactions_count: Arc<AtomicU8>,
    shutdown_mempool: Arc<AtomicBool>,
) -> Result<(), MempoolError> {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    'main: loop {
        let response =
            client::get_mempool_transaction_stream(&mut client, shutdown_mempool.clone()).await;

        match response {
            Ok(mut mempool_stream) => {
                interval.reset();
                loop {
                    tokio::select! {
                        mempool_stream_message = mempool_stream.message() => {
                            match mempool_stream_message.unwrap_or(None) {
                                Some(raw_transaction) => {
                                    // counted before sending so the shutdown check never observes a zero count
                                    // while a transaction is queued in the channel.
                                    unprocessed_transactions_count.fetch_add(1, atomic::Ordering::Release);
                                    if mempool_transaction_sender
                                        .send(raw_transaction)
                                        .await
                                        .is_err()
                                    {
                                        // the receiver is dropped once sync has ended, so nothing will process
                                        // the transaction.
                                        unprocessed_transactions_count.store(0, atomic::Ordering::Release);
                                        break 'main;
                                    }
                                }
                                None => {
                                    continue 'main;
                                }
                            }

                        }

                        _ = interval.tick() => {
                            if shutdown_mempool.load(atomic::Ordering::Acquire) {
                                break 'main;
                            }
                        }
                    }
                }
            }
            Err(e @ MempoolError::ShutdownWithoutStream) => return Err(e),
            Err(MempoolError::ServerError(e)) => {
                tracing::warn!("Mempool stream request failed! Status: {e}.\nRetrying...");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    }

    Ok(())
}

/// Records the Ironwood tree size at the end of every block the wallet stores in `ironwood_sizes`, so a server
/// that serves no Ironwood tree state is only asked to count the Ironwood actions of blocks the wallet has not
/// scanned (see [`IronwoodSizes`]).
fn seed_ironwood_sizes<W>(
    wallet: &mut W,
    ironwood_sizes: &IronwoodSizes,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncBlocks,
{
    let wallet_blocks = wallet
        .get_wallet_blocks_mut()
        .map_err(SyncError::WalletError)?;
    ironwood_sizes.extend(
        wallet_blocks
            .iter()
            .map(|(height, block)| (*height, block.tree_bounds().ironwood_final_tree_size)),
    );

    Ok(())
}

/// Spends will be reset to free up funds if transaction has been unconfirmed for
/// `UNCONFIRMED_SPEND_INVALIDATION_THRESHOLD` confirmed blocks.
/// Transaction status will then be set to `Failed` if it's still unconfirmed when the chain reaches it's expiry height.
// TODO: add config to pepper-sync to set UNCONFIRMED_SPEND_INVALIDATION_THRESHOLD
fn expire_transactions<W>(wallet: &mut W) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncTransactions,
{
    let last_known_chain_height = wallet
        .get_sync_state()
        .map_err(SyncError::WalletError)?
        .last_known_chain_height()
        .expect("wallet height must exist after scan ranges have been updated");
    let wallet_transactions = wallet
        .get_wallet_transactions_mut()
        .map_err(SyncError::WalletError)?;

    let expired_txids = wallet_transactions
        .values()
        .filter(|transaction| {
            transaction.status().is_pending()
                && last_known_chain_height >= transaction.transaction().expiry_height()
        })
        .map(super::wallet::WalletTransaction::txid)
        .collect::<Vec<_>>();
    set_transactions_failed(wallet_transactions, expired_txids);

    let stuck_funds_txids = wallet_transactions
        .values()
        .filter(|transaction| {
            transaction.status().is_pending()
                && last_known_chain_height
                    >= transaction.status().get_height() + UNCONFIRMED_SPEND_INVALIDATION_THRESHOLD
        })
        .map(super::wallet::WalletTransaction::txid)
        .collect::<Vec<_>>();
    reset_spends(wallet_transactions, stuck_funds_txids);

    Ok(())
}

fn max_nullifier_map_size(performance_level: PerformanceLevel) -> Option<usize> {
    match performance_level {
        PerformanceLevel::Low => Some(0),
        PerformanceLevel::Medium => Some(125_000),
        PerformanceLevel::High => Some(2_000_000),
        PerformanceLevel::Maximum => None,
    }
}

#[cfg(test)]
mod test {
    /// The truncation contract of [`crate::sync::truncate_wallet_data`]:
    /// a reorg truncation rolls every store back to the truncate height,
    /// and a shard tree that records nothing above that height — such as
    /// the empty ironwood tree a pre-ironwood (v0) wallet blob migrates
    /// to — is untouched, never classified broken. Only a tree that
    /// holds state above the height and cannot roll back to it forces
    /// the clear-and-rescan path.
    mod truncation {
        use std::collections::BTreeMap;

        use zcash_primitives::block::BlockHash;
        use zcash_protocol::consensus::BlockHeight;

        use crate::mocks::MockWalletBuilder;
        use crate::shardtree_ext::{CheckpointAppendOutcome, ShardTreeExt};
        use crate::sync::{ScanPriority, ScanRange, truncate_wallet_data};
        use crate::wallet::{ShardTrees, SyncState, TreeBounds, WalletBlock, traits::SyncBlocks};

        /// A wallet block carrying only what truncation reads: its height.
        fn block(height: u32) -> WalletBlock {
            WalletBlock {
                block_height: BlockHeight::from_u32(height),
                block_hash: BlockHash([0; 32]),
                prev_hash: BlockHash([0; 32]),
                time: 0,
                txids: Vec::new(),
                tree_bounds: TreeBounds {
                    sapling_initial_tree_size: 0,
                    sapling_final_tree_size: 0,
                    orchard_initial_tree_size: 0,
                    orchard_final_tree_size: 0,
                    ironwood_initial_tree_size: 0,
                    ironwood_final_tree_size: 0,
                },
            }
        }

        /// A wallet synced through height 10 with a birthday of 6: one
        /// fully scanned range, one wallet block per scanned height, and
        /// the given shard trees.
        fn synced_wallet(shard_trees: ShardTrees) -> crate::mocks::MockWallet {
            let wallet_blocks: BTreeMap<_, _> = (6..=10u32)
                .map(|height| (BlockHeight::from_u32(height), block(height)))
                .collect();
            let sync_state = SyncState::new_for_test(vec![ScanRange::from_parts(
                BlockHeight::from_u32(6)..BlockHeight::from_u32(11),
                ScanPriority::Scanned,
            )]);
            MockWalletBuilder::new()
                .birthday(BlockHeight::from_u32(6))
                .sync_state(sync_state)
                .wallet_blocks(wallet_blocks)
                .shard_trees(shard_trees)
                .create_mock_wallet()
        }

        /// A pre-ironwood (v0) wallet blob deserializes to an ironwood
        /// tree holding only the initialization checkpoint at height
        /// zero (`ShardTrees::read`), while sapling and orchard carry
        /// the checkpoints of past scanning. The first routine reorg
        /// truncation after the upgrade must roll sapling and orchard
        /// back and leave the empty ironwood tree untouched — not
        /// classify it broken and wipe the wallet.
        #[test]
        fn migrated_wallet_survives_reorg_truncation() {
            // Sapling and orchard hold checkpoints for the scanned
            // heights; the ironwood tree stays exactly as
            // `ShardTrees::new` built it (its height-zero initialization
            // checkpoint and nothing else), which is also the state
            // `ShardTrees::read` produces for a pre-ironwood blob.
            let mut shard_trees = ShardTrees::new();
            for height in 6..=10u32 {
                assert_eq!(
                    shard_trees
                        .sapling
                        .append_checkpoint(BlockHeight::from_u32(height))
                        .unwrap(),
                    CheckpointAppendOutcome::Appended
                );
                assert_eq!(
                    shard_trees
                        .orchard
                        .append_checkpoint(BlockHeight::from_u32(height))
                        .unwrap(),
                    CheckpointAppendOutcome::Appended
                );
            }
            let mut wallet = synced_wallet(shard_trees);

            // A routine two-block reorg rolls the wallet back to height 8.
            let result = truncate_wallet_data(&mut wallet, BlockHeight::from_u32(8));

            assert!(
                result.is_ok(),
                "reorg truncation wiped a healthy migrated wallet: {result:?}"
            );
            // The blocks at and below the truncate height survive; the
            // clear-and-rescan path leaves none.
            assert!(wallet.get_wallet_block(BlockHeight::from_u32(6)).is_ok());
            assert!(wallet.get_wallet_block(BlockHeight::from_u32(8)).is_ok());
            assert!(wallet.get_wallet_block(BlockHeight::from_u32(9)).is_err());
        }

        /// A tree that records state above the truncate height but holds
        /// no checkpoint at it cannot roll back; the established recovery
        /// — clear all wallet data and rescan — is preserved for it.
        #[test]
        fn unrecoverable_tree_still_clears_wallet_data() {
            // Orchard scanned past the target but its checkpoint at the
            // target height is gone (e.g. pruned): checkpoints exist only
            // above it.
            let mut shard_trees = ShardTrees::new();
            for height in 6..=10u32 {
                assert_eq!(
                    shard_trees
                        .sapling
                        .append_checkpoint(BlockHeight::from_u32(height))
                        .unwrap(),
                    CheckpointAppendOutcome::Appended
                );
            }
            for height in 9..=10u32 {
                assert_eq!(
                    shard_trees
                        .orchard
                        .append_checkpoint(BlockHeight::from_u32(height))
                        .unwrap(),
                    CheckpointAppendOutcome::Appended
                );
            }
            let mut wallet = synced_wallet(shard_trees);

            let result = truncate_wallet_data(&mut wallet, BlockHeight::from_u32(8));

            assert!(matches!(
                result,
                Err(crate::error::SyncError::TruncationError(_, _))
            ));
            // The wallet was cleared for rescan.
            assert!(wallet.get_wallet_block(BlockHeight::from_u32(6)).is_err());
        }
    }

    /// The repair [`crate::sync::rollback_unscanned_tree_states`] applies at the start of a sync session: a shard
    /// tree holding state from blocks above the highest scanned height rolls back to its checkpoint at this height,
    /// and every other tree is left as it is.
    mod unscanned_shard_tree_state {
        use std::convert::Infallible;

        use incrementalmerkletree::{Marking, Position, Retention};
        use orchard::tree::MerkleHashOrchard;
        use shardtree::error::ShardTreeError;
        use shardtree::store::{Checkpoint, ShardStore as _};
        #[cfg(not(feature = "darkside_test"))]
        use zcash_client_backend::proto::service::SubtreeRoot;
        use zcash_protocol::consensus::BlockHeight;

        use crate::mocks::{MockWallet, MockWalletBuilder};
        use crate::sync::{ScanPriority, ScanRange, rollback_unscanned_tree_states};
        use crate::wallet::{ShardTrees, SyncState, traits::SyncShardTrees};
        use crate::witness;

        const BIRTHDAY: u32 = 6;
        const HIGHEST_SCANNED: u32 = 10;
        const NEXT_BLOCK: u32 = HIGHEST_SCANNED + 1;
        /// Each block holds one orchard note commitment, so the block above the highest scanned height inserts here.
        const NEXT_BLOCK_POSITION: u64 = (NEXT_BLOCK - BIRTHDAY) as u64;

        const SCANNED_CHAIN: u8 = 1;
        const STALE_BLOCK: u8 = 2;
        const REPLACING_BLOCK: u8 = 3;

        /// A note commitment that differs for each `chain_tag`.
        fn note_commitment(chain_tag: u8) -> MerkleHashOrchard {
            let mut bytes = [0; 32];
            bytes[0] = chain_tag;
            MerkleHashOrchard::from_bytes(&bytes).expect("a small value is a valid note commitment")
        }

        /// The note commitment of the block at `height` with the checkpoint scanning gives the last note commitment
        /// of a block.
        fn block_leaf(chain_tag: u8, height: u32) -> (MerkleHashOrchard, Retention<BlockHeight>) {
            (
                note_commitment(chain_tag),
                Retention::Checkpoint {
                    id: BlockHeight::from_u32(height),
                    marking: Marking::None,
                },
            )
        }

        /// Shard trees with the orchard note commitments of every block from the birthday to the highest scanned
        /// height.
        fn scanned_shard_trees() -> ShardTrees {
            let mut shard_trees = ShardTrees::new();
            for height in BIRTHDAY..=HIGHEST_SCANNED {
                let (leaf, retention) = block_leaf(SCANNED_CHAIN, height);
                shard_trees.orchard.append(leaf, retention).unwrap();
            }

            shard_trees
        }

        /// Inserts the orchard note commitment of the block above the highest scanned height as scan results do.
        fn insert_next_block(
            shard_trees: &mut ShardTrees,
            chain_tag: u8,
        ) -> Result<(), ShardTreeError<Infallible>> {
            const LOCATED_TREE_SIZE: usize = 1;
            for located_tree in witness::build_located_trees(
                Position::from(NEXT_BLOCK_POSITION),
                vec![block_leaf(chain_tag, NEXT_BLOCK)],
                LOCATED_TREE_SIZE,
            ) {
                shard_trees
                    .orchard
                    .insert_tree(located_tree.subtree, located_tree.checkpoints)?;
            }

            Ok(())
        }

        /// A wallet scanned from the birthday to the highest scanned height with the block above still to be
        /// scanned.
        fn wallet(shard_trees: ShardTrees) -> MockWallet {
            let sync_state = SyncState::new_for_test(vec![
                ScanRange::from_parts(
                    BlockHeight::from_u32(BIRTHDAY)..BlockHeight::from_u32(NEXT_BLOCK),
                    ScanPriority::Scanned,
                ),
                ScanRange::from_parts(
                    BlockHeight::from_u32(NEXT_BLOCK)..BlockHeight::from_u32(NEXT_BLOCK + 1),
                    ScanPriority::Verify,
                ),
            ]);
            MockWalletBuilder::new()
                .birthday(BlockHeight::from_u32(BIRTHDAY))
                .sync_state(sync_state)
                .shard_trees(shard_trees)
                .create_mock_wallet()
        }

        /// A sync session ended after it inserted the note commitments of the block above the highest scanned
        /// height and before it recorded the block as scanned. A re-org then replaced the block, so scanning it
        /// again conflicts with the stale note commitment. The rollback removes the stale note commitment and its
        /// checkpoint, and the replacing block inserts.
        #[test]
        fn stale_block_is_rolled_back_and_its_replacement_inserts() {
            let mut shard_trees = scanned_shard_trees();
            insert_next_block(&mut shard_trees, STALE_BLOCK).unwrap();
            assert!(
                insert_next_block(&mut shard_trees, REPLACING_BLOCK).is_err(),
                "the replacing block must conflict with the stale block for this test to hold"
            );
            let mut wallet = wallet(shard_trees);

            rollback_unscanned_tree_states(&mut wallet).unwrap();

            let shard_trees = wallet.get_shard_trees_mut().unwrap();
            assert_eq!(
                shard_trees.orchard.max_leaf_position(None).unwrap(),
                Some(Position::from(NEXT_BLOCK_POSITION - 1)),
                "only the note commitments of scanned blocks are kept"
            );
            assert_eq!(
                shard_trees.orchard.store().max_checkpoint_id().unwrap(),
                Some(BlockHeight::from_u32(HIGHEST_SCANNED))
            );
            insert_next_block(shard_trees, REPLACING_BLOCK)
                .expect("the replacing block inserts into the rolled back tree");
        }

        /// A wallet whose shard trees hold only scanned blocks keeps the subtree roots above its highest scanned
        /// height, which a wallet that is behind the chain tip holds.
        #[cfg(not(feature = "darkside_test"))]
        #[test]
        fn scanned_shard_trees_are_left_as_they_are() {
            const SUBTREE_ABOVE_HIGHEST_SCANNED: usize = 1;
            let mut shard_trees = scanned_shard_trees();
            witness::add_subtree_roots(
                SUBTREE_ABOVE_HIGHEST_SCANNED,
                vec![SubtreeRoot {
                    root_hash: note_commitment(SCANNED_CHAIN).to_bytes().to_vec(),
                    ..Default::default()
                }],
                &mut shard_trees.orchard,
            )
            .unwrap();
            let mut wallet = wallet(shard_trees);

            rollback_unscanned_tree_states(&mut wallet).unwrap();

            assert_eq!(
                witness::stored_subtree_root_count(&wallet.get_shard_trees_mut().unwrap().orchard),
                SUBTREE_ABOVE_HIGHEST_SCANNED + 1
            );
        }

        /// The orchard pool has no note commitments in the scanned blocks or in the block above them, so the
        /// checkpoints of these blocks point to the last note commitment of a block below the scanned range, which
        /// the tree is yet to hold. shardtree refuses to roll back to such a checkpoint, so the session starts with
        /// the tree as it is.
        #[test]
        fn shard_tree_that_cannot_roll_back_is_left_as_it_is() {
            let mut shard_trees = ShardTrees::new();
            for height in [HIGHEST_SCANNED, NEXT_BLOCK] {
                shard_trees
                    .orchard
                    .store_mut()
                    .add_checkpoint(
                        BlockHeight::from_u32(height),
                        Checkpoint::at_position(Position::from(NEXT_BLOCK_POSITION - 1)),
                    )
                    .unwrap();
            }
            let mut wallet = wallet(shard_trees);

            rollback_unscanned_tree_states(&mut wallet).unwrap();

            let shard_trees = wallet.get_shard_trees_mut().unwrap();
            assert_eq!(
                shard_trees.orchard.store().max_checkpoint_id().unwrap(),
                Some(BlockHeight::from_u32(NEXT_BLOCK))
            );
        }
    }

    mod checked_height_validation {
        use zcash_protocol::consensus::BlockHeight;
        use zcash_protocol::local_consensus::LocalNetwork;
        const LOCAL_NETWORK: LocalNetwork = LocalNetwork {
            overwinter: Some(BlockHeight::from_u32(1)),
            sapling: Some(BlockHeight::from_u32(3)),
            blossom: Some(BlockHeight::from_u32(3)),
            heartwood: Some(BlockHeight::from_u32(3)),
            canopy: Some(BlockHeight::from_u32(3)),
            nu5: Some(BlockHeight::from_u32(3)),
            nu6: Some(BlockHeight::from_u32(3)),
            nu6_1: Some(BlockHeight::from_u32(3)),
            nu6_2: Some(BlockHeight::from_u32(3)),
            nu6_3: Some(BlockHeight::from_u32(3)),
        };
        use crate::{error::SyncError, mocks::MockWalletError, sync::checked_wallet_height};
        // It's possible an error from an implementor's get_sync_state could bubble up to checked_wallet_height
        // this test shows that such an error is raies wrapped in a WalletError and return as the Err variant
        #[tokio::test]
        async fn get_sync_state_error() {
            let builder = crate::mocks::MockWalletBuilder::new();
            let test_error = "get_sync_state_error";
            let mut test_wallet = builder
                .get_sync_state_patch(Box::new(|_| {
                    Err(MockWalletError::AnErrorVariant(test_error.to_string()))
                }))
                .create_mock_wallet();
            let res =
                checked_wallet_height(&mut test_wallet, BlockHeight::from_u32(1), &LOCAL_NETWORK);
            assert!(matches!(
                res,
                Err(SyncError::WalletError(
                    crate::mocks::MockWalletError::AnErrorVariant(ref s)
                )) if s == test_error
            ));
        }

        mod last_known_chain_height {
            use crate::{
                sync::{MAX_REORG_ALLOWANCE, ScanRange},
                wallet::SyncState,
            };
            const DEFAULT_START_HEIGHT: BlockHeight = BlockHeight::from_u32(1);
            const _DEFAULT_LAST_KNOWN_HEIGHT: BlockHeight = BlockHeight::from_u32(102);
            const DEFAULT_CHAIN_HEIGHT: BlockHeight = BlockHeight::from_u32(110);

            use super::*;
            #[tokio::test]
            async fn above_allowance() {
                const LAST_KNOWN_HEIGHT: BlockHeight = BlockHeight::from_u32(211);
                let lkch = vec![ScanRange::from_parts(
                    DEFAULT_START_HEIGHT..LAST_KNOWN_HEIGHT,
                    crate::sync::ScanPriority::Scanned,
                )];
                let state = SyncState {
                    scan_ranges: lkch,
                    ..Default::default()
                };
                let builder = crate::mocks::MockWalletBuilder::new();
                let mut test_wallet = builder.sync_state(state).create_mock_wallet();
                let res =
                    checked_wallet_height(&mut test_wallet, DEFAULT_CHAIN_HEIGHT, &LOCAL_NETWORK);
                if let Err(e) = res {
                    assert_eq!(
                        e.to_string(),
                        format!(
                            "wallet height {} is more than {} blocks ahead of best chain height {}",
                            LAST_KNOWN_HEIGHT - 1,
                            MAX_REORG_ALLOWANCE,
                            DEFAULT_CHAIN_HEIGHT
                        )
                    );
                } else {
                    panic!()
                }
            }
            #[tokio::test]
            async fn above_chain_height_below_allowance() {
                // The hain_height is received from the proxy
                // truncate uses the wallet scan start height
                // as a
                let lkch = vec![ScanRange::from_parts(
                    BlockHeight::from_u32(6)..BlockHeight::from_u32(10),
                    crate::sync::ScanPriority::Scanned,
                )];
                let state = SyncState {
                    scan_ranges: lkch,
                    ..Default::default()
                };
                let builder = crate::mocks::MockWalletBuilder::new();
                let mut test_wallet = builder.sync_state(state).create_mock_wallet();
                let chain_height = BlockHeight::from_u32(4);
                // This will trigger a call to truncate_wallet_data with
                // chain_height and start_height inferred from the wallet.
                // chain must be greater than by this time which hits the Greater cmp
                // match
                let res = checked_wallet_height(&mut test_wallet, chain_height, &LOCAL_NETWORK);
                assert_eq!(res.unwrap(), BlockHeight::from_u32(4));
            }
            #[ignore = "in progress"]
            #[tokio::test]
            async fn equal_or_below_chain_height_and_above_sapling() {
                let lkch = vec![ScanRange::from_parts(
                    BlockHeight::from_u32(1)..BlockHeight::from_u32(10),
                    crate::sync::ScanPriority::Scanned,
                )];
                let state = SyncState {
                    scan_ranges: lkch,
                    ..Default::default()
                };
                let builder = crate::mocks::MockWalletBuilder::new();
                let mut _test_wallet = builder.sync_state(state).create_mock_wallet();
            }
            #[ignore = "in progress"]
            #[tokio::test]
            async fn equal_or_below_chain_height_and_below_sapling() {
                // This case requires that the wallet have a scan_start_below sapling
                // which is an unexpected state.
                let lkch = vec![ScanRange::from_parts(
                    BlockHeight::from_u32(1)..BlockHeight::from_u32(10),
                    crate::sync::ScanPriority::Scanned,
                )];
                let state = SyncState {
                    scan_ranges: lkch,
                    ..Default::default()
                };
                let builder = crate::mocks::MockWalletBuilder::new();
                let mut _test_wallet = builder.sync_state(state).create_mock_wallet();
            }
            #[ignore = "in progress"]
            #[tokio::test]
            async fn below_sapling() {
                let lkch = vec![ScanRange::from_parts(
                    BlockHeight::from_u32(1)..BlockHeight::from_u32(10),
                    crate::sync::ScanPriority::Scanned,
                )];
                let state = SyncState {
                    scan_ranges: lkch,
                    ..Default::default()
                };
                let builder = crate::mocks::MockWalletBuilder::new();
                let mut _test_wallet = builder.sync_state(state).create_mock_wallet();
            }
        }
        mod no_last_known_chain_height {
            use super::*;
            // If there are know scan_ranges in the SyncState
            #[tokio::test]
            async fn get_bday_error() {
                let test_error = "get_bday_error";
                let builder = crate::mocks::MockWalletBuilder::new();
                let mut test_wallet = builder
                    .get_birthday_patch(Box::new(|_| {
                        Err(crate::mocks::MockWalletError::AnErrorVariant(
                            test_error.to_string(),
                        ))
                    }))
                    .create_mock_wallet();
                let res = checked_wallet_height(
                    &mut test_wallet,
                    BlockHeight::from_u32(1),
                    &LOCAL_NETWORK,
                );
                assert!(matches!(
                    res,
                    Err(SyncError::WalletError(
                        crate::mocks::MockWalletError::AnErrorVariant(ref s)
                    )) if s == test_error
                ));
            }
            #[ignore = "in progress"]
            #[tokio::test]
            async fn raw_bday_above_chain_height() {
                let builder = crate::mocks::MockWalletBuilder::new();
                let mut test_wallet = builder
                    .birthday(BlockHeight::from_u32(15))
                    .create_mock_wallet();
                let res = checked_wallet_height(
                    &mut test_wallet,
                    BlockHeight::from_u32(1),
                    &LOCAL_NETWORK,
                );
                if let Err(e) = res {
                    assert_eq!(
                        e.to_string(),
                        format!(
                            "wallet height is more than {} blocks ahead of best chain height",
                            15 - 1
                        )
                    );
                } else {
                    panic!()
                }
            }
            mod sapling_height {
                use super::*;
                #[tokio::test]
                async fn raw_bday_above() {
                    let builder = crate::mocks::MockWalletBuilder::new();
                    let mut test_wallet = builder
                        .birthday(BlockHeight::from_u32(4))
                        .create_mock_wallet();
                    let res = checked_wallet_height(
                        &mut test_wallet,
                        BlockHeight::from_u32(5),
                        &LOCAL_NETWORK,
                    );
                    assert_eq!(res.unwrap(), BlockHeight::from_u32(4 - 1));
                }
                #[tokio::test]
                async fn raw_bday_equal() {
                    let builder = crate::mocks::MockWalletBuilder::new();
                    let mut test_wallet = builder
                        .birthday(BlockHeight::from_u32(3))
                        .create_mock_wallet();
                    let res = checked_wallet_height(
                        &mut test_wallet,
                        BlockHeight::from_u32(5),
                        &LOCAL_NETWORK,
                    );
                    assert_eq!(res.unwrap(), BlockHeight::from_u32(3 - 1));
                }
                #[tokio::test]
                async fn raw_bday_below() {
                    let builder = crate::mocks::MockWalletBuilder::new();
                    let mut test_wallet = builder
                        .birthday(BlockHeight::from_u32(1))
                        .create_mock_wallet();
                    let res = checked_wallet_height(
                        &mut test_wallet,
                        BlockHeight::from_u32(5),
                        &LOCAL_NETWORK,
                    );
                    assert!(matches!(res, Err(SyncError::BirthdayBelowSapling(1, 3))));
                }
            }
        }
    }
}
