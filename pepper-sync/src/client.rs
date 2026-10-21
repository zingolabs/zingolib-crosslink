//! Module for handling all connections to the server

use std::{
    collections::BTreeMap,
    ops::Range,
    sync::{
        Arc, Mutex,
        atomic::{self, AtomicBool},
    },
    time::Duration,
};

use tokio::sync::{mpsc::UnboundedSender, oneshot};

use zcash_client_backend::proto::{
    compact_formats::CompactBlock,
    service::{
        BlockId, GetAddressUtxosReply, RawTransaction, TreeState,
        compact_tx_streamer_client::CompactTxStreamerClient,
    },
};
use zcash_primitives::transaction::{Transaction, TxId};
use zcash_protocol::{
    PoolType, ShieldedPool,
    consensus::{self, BlockHeight},
};

#[cfg(not(feature = "darkside_test"))]
use zcash_client_backend::proto::service::SubtreeRoot;

use crate::error::{MempoolError, ServerError};

pub(crate) mod fetch;

const MAX_RETRIES: u8 = 3;

const FETCH_REPLY_TIMEOUT: Duration = Duration::from_secs(10);
const STREAM_MSG_TIMEOUT: Duration = Duration::from_secs(15);

async fn recv_fetch_reply<T>(
    rx: oneshot::Receiver<Result<T, tonic::Status>>,
    what: &'static str,
) -> Result<T, ServerError> {
    match tokio::time::timeout(FETCH_REPLY_TIMEOUT, rx).await {
        Ok(res) => {
            let inner = res.map_err(|_| ServerError::FetcherDropped)?;
            inner.map_err(Into::into)
        }
        Err(_) => {
            Err(tonic::Status::deadline_exceeded(format!("fetch {what} reply timeout")).into())
        }
    }
}

async fn next_stream_item<T>(
    stream: &mut tonic::Streaming<T>,
    what: &'static str,
) -> Result<Option<T>, tonic::Status> {
    match tokio::time::timeout(STREAM_MSG_TIMEOUT, stream.message()).await {
        Ok(res) => res,
        Err(_) => Err(tonic::Status::deadline_exceeded(format!(
            "{what} stream message timeout"
        ))),
    }
}

/// Fetch requests are created and sent to the [`crate::client::fetch::fetch`] task when a connection to the server is required.
///
/// Each variant includes a [`tokio::sync::oneshot::Sender`] for returning the fetched data to the requester.
#[derive(Debug)]
pub enum FetchRequest {
    /// Gets the height of the blockchain from the server.
    ChainTip(oneshot::Sender<Result<BlockId, tonic::Status>>),
    /// Gets  a compact block of the given block height.
    CompactBlock(
        oneshot::Sender<Result<CompactBlock, tonic::Status>>,
        BlockHeight,
    ),
    /// Gets the specified range of compact blocks from the server (end exclusive).
    CompactBlockRange(
        oneshot::Sender<Result<tonic::Streaming<CompactBlock>, tonic::Status>>,
        Range<BlockHeight>,
    ),
    /// Gets the specified range of nullifiers from the server (end exclusive).
    NullifierRange(
        oneshot::Sender<Result<tonic::Streaming<CompactBlock>, tonic::Status>>,
        Range<BlockHeight>,
    ),
    /// Gets the tree states for a specified block height.
    TreeState(
        oneshot::Sender<Result<TreeState, tonic::Status>>,
        BlockHeight,
    ),
    /// Get a full transaction by txid.
    Transaction(oneshot::Sender<Result<RawTransaction, tonic::Status>>, TxId),
    /// Get a list of unspent transparent output metadata for a given list of transparent addresses and start height.
    #[allow(dead_code)]
    UtxoMetadata(
        oneshot::Sender<Result<Vec<GetAddressUtxosReply>, tonic::Status>>,
        (Vec<String>, BlockHeight),
    ),
    /// Get a list of transactions for a given transparent address and block range.
    TransparentAddressTxs(
        oneshot::Sender<Result<tonic::Streaming<RawTransaction>, tonic::Status>>,
        (String, Range<BlockHeight>),
    ),
    /// Get a stream of shards.
    #[cfg(not(feature = "darkside_test"))]
    SubtreeRoots(
        oneshot::Sender<Result<tonic::Streaming<SubtreeRoot>, tonic::Status>>,
        u32,
        i32,
        u32,
    ),
}

