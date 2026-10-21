use std::{
    cmp,
    collections::{BTreeMap, BTreeSet, HashMap},
};

use incrementalmerkletree::{Marking, Position, Retention};
use orchard::{note_encryption::CompactAction, tree::MerkleHashOrchard};
use sapling_crypto::{Node, note_encryption::CompactOutputDescription};
use tokio::sync::mpsc;
use zcash_client_backend::proto::compact_formats::{
    CompactBlock, CompactOrchardAction, CompactSaplingOutput,
};
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_note_encryption::Domain;
use zcash_primitives::block::BlockHash;
use zcash_protocol::consensus::{self, BlockHeight};
use zip32::AccountId;

use crate::{
    client::{self, FetchRequest, IronwoodSizes},
    error::{ContinuityError, ScanError, ServerError},
    keys::{KeyId, ScanningKeyOps, ScanningKeys},
    wallet::{NullifierMap, OutputId, ScanTarget, TreeBounds, WalletBlock},
    witness::WitnessData,
};

use zcash_protocol::ShieldedPool;

#[cfg(not(feature = "darkside_test"))]
use zcash_protocol::PoolType;

use self::runners::{BatchRunners, DecryptedOutput};

use super::{DecryptedNoteData, InitialScanData, ScanData, collect_nullifiers};

mod runners;

