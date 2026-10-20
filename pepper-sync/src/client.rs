//! Module for handling all connections to the server

use std::{
    ops::Range,
    sync::{
        Arc,
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
use zcash_protocol::consensus::{self, BlockHeight};

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

/// Whether `tree_state` lacks the Ironwood tree it should carry.
///
/// A server encodes an empty-but-active tree as the serialised empty commitment tree, never as
/// an empty string, so an empty `ironwood_tree` at a height where the pool is active means the
/// server does not serve the Ironwood tree. The crosslink node currently serves none.
pub(crate) fn ironwood_tree_state_missing(
    consensus_parameters: &impl consensus::Parameters,
    tree_state: &TreeState,
    block_height: BlockHeight,
) -> bool {
    tree_state.ironwood_tree.is_empty()
        && consensus_parameters.is_nu_active(consensus::NetworkUpgrade::Nu6_3, block_height)
}

/// Note commitment tree sizes at the end of a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TreeSizes {
    pub(crate) sapling: u32,
    pub(crate) orchard: u32,
    pub(crate) ironwood: u32,
}

/// Gets the note commitment tree sizes at the end of `block_height`.
///
/// Sizes are taken from the server's tree state. When the server serves no Ironwood tree for a
/// height where the pool is active (see [`ironwood_tree_state_missing`]), the Ironwood size is
/// instead counted from the Ironwood actions in every block since the pool's activation, so the
/// wallet still positions its notes correctly.
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
            ironwood: 0,
        });
    }

    let tree_state = get_tree_state(fetch_request_sender.clone(), block_height).await?;
    let counted_ironwood = if ironwood_tree_state_missing(
        consensus_parameters,
        &tree_state,
        block_height,
    ) {
        let ironwood_activation_height = consensus_parameters
            .activation_height(consensus::NetworkUpgrade::Nu6_3)
            .expect("checked active above");
        tracing::warn!(
            "server returned no Ironwood tree state at height {}. counting Ironwood actions from height {} instead.",
            block_height,
            ironwood_activation_height
        );
        Some(
            count_ironwood_actions(
                fetch_request_sender,
                ironwood_activation_height..block_height + 1,
            )
            .await?,
        )
    } else {
        None
    };
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
        ironwood: counted_ironwood
            .unwrap_or_else(|| size_of(chain_state.final_ironwood_tree().tree_size())),
    })
}

/// Counts the Ironwood actions in `block_range` from the server's nullifier stream.
async fn count_ironwood_actions(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    block_range: Range<BlockHeight>,
) -> Result<u32, ServerError> {
    if block_range.is_empty() {
        return Ok(0);
    }

    let mut block_stream = get_nullifier_range(fetch_request_sender, block_range).await?;
    let mut count: u32 = 0;
    while let Some(block) = next_stream_item(&mut block_stream, "NullifierRange").await? {
        let block_count: u32 = block
            .vtx
            .iter()
            .map(|tx| tx.ironwood_actions.len())
            .sum::<usize>()
            .try_into()
            .expect("Ironwood output count cannot exceed a u32");
        count += block_count;
    }

    Ok(count)
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
    fn empty_ironwood_tree_is_missing_only_once_the_pool_is_active() {
        assert!(ironwood_tree_state_missing(
            &IRONWOOD_FROM_TEN,
            &tree_state(""),
            BlockHeight::from_u32(10)
        ));
        assert!(!ironwood_tree_state_missing(
            &IRONWOOD_FROM_TEN,
            &tree_state(""),
            BlockHeight::from_u32(9)
        ));
    }

    #[test]
    fn served_empty_ironwood_tree_is_not_missing() {
        assert!(!ironwood_tree_state_missing(
            &IRONWOOD_FROM_TEN,
            &tree_state("000000"),
            BlockHeight::from_u32(10)
        ));
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