/// Gets the height of the blockchain from the server.
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub(crate) async fn get_chain_height(
    fetch_request_sender: UnboundedSender<FetchRequest>,
) -> Result<BlockHeight, ServerError> {
    let (reply_sender, reply_receiver) = oneshot::channel();
    fetch_request_sender
        .send(FetchRequest::ChainTip(reply_sender))
        .map_err(|_| ServerError::FetcherDropped)?;
    let chain_tip = match tokio::time::timeout(FETCH_REPLY_TIMEOUT, reply_receiver).await {
        Ok(res) => res.map_err(|_| ServerError::FetcherDropped)??,
        Err(_) => {
            return Err(tonic::Status::deadline_exceeded("fetch ChainTip reply timeout").into());
        }
    };

    Ok(BlockHeight::from_u32(chain_tip.height as u32))
}

/// Gets the specified range of compact blocks from the server (end exclusive).
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub(crate) async fn get_compact_block(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    block_height: BlockHeight,
) -> Result<CompactBlock, ServerError> {
    let (reply_sender, reply_receiver) = oneshot::channel();
    fetch_request_sender
        .send(FetchRequest::CompactBlock(reply_sender, block_height))
        .map_err(|_| ServerError::FetcherDropped)?;

    let block = match tokio::time::timeout(FETCH_REPLY_TIMEOUT, reply_receiver).await {
        Ok(res) => res.map_err(|_| ServerError::FetcherDropped)??,
        Err(_) => {
            return Err(
                tonic::Status::deadline_exceeded("fetch CompactBlock reply timeout").into(),
            );
        }
    };

    Ok(block)
}

/// Gets the specified range of compact blocks from the server (end exclusive).
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub(crate) async fn get_compact_block_range(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    block_range: Range<BlockHeight>,
) -> Result<tonic::Streaming<CompactBlock>, ServerError> {
    let (reply_sender, reply_receiver) = oneshot::channel();
    fetch_request_sender
        .send(FetchRequest::CompactBlockRange(reply_sender, block_range))
        .map_err(|_| ServerError::FetcherDropped)?;

    let block_stream = match tokio::time::timeout(FETCH_REPLY_TIMEOUT, reply_receiver).await {
        Ok(res) => res.map_err(|_| ServerError::FetcherDropped)??,
        Err(_) => {
            return Err(
                tonic::Status::deadline_exceeded("fetch CompactBlockRange reply timeout").into(),
            );
        }
    };

    Ok(block_stream)
}

/// Gets the specified range of nullifiers from the server (end exclusive).
///
/// Nullifiers are stored in compact blocks where the actions contain only nullifiers.
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub(crate) async fn get_nullifier_range(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    block_range: Range<BlockHeight>,
) -> Result<tonic::Streaming<CompactBlock>, ServerError> {
    let (reply_sender, reply_receiver) = oneshot::channel();
    fetch_request_sender
        .send(FetchRequest::NullifierRange(reply_sender, block_range))
        .map_err(|_| ServerError::FetcherDropped)?;

    let block_stream = match tokio::time::timeout(FETCH_REPLY_TIMEOUT, reply_receiver).await {
        Ok(res) => res.map_err(|_| ServerError::FetcherDropped)??,
        Err(_) => {
            return Err(
                tonic::Status::deadline_exceeded("fetch NullifierRange reply timeout").into(),
            );
        }
    };

    Ok(block_stream)
}

/// Gets the stream of shards (subtree roots)
/// from the server.
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
#[cfg(not(feature = "darkside_test"))]
pub(crate) async fn get_subtree_roots(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    mut start_index: u32,
    shielded_protocol: i32,
    max_entries: u32,
) -> Result<Vec<SubtreeRoot>, ServerError> {
    let mut subtree_roots = Vec::new();
    let mut retry_count = 0;

    'retry: loop {
        let (reply_sender, reply_receiver) = oneshot::channel();

        fetch_request_sender
            .send(FetchRequest::SubtreeRoots(
                reply_sender,
                start_index,
                shielded_protocol,
                max_entries,
            ))
            .map_err(|_| ServerError::FetcherDropped)?;

        let mut subtree_root_stream = match recv_fetch_reply(reply_receiver, "SubtreeRoots").await {
            Ok(stream) => stream,
            Err(ServerError::RequestFailed(status))
                if subtree_roots_unserved(shielded_protocol, &status) =>
            {
                return Ok(Vec::new());
            }
            Err(e) => return Err(e),
        };

        while let Some(subtree_root) =
            match next_stream_item(&mut subtree_root_stream, "SubtreeRoots").await {
                Ok(s) => s,
                Err(e)
                    if (e.code() == tonic::Code::DeadlineExceeded
                        || e.message().contains("Unexpected EOF decoding stream."))
                        && retry_count < MAX_RETRIES =>
                {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    retry_count += 1;
                    continue 'retry;
                }
                Err(e) if subtree_roots_unserved(shielded_protocol, &e) => {
                    return Ok(Vec::new());
                }
                Err(e) => return Err(e.into()),
            }
        {
            subtree_roots.push(subtree_root);
            start_index += 1;
        }

        break 'retry;
    }

    Ok(subtree_roots)
}