pub(super) fn scan_compact_blocks<P>(
    compact_blocks: Vec<CompactBlock>,
    consensus_parameters: &P,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    initial_scan_data: InitialScanData,
    trial_decrypt_task_size: usize,
) -> Result<ScanData, ScanError>
where
    P: consensus::Parameters + Sync + Send + 'static,
{
    check_continuity(
        &compact_blocks,
        initial_scan_data.start_seam_block.as_ref(),
        initial_scan_data.end_seam_block.as_ref(),
    )?;

    let scanning_keys = ScanningKeys::from_account_ufvks(ufvks.clone());
    let mut runners = trial_decrypt(
        consensus_parameters,
        &scanning_keys,
        &compact_blocks,
        trial_decrypt_task_size,
    )?;

    let mut wallet_blocks: BTreeMap<BlockHeight, WalletBlock> = BTreeMap::new();
    let mut nullifiers = NullifierMap::new();
    let mut decrypted_scan_targets = BTreeSet::new();
    let mut decrypted_note_data = DecryptedNoteData::new();
    let mut witness_data = WitnessData::new(
        Position::from(u64::from(initial_scan_data.sapling_initial_tree_size)),
        Position::from(u64::from(initial_scan_data.orchard_initial_tree_size)),
        Position::from(u64::from(initial_scan_data.ironwood_initial_tree_size)),
    );
    let mut sapling_initial_tree_size;
    let mut orchard_initial_tree_size;
    let mut ironwood_initial_tree_size;
    let mut sapling_final_tree_size = initial_scan_data.sapling_initial_tree_size;
    let mut orchard_final_tree_size = initial_scan_data.orchard_initial_tree_size;
    let mut ironwood_final_tree_size = initial_scan_data.ironwood_initial_tree_size;
    for block in &compact_blocks {
        sapling_initial_tree_size = sapling_final_tree_size;
        orchard_initial_tree_size = orchard_final_tree_size;
        ironwood_initial_tree_size = ironwood_final_tree_size;

        let block_height = block.height();

        for transaction in &block.vtx {
            // collect trial decryption results by transaction
            let incoming_sapling_outputs = runners
                .sapling
                .collect_results(block.hash(), transaction.txid());
            let incoming_orchard_outputs = runners
                .orchard
                .collect_results(block.hash(), transaction.txid());
            let incoming_ironwood_outputs = runners
                .ironwood
                .collect_results(block.hash(), transaction.txid());

            // gather the txids of all transactions relevant to the wallet
            // the edge case of transactions that this capability created but did not receive change
            // or create outgoing data is handled when the nullifiers are added and linked
            for output_id in incoming_sapling_outputs.keys() {
                decrypted_scan_targets.insert(ScanTarget {
                    block_height,
                    txid: output_id.txid(),
                    narrow_scan_area: false,
                });
            }
            for output_id in incoming_orchard_outputs.keys() {
                decrypted_scan_targets.insert(ScanTarget {
                    block_height,
                    txid: output_id.txid(),
                    narrow_scan_area: false,
                });
            }
            for output_id in incoming_ironwood_outputs.keys() {
                decrypted_scan_targets.insert(ScanTarget {
                    block_height,
                    txid: output_id.txid(),
                    narrow_scan_area: false,
                });
            }

            collect_nullifiers(&mut nullifiers, block.height(), transaction)?;

            witness_data.sapling_leaves_and_retentions.extend(
                calculate_sapling_leaves_and_retentions(
                    &transaction.outputs,
                    &incoming_sapling_outputs,
                )?,
            );
            witness_data.orchard_leaves_and_retentions.extend(
                calculate_orchard_leaves_and_retentions(
                    &transaction.actions,
                    &incoming_orchard_outputs,
                    || ScanError::InvalidOrchardAction,
                )?,
            );
            witness_data.ironwood_leaves_and_retentions.extend(
                calculate_orchard_leaves_and_retentions(
                    &transaction.ironwood_actions,
                    &incoming_ironwood_outputs,
                    || ScanError::InvalidIronwoodAction,
                )?,
            );

            calculate_nullifiers_and_positions(
                sapling_final_tree_size,
                &scanning_keys.sapling,
                &incoming_sapling_outputs,
                &mut decrypted_note_data.sapling_nullifiers_and_positions,
            );
            calculate_nullifiers_and_positions(
                orchard_final_tree_size,
                &scanning_keys.orchard,
                &incoming_orchard_outputs,
                &mut decrypted_note_data.orchard_nullifiers_and_positions,
            );
            calculate_nullifiers_and_positions(
                ironwood_final_tree_size,
                &scanning_keys.ironwood,
                &incoming_ironwood_outputs,
                &mut decrypted_note_data.ironwood_nullifiers_and_positions,
            );

            sapling_final_tree_size += u32::try_from(transaction.outputs.len())
                .expect("should not be more than 2^32 outputs in a transaction");
            orchard_final_tree_size += u32::try_from(transaction.actions.len())
                .expect("should not be more than 2^32 outputs in a transaction");
            ironwood_final_tree_size += u32::try_from(transaction.ironwood_actions.len())
                .expect("should not be more than 2^32 outputs in a transaction");
        }

        set_checkpoint_retentions(
            block_height,
            &mut witness_data.sapling_leaves_and_retentions,
        );
        set_checkpoint_retentions(
            block_height,
            &mut witness_data.orchard_leaves_and_retentions,
        );
        set_checkpoint_retentions(
            block_height,
            &mut witness_data.ironwood_leaves_and_retentions,
        );

        let wallet_block = WalletBlock {
            block_height: block.height(),
            block_hash: block.hash(),
            prev_hash: block.prev_hash(),
            time: block.time,
            txids: block
                .vtx
                .iter()
                .map(zcash_client_backend::proto::compact_formats::CompactTx::txid)
                .collect(),
            tree_bounds: TreeBounds {
                sapling_initial_tree_size,
                sapling_final_tree_size,
                orchard_initial_tree_size,
                orchard_final_tree_size,
                ironwood_initial_tree_size,
                ironwood_final_tree_size,
            },
        };

        check_tree_size(block, &wallet_block)?;

        wallet_blocks.insert(wallet_block.block_height(), wallet_block);
    }

    Ok(ScanData {
        nullifiers,
        wallet_blocks,
        decrypted_scan_targets,
        decrypted_note_data,
        witness_data,
    })
}

fn trial_decrypt<P>(
    consensus_parameters: &P,
    scanning_keys: &ScanningKeys,
    compact_blocks: &[CompactBlock],
    trial_decrypt_task_size: usize,
) -> Result<BatchRunners<(), (), ()>, ScanError>
where
    P: consensus::Parameters + Send + 'static,
{
    let mut runners = BatchRunners::<(), (), ()>::for_keys(trial_decrypt_task_size, scanning_keys);
    for block in compact_blocks {
        runners
            .add_block(consensus_parameters, block.clone())
            .map_err(ScanError::ZcbScanError)?;
    }
    runners.flush();

    Ok(runners)
}

/// Checks height and hash continuity of a batch of compact blocks.
///
/// If available, also checks continuity with the blocks adjacent to the `compact_blocks` forming the start and end
/// seams of the scan ranges.
fn check_continuity(
    compact_blocks: &[CompactBlock],
    start_seam_block: Option<&WalletBlock>,
    end_seam_block: Option<&WalletBlock>,
) -> Result<(), ContinuityError> {
    let mut prev_height: Option<BlockHeight> = None;
    let mut prev_hash: Option<BlockHash> = None;

    if let Some(start_seam_block) = start_seam_block {
        prev_height = Some(start_seam_block.block_height());
        prev_hash = Some(start_seam_block.block_hash());
    }

    for block in compact_blocks {
        if let Some(prev_height) = prev_height
            && block.height() != prev_height + 1
        {
            return Err(ContinuityError::HeightDiscontinuity {
                height: block.height(),
                previous_block_height: prev_height,
            });
        }

        if let Some(prev_hash) = prev_hash
            && block.prev_hash() != prev_hash
        {
            return Err(ContinuityError::HashDiscontinuity {
                height: block.height(),
                prev_hash: block.prev_hash(),
                previous_block_hash: prev_hash,
            });
        }

        prev_height = Some(block.height());
        prev_hash = Some(block.hash());
    }

    if let Some(end_seam_block) = end_seam_block {
        let prev_height = prev_height.expect("compact blocks should not be empty");
        if end_seam_block.block_height() != prev_height + 1 {
            return Err(ContinuityError::HeightDiscontinuity {
                height: end_seam_block.block_height(),
                previous_block_height: prev_height,
            });
        }

        let prev_hash = prev_hash.expect("compact blocks should not be empty");
        if end_seam_block.prev_hash() != prev_hash {
            return Err(ContinuityError::HashDiscontinuity {
                height: end_seam_block.block_height(),
                prev_hash: end_seam_block.prev_hash(),
                previous_block_hash: prev_hash,
            });
        }
    }

    Ok(())
}

/// Checks the tree sizes the wallet calculated for `wallet_block` against the block's chain metadata, where the
/// server sends any.
fn check_tree_size(
    compact_block: &CompactBlock,
    wallet_block: &WalletBlock,
) -> Result<(), ScanError> {
    let Some(chain_metadata) = &compact_block.chain_metadata else {
        return Ok(());
    };
    let tree_bounds = wallet_block.tree_bounds();

    for (pool, metadata_size, calculated_size) in [
        (
            ShieldedPool::Sapling,
            chain_metadata.sapling_commitment_tree_size,
            tree_bounds.sapling_final_tree_size,
        ),
        (
            ShieldedPool::Orchard,
            chain_metadata.orchard_commitment_tree_size,
            tree_bounds.orchard_final_tree_size,
        ),
        (
            ShieldedPool::Ironwood,
            chain_metadata.ironwood_commitment_tree_size,
            tree_bounds.ironwood_final_tree_size,
        ),
    ] {
        if metadata_size == calculated_size {
            continue;
        }

        #[cfg(feature = "darkside_test")]
        {
            tracing::error!(
                "darkside compact block {pool:?} tree size incorrect.\nwallet block: {calculated_size}\ncompact_block: {metadata_size}"
            );
            return Ok(());
        }

        #[cfg(not(feature = "darkside_test"))]
        {
            // block metadata omits a tree size of zero, so a zero tree size where the wallet has calculated a
            // non-zero tree size means the server does not report the tree size of this pool. rescanning would not
            // resolve this.
            if metadata_size == 0 {
                return Err(ScanError::TreeSizeNotReported {
                    shielded_protocol: PoolType::Shielded(pool),
                    height: wallet_block.block_height(),
                    calculated_size,
                });
            }

            return Err(ScanError::IncorrectTreeSize {
                shielded_protocol: PoolType::Shielded(pool),
                block_metadata_size: metadata_size,
                calculated_size,
            });
        }
    }

    Ok(())
}