/// The `ShieldedProtocol` wire value of the Ironwood pool.
#[cfg(not(feature = "darkside_test"))]
const IRONWOOD_SHIELDED_PROTOCOL: i32 = 2;

/// Whether `status` says the server does not serve subtree roots for `shielded_protocol`.
///
/// Only tolerated for Ironwood: the crosslink node rejects the pool with `InvalidArgument`, and
/// the wallet can still build the Ironwood tree from scanned blocks alone when its birthday is
/// the pool's activation height.
#[cfg(not(feature = "darkside_test"))]
fn subtree_roots_unserved(shielded_protocol: i32, status: &tonic::Status) -> bool {
    let unserved = shielded_protocol == IRONWOOD_SHIELDED_PROTOCOL
        && matches!(
            status.code(),
            tonic::Code::InvalidArgument | tonic::Code::Unimplemented
        );
    if unserved {
        tracing::warn!(
            "server does not serve Ironwood subtree roots ({}). the Ironwood tree will be built from scanned blocks alone.",
            status.message()
        );
    }
    unserved
}

/// Gets the raw tree state for a specified block height.
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub(crate) async fn get_tree_state(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    block_height: BlockHeight,
) -> Result<TreeState, ServerError> {
    let (reply_sender, reply_receiver) = oneshot::channel();
    fetch_request_sender
        .send(FetchRequest::TreeState(reply_sender, block_height))
        .map_err(|_| ServerError::FetcherDropped)?;

    recv_fetch_reply(reply_receiver, "TreeState").await
}

/// Whether `tree_state` omits the tree of `pool` it should carry.
///
/// Servers omit a pool's tree state below the pool's activation height. At and above the activation height an
/// empty tree is served as the serialised empty commitment tree, never as an empty string, so an omitted tree at
/// an active height means the server does not serve the pool. The crosslink node serves no Ironwood tree state.
pub(crate) fn tree_state_omitted(
    consensus_parameters: &impl consensus::Parameters,
    tree_state: &TreeState,
    pool: ShieldedPool,
    block_height: BlockHeight,
) -> bool {
    let (tree, network_upgrade) = match pool {
        ShieldedPool::Sapling => (&tree_state.sapling_tree, consensus::NetworkUpgrade::Sapling),
        ShieldedPool::Orchard => (&tree_state.orchard_tree, consensus::NetworkUpgrade::Nu5),
        ShieldedPool::Ironwood => (&tree_state.ironwood_tree, consensus::NetworkUpgrade::Nu6_3),
    };

    tree.is_empty() && consensus_parameters.is_nu_active(network_upgrade, block_height)
}

/// Note commitment tree sizes at the end of a block, as served by the server's tree state.
///
/// `ironwood` is `None` where the server omits the Ironwood tree at a height the pool is active. Resolve it
/// through [`IronwoodSizes::resolve`], or use [`get_final_tree_sizes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TreeSizes {
    pub(crate) sapling: u32,
    pub(crate) orchard: u32,
    pub(crate) ironwood: Option<u32>,
}