/// Calculates nullifiers and positions of incoming decrypted outputs for a given compact transaction and insert into hash map
/// `tree_size` is the tree size of the corresponding shielded pool up to - and not including - the compact transaction
/// being processed
fn calculate_nullifiers_and_positions<D, K, Nf>(
    tree_size: u32,
    keys: &HashMap<KeyId, K>,
    incoming_decrypted_outputs: &HashMap<OutputId, DecryptedOutput<D, ()>>,
    nullifiers_and_positions: &mut HashMap<OutputId, (Nf, Position)>,
) where
    D: Domain,
    K: ScanningKeyOps<D, Nf>,
{
    for (output_id, incoming_output) in incoming_decrypted_outputs {
        let position = Position::from(u64::from(tree_size + u32::from(output_id.output_index())));
        let key = keys
            .get(&incoming_output.ivk_tag)
            .expect("key should be available as it was used to decrypt output");
        let nullifier = key
            .nf(&incoming_output.note, position)
            .expect("only fvks currently supported");
        nullifiers_and_positions.insert(*output_id, (nullifier, position));
    }
}

// TODO: unify sapling and orchard leaf and retention fns
/// Calculates the sapling note commitment tree leaves and shardtree retentions for a given compact transaction
fn calculate_sapling_leaves_and_retentions<D: Domain>(
    outputs: &[CompactSaplingOutput],
    incoming_decrypted_outputs: &HashMap<OutputId, DecryptedOutput<D, ()>>,
) -> Result<Vec<(Node, Retention<BlockHeight>)>, ScanError> {
    let incoming_output_indexes = incoming_decrypted_outputs
        .keys()
        .copied()
        .map(|output_id| output_id.output_index())
        .collect::<Vec<_>>();

    if outputs.is_empty() {
        Ok(Vec::new())
    } else {
        let leaves_and_retentions = outputs
            .iter()
            .enumerate()
            .map(|(output_index, output)| {
                let note_commitment = CompactOutputDescription::try_from(output)
                    .map_err(|_| ScanError::InvalidSaplingOutput)?
                    .cmu;
                let leaf = sapling_crypto::Node::from_cmu(&note_commitment);
                let decrypted: bool = incoming_output_indexes.contains(&(output_index as u16));
                let retention = if decrypted {
                    Retention::Marked
                } else {
                    Retention::Ephemeral
                };

                Ok((leaf, retention))
            })
            .collect::<Result<_, ScanError>>()?;

        Ok(leaves_and_retentions)
    }
}

/// Calculates the orchard-family (Orchard or Ironwood) note commitment tree leaves and shardtree retentions for a
/// given compact transaction. `invalid_action_error` builds the error returned if an action fails to parse.
fn calculate_orchard_leaves_and_retentions<D: Domain>(
    actions: &[CompactOrchardAction],
    incoming_decrypted_outputs: &HashMap<OutputId, DecryptedOutput<D, ()>>,
    invalid_action_error: impl Fn() -> ScanError,
) -> Result<Vec<(MerkleHashOrchard, Retention<BlockHeight>)>, ScanError> {
    let incoming_output_indexes = incoming_decrypted_outputs
        .keys()
        .copied()
        .map(|output_id| output_id.output_index())
        .collect::<Vec<_>>();

    if actions.is_empty() {
        Ok(Vec::new())
    } else {
        let leaves_and_retentions = actions
            .iter()
            .enumerate()
            .map(|(output_index, output)| {
                let note_commitment = CompactAction::try_from(output)
                    .map_err(|_| invalid_action_error())?
                    .cmx();
                let leaf = MerkleHashOrchard::from_cmx(&note_commitment);
                let decrypted: bool = incoming_output_indexes.contains(&(output_index as u16));
                let retention = if decrypted {
                    Retention::Marked
                } else {
                    Retention::Ephemeral
                };

                Ok((leaf, retention))
            })
            .collect::<Result<_, ScanError>>()?;

        Ok(leaves_and_retentions)
    }
}

/// Calculates the tree bounds of `compact_block` from its chain metadata or, where the server sends none, from
/// the server's tree state at the block's height. The Ironwood size is resolved through `ironwood_sizes` where
/// the server serves no Ironwood tree state (see [`IronwoodSizes`]).
///
/// Prefer deriving bounds from an adjacent block whose bounds are known
/// ([`TreeBounds::from_initial_tree_sizes`]) where one is at hand, as that needs no server at all.
pub(crate) async fn calculate_block_tree_bounds(
    consensus_parameters: &impl consensus::Parameters,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ironwood_sizes: &IronwoodSizes,
    compact_block: &CompactBlock,
) -> Result<TreeBounds, ServerError> {
    let final_tree_sizes = if let Some(chain_metadata) = compact_block.chain_metadata {
        (
            chain_metadata.sapling_commitment_tree_size,
            chain_metadata.orchard_commitment_tree_size,
            chain_metadata.ironwood_commitment_tree_size,
        )
    } else {
        let sapling_activation_height = consensus_parameters
            .activation_height(consensus::NetworkUpgrade::Sapling)
            .expect("should have some sapling activation height");

        match compact_block.height().cmp(&sapling_activation_height) {
            cmp::Ordering::Greater => {
                client::get_final_tree_sizes(
                    fetch_request_sender,
                    consensus_parameters,
                    ironwood_sizes,
                    compact_block.height(),
                )
                .await?
            }
            cmp::Ordering::Equal => (0, 0, 0),
            cmp::Ordering::Less => panic!("pre-sapling not supported!"),
        }
    };

    Ok(TreeBounds::from_final_tree_sizes(
        compact_block,
        final_tree_sizes,
    ))
}

fn set_checkpoint_retentions<L>(
    block_height: BlockHeight,
    leaves_and_retentions: &mut [(L, Retention<BlockHeight>)],
) {
    if let Some((_leaf, retention)) = leaves_and_retentions.last_mut() {
        match retention {
            Retention::Marked => {
                *retention = Retention::Checkpoint {
                    id: block_height,
                    marking: Marking::Marked,
                };
            }
            Retention::Ephemeral => {
                *retention = Retention::Checkpoint {
                    id: block_height,
                    marking: Marking::None,
                };
            }
            // NOTE: if there are no outputs in the block, this last retention will be a checkpoint and nothing will need to be mutated.
            _ => (),
        }
    }
}

#[cfg(all(test, not(feature = "darkside_test")))]
mod tests {
    use zcash_client_backend::proto::compact_formats::{
        ChainMetadata, CompactBlock, CompactOrchardAction, CompactTx,
    };
    use zcash_primitives::block::BlockHash;

    use super::*;

    /// A block at height 5 carrying `served_actions` Ironwood actions whose chain metadata reports an Ironwood
    /// tree size of `metadata_size`.
    fn block_with_served_ironwood_actions(
        served_actions: usize,
        metadata_size: u32,
    ) -> CompactBlock {
        CompactBlock {
            height: 5,
            vtx: vec![CompactTx {
                ironwood_actions: vec![CompactOrchardAction::default(); served_actions],
                ..Default::default()
            }],
            chain_metadata: Some(ChainMetadata {
                sapling_commitment_tree_size: 0,
                orchard_commitment_tree_size: 0,
                ironwood_commitment_tree_size: metadata_size,
            }),
            ..Default::default()
        }
    }