/// Gets the note commitment tree sizes at the end of `block_height` from the server's tree state.
///
/// An omitted Sapling or Orchard tree at or above the pool's activation height is
/// [`ServerError::TreeStateNotServed`]. An omitted Ironwood tree at an active height is tolerated as
/// `ironwood: None`, so callers that only need Sapling or Orchard sizes do no Ironwood work at all.
///
/// Heights below Sapling activation have empty trees.
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub(crate) async fn get_tree_sizes(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    consensus_parameters: &impl consensus::Parameters,
    block_height: BlockHeight,
) -> Result<TreeSizes, ServerError> {
    let sapling_activation_height = consensus_parameters
        .activation_height(consensus::NetworkUpgrade::Sapling)
        .expect("should have some sapling activation height");
    if block_height < sapling_activation_height {
        return Ok(TreeSizes {
            sapling: 0,
            orchard: 0,
            ironwood: Some(0),
        });
    }

    let tree_state = get_tree_state(fetch_request_sender, block_height).await?;
    for pool in [ShieldedPool::Sapling, ShieldedPool::Orchard] {
        if tree_state_omitted(consensus_parameters, &tree_state, pool, block_height) {
            return Err(ServerError::TreeStateNotServed {
                pool: PoolType::Shielded(pool),
                height: block_height,
            });
        }
    }
    let ironwood_omitted = tree_state_omitted(
        consensus_parameters,
        &tree_state,
        ShieldedPool::Ironwood,
        block_height,
    );
    let chain_state = tree_state
        .to_chain_state()
        .map_err(ServerError::InvalidFrontier)?;

    let size_of = |tree_size: u64| -> u32 {
        tree_size
            .try_into()
            .expect("should not be more than 2^32 note commitments in the tree!")
    };
    Ok(TreeSizes {
        sapling: size_of(chain_state.final_sapling_tree().tree_size()),
        orchard: size_of(chain_state.final_orchard_tree().tree_size()),
        ironwood: (!ironwood_omitted)
            .then(|| size_of(chain_state.final_ironwood_tree().tree_size())),
    })
}

/// Gets the `(sapling, orchard, ironwood)` note commitment tree sizes at the end of `block_height`.
///
/// Sapling and Orchard sizes come from the server's tree state. The Ironwood size does too where it is served;
/// otherwise it is resolved through `ironwood_sizes` (see [`IronwoodSizes::resolve`]).
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub(crate) async fn get_final_tree_sizes(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    consensus_parameters: &impl consensus::Parameters,
    ironwood_sizes: &IronwoodSizes,
    block_height: BlockHeight,
) -> Result<(u32, u32, u32), ServerError> {
    let tree_sizes = get_tree_sizes(
        fetch_request_sender.clone(),
        consensus_parameters,
        block_height,
    )
    .await?;
    let ironwood = ironwood_sizes
        .resolve(
            fetch_request_sender,
            consensus_parameters,
            block_height,
            tree_sizes.ironwood,
        )
        .await?;

    Ok((tree_sizes.sapling, tree_sizes.orchard, ironwood))
}

/// Ironwood note commitment tree sizes known to a sync session, for servers that serve no Ironwood tree state.
///
/// Keyed by block height, each entry is the size of the Ironwood tree at the end of that block. Entries come from
/// the wallet's stored blocks, from the blocks fetched for scanning, and from the Ironwood actions counted from the
/// server's nullifier stream when a size is needed for a height no entry covers (see [`Self::resolve`]). Counting
/// only ever covers the gap above the nearest known height, so the whole chain is streamed at most once per
/// session, and never when the server serves the Ironwood tree state.
///
/// Cloning shares the same store.
#[derive(Debug, Clone, Default)]
pub(crate) struct IronwoodSizes {
    sizes: Arc<Mutex<BTreeMap<BlockHeight, u32>>>,
}

impl IronwoodSizes {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<BlockHeight, u32>> {
        self.sizes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The known Ironwood tree size at the greatest height at or below `block_height`, if any.
    pub(crate) fn floor(&self, block_height: BlockHeight) -> Option<(BlockHeight, u32)> {
        self.lock()
            .range(..=block_height)
            .next_back()
            .map(|(height, size)| (*height, *size))
    }

    /// Records the Ironwood tree size at the end of the block at `block_height`.
    pub(crate) fn record(&self, block_height: BlockHeight, size: u32) {
        self.lock().insert(block_height, size);
    }

    /// Records the Ironwood tree sizes at the end of each block in `sizes`.
    pub(crate) fn extend(&self, sizes: impl IntoIterator<Item = (BlockHeight, u32)>) {
        self.lock().extend(sizes);
    }

    /// Forgets every size above `block_height`, as after a re-org truncation.
    pub(crate) fn truncate_above(&self, block_height: BlockHeight) {
        let mut sizes = self.lock();
        let above = sizes.split_off(&(block_height + 1));
        drop(above);
    }

    /// The Ironwood tree size at the end of `block_height`.
    ///
    /// `served` is the size from the server's tree state and is returned as is when present. Otherwise, the size
    /// is the nearest known size at or below `block_height` plus the Ironwood actions in the blocks between,
    /// counted from the server's nullifier stream and remembered for every block counted. Heights below the
    /// Ironwood activation height have an empty tree.
    ///
    /// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
    pub(crate) async fn resolve(
        &self,
        fetch_request_sender: UnboundedSender<FetchRequest>,
        consensus_parameters: &impl consensus::Parameters,
        block_height: BlockHeight,
        served: Option<u32>,
    ) -> Result<u32, ServerError> {
        if let Some(size) = served {
            return Ok(size);
        }
        let Some(ironwood_activation_height) =
            consensus_parameters.activation_height(consensus::NetworkUpgrade::Nu6_3)
        else {
            return Ok(0);
        };
        if block_height < ironwood_activation_height {
            return Ok(0);
        }

        let (count_from, mut size) = match self.floor(block_height) {
            Some((height, size)) if height == block_height => return Ok(size),
            Some((height, size)) => (height + 1, size),
            None => (ironwood_activation_height, 0),
        };
        tracing::info!(
            "server serves no Ironwood tree state at height {block_height}. counting the Ironwood actions in blocks {count_from}..={block_height} instead."
        );

        let mut block_stream =
            get_nullifier_range(fetch_request_sender, count_from..block_height + 1).await?;
        let mut counted = Vec::new();
        while let Some(block) = next_stream_item(&mut block_stream, "NullifierRange").await? {
            let block_count: u32 = block
                .vtx
                .iter()
                .map(|tx| tx.ironwood_actions.len())
                .sum::<usize>()
                .try_into()
                .expect("Ironwood output count cannot exceed a u32");
            size += block_count;
            counted.push((block.height(), size));
        }
        if counted.last().map(|(height, _)| *height) != Some(block_height) {
            return Err(tonic::Status::data_loss(format!(
                "nullifier stream for blocks {count_from}..={block_height} ended before block {block_height}"
            ))
            .into());
        }
        self.extend(counted);

        Ok(size)
    }
}

/// Gets a full transaction for a specified txid.
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub(crate) async fn get_transaction_and_block_height(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    consensus_parameters: &impl consensus::Parameters,
    txid: TxId,
) -> Result<(Transaction, BlockHeight), ServerError> {
    let (reply_sender, reply_receiver) = oneshot::channel();
    fetch_request_sender
        .send(FetchRequest::Transaction(reply_sender, txid))
        .map_err(|_| ServerError::FetcherDropped)?;

    let raw_transaction = recv_fetch_reply(reply_receiver, "Transaction").await?;

    let block_height =
        BlockHeight::from_u32(u32::try_from(raw_transaction.height).expect("should be valid u32"));

    let transaction = Transaction::read(
        &raw_transaction.data[..],
        consensus::BranchId::for_height(consensus_parameters, block_height),
    )
    .map_err(ServerError::InvalidTransaction)?;

    Ok((transaction, block_height))
}

/// Gets unspent transparent output metadata for a list of `transparent addresses` from the specified `start_height`.
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
#[allow(dead_code)]
pub(crate) async fn get_utxo_metadata(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    transparent_addresses: Vec<String>,
    start_height: BlockHeight,
) -> Result<Vec<GetAddressUtxosReply>, ServerError> {
    if transparent_addresses.is_empty() {
        return Ok(Vec::new());
    }

    let (reply_sender, reply_receiver) = oneshot::channel();

    fetch_request_sender
        .send(FetchRequest::UtxoMetadata(
            reply_sender,
            (transparent_addresses, start_height),
        ))
        .map_err(|_| ServerError::FetcherDropped)?;

    recv_fetch_reply(reply_receiver, "UtxoMetadata").await
}

/// Gets transactions relevant to a given `transparent address` in the specified `block_range`.
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub(crate) async fn get_transparent_address_transactions(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    consensus_parameters: &impl consensus::Parameters,
    transparent_address: String,
    block_range: Range<BlockHeight>,
) -> Result<Vec<(BlockHeight, Transaction)>, ServerError> {
    let mut raw_transactions: Vec<RawTransaction> = Vec::new();
    let mut retry_count = 0;

    'retry: loop {
        let (reply_sender, reply_receiver) = oneshot::channel();

        fetch_request_sender
            .send(FetchRequest::TransparentAddressTxs(
                reply_sender,
                (transparent_address.clone(), block_range.clone()),
            ))
            .map_err(|_| ServerError::FetcherDropped)?;

        let mut raw_transaction_stream =
            recv_fetch_reply(reply_receiver, "TransparentAddressTxs").await?;

        while let Some(raw_tx) =
            match next_stream_item(&mut raw_transaction_stream, "TransparentAddressTxs").await {
                Ok(s) => s,
                Err(e)
                    if (e.code() == tonic::Code::DeadlineExceeded
                        || e.message().contains("Unexpected EOF decoding stream."))
                        && retry_count < MAX_RETRIES =>
                {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    retry_count += 1;
                    raw_transactions.clear();
                    continue 'retry;
                }
                Err(e) => return Err(e.into()),
            }
        {
            raw_transactions.push(raw_tx);
        }

        break 'retry;
    }