    fn wallet_block_with_ironwood_size(ironwood_final_tree_size: u32) -> WalletBlock {
        WalletBlock {
            block_height: BlockHeight::from_u32(5),
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
                ironwood_final_tree_size,
            },
        }
    }

    /// A server that does not report a pool's tree size leaves it at zero while the block still carries that
    /// pool's outputs. The wallet's own record is not at fault, so rather than reopening the pool's history for a
    /// rescan that would never resolve it, the scan fails with an error naming the unreported pool.
    #[test]
    fn an_unreported_tree_size_fails_the_scan() {
        let compact_block = block_with_served_ironwood_actions(5, 0);
        let wallet_block = wallet_block_with_ironwood_size(5);

        assert!(matches!(
            check_tree_size(&compact_block, &wallet_block),
            Err(ScanError::TreeSizeNotReported {
                shielded_protocol: PoolType::Shielded(ShieldedPool::Ironwood),
                calculated_size: 5,
                ..
            })
        ));
    }

    /// The wallet's count is cumulative, so against a non-reporting server the mismatch persists onto blocks that
    /// serve no outputs of their own.
    #[test]
    fn an_unreported_tree_size_fails_the_scan_on_blocks_without_outputs() {
        let compact_block = block_with_served_ironwood_actions(0, 0);
        let wallet_block = wallet_block_with_ironwood_size(7);

        assert!(matches!(
            check_tree_size(&compact_block, &wallet_block),
            Err(ScanError::TreeSizeNotReported {
                shielded_protocol: PoolType::Shielded(ShieldedPool::Ironwood),
                calculated_size: 7,
                ..
            })
        ));
    }

    /// Block metadata omits a tree size of zero, as at the Ironwood activation block where the tree is still
    /// empty. A zero where the wallet also calculates zero is an empty tree, not an unreported one.
    #[test]
    fn an_empty_tree_is_not_an_unreported_tree_size() {
        let compact_block = block_with_served_ironwood_actions(0, 0);
        let wallet_block = wallet_block_with_ironwood_size(0);

        assert!(check_tree_size(&compact_block, &wallet_block).is_ok());
    }

    /// A reported size that disagrees with the wallet's record is the wallet's record being wrong.
    #[test]
    fn a_reported_tree_size_that_disagrees_is_incorrect() {
        let compact_block = block_with_served_ironwood_actions(2, 9);
        let wallet_block = wallet_block_with_ironwood_size(7);

        assert!(matches!(
            check_tree_size(&compact_block, &wallet_block),
            Err(ScanError::IncorrectTreeSize {
                shielded_protocol: PoolType::Shielded(ShieldedPool::Ironwood),
                block_metadata_size: 9,
                calculated_size: 7,
            })
        ));
    }

    /// Bounds derived from the block before follow its final sizes, so a chain of blocks needs the server only for
    /// the first.
    #[test]
    fn tree_bounds_follow_from_the_previous_block() {
        let block = block_with_served_ironwood_actions(3, 0);
        let tree_bounds = TreeBounds::from_initial_tree_sizes(&block, (1, 2, 10));
        assert_eq!(tree_bounds.sapling_final_tree_size, 1);
        assert_eq!(tree_bounds.orchard_final_tree_size, 2);
        assert_eq!(tree_bounds.ironwood_initial_tree_size, 10);
        assert_eq!(tree_bounds.ironwood_final_tree_size, 13);

        let from_final = TreeBounds::from_final_tree_sizes(&block, (1, 2, 13));
        assert_eq!(from_final.ironwood_initial_tree_size, 10);
        assert_eq!(from_final.final_tree_sizes(), (1, 2, 13));
    }
}