    let transactions = raw_transactions
        .into_iter()
        .map(|raw_transaction| {
            let block_height = BlockHeight::from_u32(
                u32::try_from(raw_transaction.height).expect("should be valid u32"),
            );

            let transaction = Transaction::read(
                &raw_transaction.data[..],
                consensus::BranchId::for_height(consensus_parameters, block_height),
            )
            .map_err(ServerError::InvalidTransaction)?;

            Ok((block_height, transaction))
        })
        .collect::<Result<Vec<(BlockHeight, Transaction)>, ServerError>>()?;

    Ok(transactions)
}

/// Gets stream of mempool transactions until the next block is mined.
///
/// Checks at intervals if `shutdown_mempool` is set to prevent hanging on awating mempool monitor handle.
pub(crate) async fn get_mempool_transaction_stream(
    client: &mut CompactTxStreamerClient<tonic::transport::Channel>,
    shutdown_mempool: Arc<AtomicBool>,
) -> Result<tonic::Streaming<RawTransaction>, MempoolError> {
    tracing::debug!("Fetching mempool stream");
    let mut interval = tokio::time::interval(Duration::from_secs(3));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    interval.tick().await;
    loop {
        tokio::select! {
            mempool_stream_response = fetch::get_mempool_stream(client) => {
                return mempool_stream_response.map_err(|e| MempoolError::ServerError(ServerError::RequestFailed(e)));
            }

            _ = interval.tick() => {
                if shutdown_mempool.load(atomic::Ordering::Acquire) {
                    return Err(MempoolError::ShutdownWithoutStream);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use zcash_protocol::consensus::BlockHeight;
    use zcash_protocol::local_consensus::LocalNetwork;

    use super::*;

    const IRONWOOD_FROM_TEN: LocalNetwork = LocalNetwork {
        overwinter: Some(BlockHeight::from_u32(1)),
        sapling: Some(BlockHeight::from_u32(1)),
        blossom: Some(BlockHeight::from_u32(1)),
        heartwood: Some(BlockHeight::from_u32(1)),
        canopy: Some(BlockHeight::from_u32(1)),
        nu5: Some(BlockHeight::from_u32(1)),
        nu6: Some(BlockHeight::from_u32(1)),
        nu6_1: Some(BlockHeight::from_u32(1)),
        nu6_2: Some(BlockHeight::from_u32(1)),
        nu6_3: Some(BlockHeight::from_u32(10)),
    };

    fn tree_state(ironwood_tree: &str) -> TreeState {
        TreeState {
            ironwood_tree: ironwood_tree.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn empty_ironwood_tree_is_omitted_only_once_the_pool_is_active() {
        assert!(tree_state_omitted(
            &IRONWOOD_FROM_TEN,
            &tree_state(""),
            ShieldedPool::Ironwood,
            BlockHeight::from_u32(10)
        ));
        assert!(!tree_state_omitted(
            &IRONWOOD_FROM_TEN,
            &tree_state(""),
            ShieldedPool::Ironwood,
            BlockHeight::from_u32(9)
        ));
    }

    #[test]
    fn served_empty_ironwood_tree_is_not_omitted() {
        assert!(!tree_state_omitted(
            &IRONWOOD_FROM_TEN,
            &tree_state("000000"),
            ShieldedPool::Ironwood,
            BlockHeight::from_u32(10)
        ));
    }

    /// Serialized empty commitment tree, as served at and above a pool's activation height.
    const EMPTY_TREE: &str = "000000";

    /// Answers tree state requests with an empty tree for every pool except `omitted_pool`, whose tree state
    /// field is left empty.
    fn spawn_tree_state_fetcher(omitted_pool: ShieldedPool) -> UnboundedSender<FetchRequest> {
        let tree = |pool: ShieldedPool| {
            if pool == omitted_pool {
                String::new()
            } else {
                EMPTY_TREE.to_string()
            }
        };
        let (sapling_tree, orchard_tree, ironwood_tree) = (
            tree(ShieldedPool::Sapling),
            tree(ShieldedPool::Orchard),
            tree(ShieldedPool::Ironwood),
        );
        let (fetch_request_sender, mut fetch_request_receiver) =
            tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(fetch_request) = fetch_request_receiver.recv().await {
                match fetch_request {
                    FetchRequest::TreeState(reply_sender, block_height) => {
                        let _ignore_error = reply_sender.send(Ok(TreeState {
                            height: u64::from(block_height),
                            hash: "00".repeat(32),
                            sapling_tree: sapling_tree.clone(),
                            orchard_tree: orchard_tree.clone(),
                            ironwood_tree: ironwood_tree.clone(),
                            ..Default::default()
                        }));
                    }
                    _ => panic!("unexpected fetch request"),
                }
            }
        });

        fetch_request_sender
    }

    const SAPLING_ACTIVATION: u32 = 100;
    const ORCHARD_ACTIVATION: u32 = 200;
    const IRONWOOD_ACTIVATION: u32 = 300;

    const STAGGERED_NETWORK: LocalNetwork = LocalNetwork {
        overwinter: Some(BlockHeight::from_u32(1)),
        sapling: Some(BlockHeight::from_u32(SAPLING_ACTIVATION)),
        blossom: Some(BlockHeight::from_u32(SAPLING_ACTIVATION)),
        heartwood: Some(BlockHeight::from_u32(SAPLING_ACTIVATION)),
        canopy: Some(BlockHeight::from_u32(SAPLING_ACTIVATION)),
        nu5: Some(BlockHeight::from_u32(ORCHARD_ACTIVATION)),
        nu6: Some(BlockHeight::from_u32(ORCHARD_ACTIVATION)),
        nu6_1: Some(BlockHeight::from_u32(ORCHARD_ACTIVATION)),
        nu6_2: Some(BlockHeight::from_u32(ORCHARD_ACTIVATION)),
        nu6_3: Some(BlockHeight::from_u32(IRONWOOD_ACTIVATION)),
    };

    /// An omitted Sapling or Orchard tree state at or above the pool's activation height means the server does
    /// not serve the pool, which no fallback can make up for.
    #[tokio::test]
    async fn omitted_sapling_or_orchard_tree_state_is_rejected_at_or_above_activation() {
        for (pool, activation_height) in [
            (ShieldedPool::Sapling, SAPLING_ACTIVATION),
            (ShieldedPool::Orchard, ORCHARD_ACTIVATION),
        ] {
            // below sapling activation no tree state is requested; below orchard activation the omission is expected.
            get_tree_sizes(
                spawn_tree_state_fetcher(pool),
                &STAGGERED_NETWORK,
                (activation_height - 1).into(),
            )
            .await
            .unwrap();

            for height in [activation_height, activation_height + 1] {
                assert!(matches!(
                    get_tree_sizes(spawn_tree_state_fetcher(pool), &STAGGERED_NETWORK, height.into()).await,
                    Err(ServerError::TreeStateNotServed { pool: PoolType::Shielded(p), height: h })
                        if p == pool && h == height.into()
                ));
            }
        }
    }

    /// An omitted Ironwood tree state is tolerated: the size is left unresolved for [`IronwoodSizes`] rather than
    /// failing sync, as the crosslink node serves none while the pool is active.
    #[tokio::test]
    async fn omitted_ironwood_tree_state_leaves_the_ironwood_size_unresolved() {
        let below = get_tree_sizes(
            spawn_tree_state_fetcher(ShieldedPool::Ironwood),
            &STAGGERED_NETWORK,
            (IRONWOOD_ACTIVATION - 1).into(),
        )
        .await
        .unwrap();
        assert_eq!(below.ironwood, Some(0));

        let at = get_tree_sizes(
            spawn_tree_state_fetcher(ShieldedPool::Ironwood),
            &STAGGERED_NETWORK,
            IRONWOOD_ACTIVATION.into(),
        )
        .await
        .unwrap();
        assert_eq!(
            at,
            TreeSizes {
                sapling: 0,
                orchard: 0,
                ironwood: None
            }
        );
    }

    /// A served Ironwood size needs no counting and is not remembered, so the store stays empty on servers that
    /// serve the pool.
    #[tokio::test]
    async fn served_ironwood_size_is_returned_without_counting() {
        let sizes = IronwoodSizes::new();
        let (fetch_request_sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let size = sizes
            .resolve(
                fetch_request_sender,
                &IRONWOOD_FROM_TEN,
                BlockHeight::from_u32(20),
                Some(7),
            )
            .await
            .unwrap();
        assert_eq!(size, 7);
        assert_eq!(sizes.floor(BlockHeight::from_u32(20)), None);
    }

    /// Below the Ironwood activation height the tree is empty, served or not.
    #[tokio::test]
    async fn ironwood_size_below_activation_is_zero() {
        let sizes = IronwoodSizes::new();
        let (fetch_request_sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let size = sizes
            .resolve(
                fetch_request_sender,
                &IRONWOOD_FROM_TEN,
                BlockHeight::from_u32(9),
                None,
            )
            .await
            .unwrap();
        assert_eq!(size, 0);
    }

    /// A height with a known size is answered from the store without touching the server.
    #[tokio::test]
    async fn known_ironwood_size_is_resolved_without_the_server() {
        let sizes = IronwoodSizes::new();
        sizes.record(BlockHeight::from_u32(15), 4);
        // no fetcher is running: any request would fail with `FetcherDropped`.
        let (fetch_request_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        drop(receiver);
        let size = sizes
            .resolve(
                fetch_request_sender,
                &IRONWOOD_FROM_TEN,
                BlockHeight::from_u32(15),
                None,
            )
            .await
            .unwrap();
        assert_eq!(size, 4);
    }

    #[test]
    fn floor_finds_the_nearest_known_height_at_or_below() {
        let sizes = IronwoodSizes::new();
        sizes.extend([
            (BlockHeight::from_u32(10), 1),
            (BlockHeight::from_u32(20), 5),
            (BlockHeight::from_u32(30), 9),
        ]);
        assert_eq!(sizes.floor(BlockHeight::from_u32(9)), None);
        assert_eq!(
            sizes.floor(BlockHeight::from_u32(10)),
            Some((BlockHeight::from_u32(10), 1))
        );
        assert_eq!(
            sizes.floor(BlockHeight::from_u32(25)),
            Some((BlockHeight::from_u32(20), 5))
        );
        assert_eq!(
            sizes.floor(BlockHeight::from_u32(100)),
            Some((BlockHeight::from_u32(30), 9))
        );
    }

    /// A re-org truncation forgets the sizes of the blocks it removes and keeps the rest.
    #[test]
    fn truncate_above_forgets_only_higher_heights() {
        let sizes = IronwoodSizes::new();
        sizes.extend([
            (BlockHeight::from_u32(10), 1),
            (BlockHeight::from_u32(20), 5),
            (BlockHeight::from_u32(30), 9),
        ]);
        sizes.truncate_above(BlockHeight::from_u32(20));
        assert_eq!(
            sizes.floor(BlockHeight::from_u32(100)),
            Some((BlockHeight::from_u32(20), 5))
        );
        sizes.truncate_above(BlockHeight::from_u32(5));
        assert_eq!(sizes.floor(BlockHeight::from_u32(100)), None);
    }

    #[cfg(not(feature = "darkside_test"))]
    #[test]
    fn only_ironwood_subtree_root_rejections_are_tolerated() {
        let rejected = tonic::Status::invalid_argument("unknown shielded protocol");
        assert!(subtree_roots_unserved(
            IRONWOOD_SHIELDED_PROTOCOL,
            &rejected
        ));
        assert!(subtree_roots_unserved(
            IRONWOOD_SHIELDED_PROTOCOL,
            &tonic::Status::unimplemented("")
        ));
        assert!(!subtree_roots_unserved(1, &rejected));
        assert!(!subtree_roots_unserved(
            IRONWOOD_SHIELDED_PROTOCOL,
            &tonic::Status::internal("")
        ));
    }
}
