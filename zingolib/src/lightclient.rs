//! TODO: Add Mod Description Here!

use std::{
    collections::HashMap,
    fs::File,
    io::{BufReader, Cursor},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8},
    },
};

use json::JsonValue;
use tokio::{sync::RwLock, task::JoinHandle};

use zcash_client_backend::{
    proto::service::{BondInfoRequest, Empty, FaucetRequest},
    tor,
};
use zcash_keys::address::UnifiedAddress;
use zcash_protocol::consensus::{BlockHeight, Parameters};
use zcash_transparent::address::TransparentAddress;

use pepper_sync::{
    error::SyncError,
    keys::transparent::TransparentAddressId,
    sync::SyncResult,
    wallet::{SyncMode, traits::SyncTransactions},
};
use zcash_primitives::transaction::{RosterMember, StakingActionKind};
use zcash_protocol::TxId;
use zingo_netutils::get_client;

use crate::{
    config::{Indexer, ZingoConfig},
    crosslink::BondStatus,
    lightclient::crosslink::{RosterMembers, WalletBond, WalletBonds},
    wallet::{
        LightWallet, SeedDerivation, WalletBase,
        balance::AccountBalance,
        error::{BalanceError, KeyError, SummaryError, WalletError},
        keys::unified::{ReceiverSelection, UnifiedAddressId},
        summary::data::{
            TransactionSummaries, ValueTransfers,
            finsight::{TotalMemoBytesToAddress, TotalSendsToAddress, TotalValueToAddress},
        },
    },
};
use error::LightClientError;

pub mod crosslink;
pub mod error;
pub mod propose;
pub mod save;
pub mod send;
pub mod sync;

/// Struct which owns and manages the [`crate::wallet::LightWallet`]. Responsible for network operations such as
/// storing the indexer URI, creating gRPC clients and syncing the wallet to the blockchain.
///
/// `sync_mode` is an atomic representation of [`pepper_sync::wallet::SyncMode`].
pub struct LightClient {
    // TODO: split zingoconfig so data is not duplicated
    pub config: ZingoConfig,
    /// Tor client
    tor_client: Option<tor::Client>,
    /// Wallet data
    pub wallet: Arc<RwLock<LightWallet>>,
    sync_mode: Arc<AtomicU8>,
    sync_handle: Option<JoinHandle<Result<SyncResult, SyncError<WalletError>>>>,
    save_active: Arc<AtomicBool>,
    save_handle: Option<JoinHandle<std::io::Result<()>>>,
}

/// What a indexer reported to a probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexerInfo {
    /// The indexer's tip height.
    pub block_height: u64,
    /// The consensus branch id at the indexer's tip.
    pub consensus_branch_id: String,
    /// The indexer software's vendor string.
    pub vendor: String,
    /// The indexer software's version string.
    pub version: String,
}

/// The outcome of probing one endpoint.
#[derive(Debug, Clone)]
pub struct IndexerProbe {
    /// The endpoint probed.
    pub indexer: Indexer,
    /// What it reported, or why the probe failed.
    pub result: Result<IndexerInfo, String>,
}

impl From<IndexerProbe> for JsonValue {
    fn from(probe: IndexerProbe) -> Self {
        let mut value = json::object! {
            "uri" => probe.indexer.uri.to_string(),
            "label" => probe.indexer.label,
        };
        match probe.result {
            Ok(info) => {
                value["reachable"] = true.into();
                value["block_height"] = info.block_height.into();
                value["consensus_branch_id"] = info.consensus_branch_id.into();
                value["vendor"] = info.vendor.into();
                value["version"] = info.version.into();
            }
            Err(error) => {
                value["reachable"] = false.into();
                value["error"] = error.into();
            }
        }
        value
    }
}

impl LightClient {
    /// Creates a `LightClient` with a new wallet from fresh entropy and a birthday of the higher value between
    /// [`chain_height` - 100] or sapling activation height.
    /// Will fail if a wallet file already exists in the given data directory unless `overwrite` is `true`.
    #[allow(clippy::result_large_err)]
    pub fn new(
        config: ZingoConfig,
        chain_height: BlockHeight,
        overwrite: bool,
        seed_derivation: SeedDerivation,
    ) -> Result<Self, LightClientError> {
        let birthday = Self::default_fresh_wallet_birthday(&config, chain_height);
        Self::new_with_birthday(config, birthday, overwrite, seed_derivation)
    }

    /// Creates a `LightClient` with a new wallet from fresh entropy and the given `birthday`.
    /// Will fail if a wallet file already exists in the given data directory unless `overwrite` is `true`.
    #[allow(clippy::result_large_err)]
    pub fn new_with_birthday(
        config: ZingoConfig,
        birthday: BlockHeight,
        overwrite: bool,
        seed_derivation: SeedDerivation,
    ) -> Result<Self, LightClientError> {
        Self::create_from_wallet(
            LightWallet::new(
                config.chain,
                WalletBase::FreshEntropy {
                    no_of_accounts: config.no_of_accounts,
                    seed_derivation,
                },
                birthday,
                config.wallet_settings.clone(),
            )?,
            config,
            overwrite,
        )
    }

    /// The birthday a fresh wallet gets without consulting the server: the higher of
    /// [`chain_height` - 100] and sapling activation height.
    pub fn default_fresh_wallet_birthday(
        config: &ZingoConfig,
        chain_height: BlockHeight,
    ) -> BlockHeight {
        let sapling_activation_height = config
            .chain
            .activation_height(zcash_protocol::consensus::NetworkUpgrade::Sapling)
            .expect("should have some sapling activation height");
        sapling_activation_height.max(chain_height - 100)
    }

    /// The birthday a fresh wallet should get against the configured server.
    ///
    /// Normally [`Self::default_fresh_wallet_birthday`]. When the server serves no Ironwood note
    /// commitment tree state at `chain_height` although the pool is active there (the crosslink
    /// node currently serves none), the wallet cannot recover the Ironwood frontier at a later
    /// birthday and must rebuild the tree from scanned blocks, so the birthday is the pool's
    /// activation height instead.
    ///
    /// # Error
    ///
    /// Returns an error if the server cannot be reached.
    pub async fn fresh_wallet_birthday(
        config: &ZingoConfig,
        chain_height: BlockHeight,
    ) -> Result<BlockHeight, String> {
        let default_birthday = Self::default_fresh_wallet_birthday(config, chain_height);
        let Some(ironwood_activation_height) = config
            .chain
            .activation_height(zcash_protocol::consensus::NetworkUpgrade::Nu6_3)
        else {
            return Ok(default_birthday);
        };
        if chain_height < ironwood_activation_height
            || default_birthday <= ironwood_activation_height
        {
            return Ok(default_birthday);
        }

        let tree_state =
            crate::grpc_connector::get_trees(config.get_lightwalletd_uri(), chain_height.into())
                .await?;
        if tree_state.ironwood_tree.is_empty() {
            tracing::warn!(
                "server serves no Ironwood tree state at height {chain_height}. the wallet birthday is set to the Ironwood activation height {ironwood_activation_height} so the Ironwood tree can be rebuilt from scanned blocks."
            );
            Ok(ironwood_activation_height)
        } else {
            Ok(default_birthday)
        }
    }

    /// Creates a `LightClient` from a `wallet` and `config`.
    /// Will fail if a wallet file already exists in the given data directory unless `overwrite` is `true`.
    #[allow(clippy::result_large_err)]
    pub fn create_from_wallet(
        wallet: LightWallet,
        config: ZingoConfig,
        overwrite: bool,
    ) -> Result<Self, LightClientError> {
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            if !overwrite && config.wallet_path_exists() {
                return Err(LightClientError::FileError(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!(
                        "Cannot save to given data directory as a wallet file already exists at:\n{}",
                        config.get_wallet_pathbuf().to_string_lossy()
                    ),
                )));
            }
        }
        Ok(LightClient {
            config,
            tor_client: None,
            wallet: Arc::new(RwLock::new(wallet)),
            sync_mode: Arc::new(AtomicU8::new(SyncMode::NotRunning as u8)),
            sync_handle: None,
            save_active: Arc::new(AtomicBool::new(false)),
            save_handle: None,
        })
    }

    /// Create a `LightClient` from an existing wallet file.
    #[allow(clippy::result_large_err)]
    pub fn create_from_wallet_path(config: ZingoConfig) -> Result<Self, LightClientError> {
        let wallet_path = if config.wallet_path_exists() {
            config.get_wallet_path()
        } else {
            return Err(LightClientError::FileError(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "Cannot read wallet. No file at {}",
                    config.get_wallet_path().display()
                ),
            )));
        };

        let buffer = BufReader::new(File::open(wallet_path).map_err(LightClientError::FileError)?);

        Self::create_from_wallet(
            LightWallet::read(buffer, config.chain).map_err(LightClientError::FileError)?,
            config,
            true,
        )
    }

    /// Returns config used to create lightclient.
    pub fn config(&self) -> &ZingoConfig {
        &self.config
    }

    /// Returns tor client.
    pub fn tor_client(&self) -> Option<&tor::Client> {
        self.tor_client.as_ref()
    }

    /// Returns URI of the server the lightclient is connected to.
    pub fn server_uri(&self) -> http::Uri {
        self.config.get_lightwalletd_uri()
    }

    /// Set the server uri.
    ///
    /// The URI is also recorded at the head of the endpoint list for this client's chain, so
    /// [`Self::indexers`] and the probes include it.
    pub fn set_server(&self, server: http::Uri) {
        if server.host().is_some() && server.port().is_some() {
            self.config
                .prefer_indexer(Indexer::new(server.clone(), "configured"));
        }
        *self.config.lightwalletd_uri.write().unwrap() = server;
    }

    /// The endpoints listed for this client's chain, most preferred first.
    #[must_use]
    pub fn indexers(&self) -> Vec<Indexer> {
        self.config.indexers_for_chain()
    }

    /// Adds an endpoint for this client's chain after the ones already listed.
    pub fn add_indexer(&self, indexer: Indexer) {
        self.config.add_indexer(indexer);
    }

    /// How long a indexer probe waits, connection included, before it is reported unreachable.
    pub const INDEXER_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

    /// Probes every endpoint listed for this client's chain concurrently with `GetLightdInfo`,
    /// giving each [`Self::INDEXER_PROBE_TIMEOUT`].
    ///
    /// Results come back in list order; each carries the indexer's reported tip height, or the
    /// error that stopped the probe.
    pub async fn probe_indexers(&self) -> Vec<IndexerProbe> {
        let indexers = self.indexers();
        let probes = indexers.into_iter().map(|indexer| async move {
            let result = match tokio::time::timeout(
                Self::INDEXER_PROBE_TIMEOUT,
                crate::grpc_connector::get_info(indexer.uri.clone()),
            )
            .await
            {
                Ok(Ok(info)) => Ok(IndexerInfo {
                    block_height: info.block_height,
                    consensus_branch_id: info.consensus_branch_id,
                    vendor: info.vendor,
                    version: info.version,
                }),
                Ok(Err(error)) => Err(error),
                Err(_) => Err(format!(
                    "no answer within {} s",
                    Self::INDEXER_PROBE_TIMEOUT.as_secs()
                )),
            };
            IndexerProbe { indexer, result }
        });
        futures::future::join_all(probes).await
    }

    /// Probes the endpoints listed for this client's chain and switches to the one reporting the
    /// highest tip, preferring the earlier-listed endpoint on a tie.
    ///
    /// # Error
    ///
    /// Returns the probe results as an error when no endpoint answered.
    pub async fn select_best_indexer(&self) -> Result<IndexerProbe, Vec<IndexerProbe>> {
        let probes = self.probe_indexers().await;
        let best = probes
            .iter()
            .filter_map(|probe| {
                probe
                    .result
                    .as_ref()
                    .ok()
                    .map(|info| (probe, info.block_height))
            })
            .max_by(|(_, a), (_, b)| a.cmp(b).then(std::cmp::Ordering::Less))
            .map(|(probe, _)| probe.clone());
        match best {
            Some(probe) => {
                *self.config.lightwalletd_uri.write().unwrap() = probe.indexer.uri.clone();
                Ok(probe)
            }
            None => Err(probes),
        }
    }

    /// Creates a tor client for current price updates.
    ///
    /// If `tor_dir` is `None` it will be set to the wallet's data directory.
    pub async fn create_tor_client(
        &mut self,
        tor_dir: Option<PathBuf>,
    ) -> Result<(), LightClientError> {
        let tor_dir =
            tor_dir.unwrap_or_else(|| self.config.get_zingo_wallet_dir().to_path_buf().join("tor"));
        tokio::fs::create_dir_all(tor_dir.as_path())
            .await
            .map_err(LightClientError::FileError)?;
        self.tor_client = Some(tor::Client::create(tor_dir.as_path(), |_| {}).await?);

        Ok(())
    }

    /// Removes the tor client.
    pub async fn remove_tor_client(&mut self) {
        self.tor_client = None;
    }

    /// Returns server information.
    // TODO: return concrete struct with from json impl
    pub async fn do_info(&self) -> String {
        match crate::grpc_connector::get_info(self.server_uri()).await {
            Ok(i) => {
                let o = json::object! {
                    "version" => i.version,
                    "git_commit" => i.git_commit,
                    "server_uri" => self.server_uri().to_string(),
                    "vendor" => i.vendor,
                    "taddr_support" => i.taddr_support,
                    "chain_name" => i.chain_name,
                    "sapling_activation_height" => i.sapling_activation_height,
                    "consensus_branch_id" => i.consensus_branch_id,
                    "latest_block_height" => i.block_height
                };
                o.pretty(2)
            }
            Err(e) => e,
        }
    }

    /// Wrapper for [`crate::wallet::LightWallet::generate_unified_address`].
    pub async fn generate_unified_address(
        &mut self,
        receivers: ReceiverSelection,
        account_id: zip32::AccountId,
    ) -> Result<(UnifiedAddressId, UnifiedAddress), KeyError> {
        self.wallet
            .write()
            .await
            .generate_unified_address(receivers, account_id)
    }

    /// Wrapper for [`crate::wallet::LightWallet::generate_transparent_address`].
    pub async fn generate_transparent_address(
        &mut self,
        account_id: zip32::AccountId,
        enforce_no_gap: bool,
    ) -> Result<(TransparentAddressId, TransparentAddress), KeyError> {
        self.wallet
            .write()
            .await
            .generate_transparent_address(account_id, enforce_no_gap)
    }

    /// Wrapper for [`crate::wallet::LightWallet::unified_addresses_json`].
    pub async fn unified_addresses_json(&self) -> JsonValue {
        self.wallet.read().await.unified_addresses_json()
    }

    /// Wrapper for [`crate::wallet::LightWallet::transparent_addresses_json`].
    pub async fn transparent_addresses_json(&self) -> JsonValue {
        self.wallet.read().await.transparent_addresses_json()
    }

    /// Wrapper for [`crate::wallet::LightWallet::account_balance`].
    pub async fn account_balance(
        &self,
        account_id: zip32::AccountId,
    ) -> Result<AccountBalance, BalanceError> {
        self.wallet
            .read()
            .await
            .account_balance(account_id, &self.config)
            .await
    }

    /// Wrapper for [`crate::wallet::LightWallet::transaction_summaries`].
    pub async fn transaction_summaries(
        &self,
        reverse_sort: bool,
    ) -> Result<TransactionSummaries, SummaryError> {
        self.wallet
            .read()
            .await
            .transaction_summaries(reverse_sort)
            .await
    }

    /// Wrapper for [`crate::wallet::LightWallet::value_transfers`].
    pub async fn value_transfers(
        &self,
        sort_highest_to_lowest: bool,
    ) -> Result<ValueTransfers, SummaryError> {
        self.wallet
            .read()
            .await
            .value_transfers(sort_highest_to_lowest)
            .await
    }

    /// Wrapper for [`crate::wallet::LightWallet::messages_containing`].
    pub async fn messages_containing(
        &self,
        filter: Option<&str>,
    ) -> Result<ValueTransfers, SummaryError> {
        self.wallet.read().await.messages_containing(filter).await
    }

    /// Wrapper for [`crate::wallet::LightWallet::do_total_memobytes_to_address`].
    pub async fn do_total_memobytes_to_address(
        &self,
    ) -> Result<TotalMemoBytesToAddress, SummaryError> {
        self.wallet
            .read()
            .await
            .do_total_memobytes_to_address()
            .await
    }

    /// Wrapper for [`crate::wallet::LightWallet::do_total_spends_to_address`].
    pub async fn do_total_spends_to_address(&self) -> Result<TotalSendsToAddress, SummaryError> {
        self.wallet.read().await.do_total_spends_to_address().await
    }

    /// Wrapper for [`crate::wallet::LightWallet::do_total_value_to_address`].
    pub async fn do_total_value_to_address(&self) -> Result<TotalValueToAddress, SummaryError> {
        self.wallet.read().await.do_total_value_to_address().await
    }

    /// The active finalizer roster as the node reports it.
    ///
    /// Each member is decoded with the node's own codec, which carries the member's
    /// self-certifying finalizer address when the node knows it. An address whose signature does
    /// not verify is dropped by the codec rather than surfaced.
    pub async fn get_roster_info(&self) -> Result<RosterMembers, String> {
        let uri = self.server_uri();
        let mut zcb_client = get_client(uri)
            .await
            .map_err(|e| format!("Error connecting to server: {e}"))?;

        let res = zcb_client
            .get_roster(Empty {})
            .await
            .map_err(|e| format!("Error getting roster: {e}"))?;
        let roster_bytes = res.into_inner().data;

        let mut cur = Cursor::new(&roster_bytes);
        let mut roster = Vec::new();
        while cur.position() < roster_bytes.len() as u64 {
            match RosterMember::read_from(&mut cur) {
                Ok(member) => roster.push(member),
                Err(err) => {
                    return Err(format!(
                        "Error deserializing roster member {}: {err}",
                        roster.len()
                    ));
                }
            }
        }

        Ok(RosterMembers::from_parts(roster))
    }

    /// The node's view of the bond `bond_key`: amount, status and the height of its last action.
    pub async fn get_bond_info(
        &self,
        bond_key: [u8; 32],
    ) -> Result<crate::crosslink::BondInfo, String> {
        let mut zcb_client = get_client(self.server_uri())
            .await
            .map_err(|err| format!("Error connecting to the server: {err}"))?;

        match zcb_client
            .get_bond_info(BondInfoRequest {
                bond_key: bond_key.to_vec(),
            })
            .await
        {
            Ok(res) => Ok(res.into_inner().into()),
            Err(status) if status.code() == tonic::Code::NotFound => Err(format!(
                "the node has no bond with key {}",
                hex::encode(bond_key)
            )),
            Err(err) => Err(format!("Error getting bond info: {err}")),
        }
    }

    pub async fn get_wallet_bonds(&self) -> Result<WalletBonds, String> {
        let wallet = self.wallet.read().await;
        let wallet_txs = wallet
            .get_wallet_transactions()
            .map_err(|err| format!("Error getting wallet transactions: {err:?}"))?;

        #[derive(Clone)]
        struct StkEvt {
            order: (u32, u32),
            txid: TxId,
            bond_key: [u8; 32],
            kind: StakingActionKind,
            amount_zats: u64,
            target: [u8; 32],
            target_address: Option<zcash_primitives::bft::FinalizerAddress>,
        }

        let mut evts: Vec<StkEvt> = Vec::new();

        for tx in wallet_txs.values() {
            let Some(sa) = tx.staking_data() else {
                continue;
            };
            if tx.status().is_failed() {
                continue;
            }

            let height: u32 = tx.status().get_height().into();
            let time: u32 = tx.datetime();

            let mut reversed_target = sa.target_finalizer_pk();
            reversed_target.reverse();

            evts.push(StkEvt {
                order: (height, time),
                txid: tx.txid(),
                bond_key: sa.unique_pubkey(),
                kind: sa.kind(),
                amount_zats: sa.amount_zats(),
                target: reversed_target,
                target_address: crate::crosslink::action_target_address(&sa),
            });
        }

        evts.sort_by_key(|e| e.order);

        // Bond key -> (bond, last_seen_order)
        let mut by_bond: HashMap<[u8; 32], (WalletBond, (u32, u32))> = HashMap::new();

        for e in evts {
            match e.kind {
                StakingActionKind::CreateNewDelegationBond
                | StakingActionKind::ConvertFinalizerRewardToDelegationBond => {
                    let new = WalletBond {
                        created_in_txid: e.txid,
                        pubkey: e.bond_key,
                        amount_zats: e.amount_zats,
                        status: BondStatus::Active,
                        last_action_height: None,
                        next_action_height: None,
                        finalizer: e.target,
                        finalizer_address: e.target_address,
                    };
                    LightClient::upsert_latest(&mut by_bond, e.bond_key, new, e.order);
                }

                StakingActionKind::RetargetDelegationBond => {
                    let updated = if let Some((existing, _)) = by_bond.get(&e.bond_key) {
                        WalletBond {
                            finalizer: e.target,
                            finalizer_address: e.target_address,
                            ..*existing
                        }
                    } else {
                        WalletBond {
                            created_in_txid: e.txid,
                            pubkey: e.bond_key,
                            amount_zats: 0,
                            status: BondStatus::Active,
                            last_action_height: None,
                            next_action_height: None,
                            finalizer: e.target,
                            finalizer_address: e.target_address,
                        }
                    };

                    LightClient::upsert_latest(&mut by_bond, e.bond_key, updated, e.order);
                }

                StakingActionKind::BeginDelegationUnbonding => {
                    let (existing, _) = by_bond.get(&e.bond_key).copied().ok_or_else(|| {
                    format!(
                        "Saw BeginDelegationUnbonding for bond_key={} but no prior Create/Retarget in wallet history",
                        hex::encode(e.bond_key)
                    )
                })?;

                    let updated = WalletBond {
                        status: BondStatus::Unbonding,
                        ..existing
                    };
                    LightClient::upsert_latest(&mut by_bond, e.bond_key, updated, e.order);
                }

                StakingActionKind::WithdrawDelegationBond => {
                    let (existing, _) = by_bond.get(&e.bond_key).copied().ok_or_else(|| {
                    format!(
                        "Saw WithdrawDelegationBond for bond_key={} but no prior Create/Retarget in wallet history",
                        hex::encode(e.bond_key)
                    )
                })?;

                    let updated = WalletBond {
                        status: BondStatus::Withdrawn,
                        ..existing
                    };
                    LightClient::upsert_latest(&mut by_bond, e.bond_key, updated, e.order);
                }

                _ => continue,
            }
        }

        let mut bonds: Vec<WalletBond> = by_bond.into_values().map(|(bond, _order)| bond).collect();

        bonds.sort_by_key(|b| b.status.wire());

        // The node knows the bond's current amount (rewards grow it), its status and the height
        // of its last action, from which the next permitted action's height follows.
        let calendar = crate::crosslink::staking_calendar(&self.config.chain);
        let mut enriched: Vec<WalletBond> = Vec::with_capacity(bonds.len());
        for b in bonds.into_iter() {
            match self.get_bond_info(b.pubkey).await {
                Ok(bond_info) => {
                    enriched.push(WalletBond {
                        amount_zats: bond_info.amount_zats,
                        status: bond_info.status,
                        last_action_height: Some(bond_info.last_action_height),
                        next_action_height: calendar
                            .and_then(|calendar| bond_info.next_action_height(calendar)),
                        ..b
                    });
                }
                Err(err) => {
                    tracing::warn!("showing bond from wallet history only: {err}");
                    enriched.push(b);
                }
            }
        }

        Ok(WalletBonds { bonds: enriched })
    }

    pub async fn request_faucet_funds(&self, address: String) -> Result<String, String> {
        let mut zcb_client = get_client(self.server_uri()).await.unwrap();
        let res = zcb_client
            .request_faucet_donation(FaucetRequest { address })
            .await;
        match res {
            Ok(res) => Ok(res.into_inner().amount.to_string()),
            Err(err) => Err(format!("Error requesting faucet funds: {err:?}")),
        }
    }

    fn upsert_latest(
        map: &mut HashMap<[u8; 32], (WalletBond, (u32, u32))>,
        bond_key: [u8; 32],
        candidate: WalletBond,
        candidate_order: (u32, u32),
    ) {
        match map.get(&bond_key) {
            None => {
                map.insert(bond_key, (candidate, candidate_order));
            }
            Some((_existing, existing_order)) => {
                if candidate_order >= *existing_order {
                    map.insert(bond_key, (candidate, candidate_order));
                }
            }
        }
    }
}

impl std::fmt::Debug for LightClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LightClient")
            .field("config", &self.config)
            .field("sync_mode", &self.sync_mode())
            .field(
                "save_active",
                &self.save_active.load(std::sync::atomic::Ordering::Acquire),
            )
            .finish()
    }
}
#[cfg(test)]
mod tests {
    use crate::config::ConfiguredActivationHeights;
    use crate::{
        config::{ChainType, ZingoConfig},
        lightclient::error::LightClientError,
        wallet::LightWallet,
    };
    use bip0039::Mnemonic;
    use tempfile::TempDir;
    use zingo_test_vectors::seeds::CHIMNEY_BETTER_SEED;

    use crate::{
        lightclient::LightClient,
        wallet::{SeedDerivation, WalletBase},
    };

    #[tokio::test]
    async fn new_wallet_from_phrase() {
        let temp_dir = TempDir::new().unwrap();
        let config = ZingoConfig::build(ChainType::Regtest(
            ConfiguredActivationHeights::regtest_all_active(),
        ))
        .set_wallet_dir(temp_dir.path().to_path_buf())
        .create();
        let mut lc = LightClient::create_from_wallet(
            LightWallet::new(
                config.chain,
                WalletBase::Mnemonic {
                    mnemonic: Mnemonic::from_phrase(CHIMNEY_BETTER_SEED.to_string()).unwrap(),
                    no_of_accounts: config.no_of_accounts,
                    seed_derivation: SeedDerivation::Zip32Standard,
                },
                1.into(),
                config.wallet_settings.clone(),
            )
            .unwrap(),
            config.clone(),
            false,
        )
        .unwrap();

        lc.save_task().await;
        lc.wait_for_save().await;

        let lc_file_exists_error = LightClient::create_from_wallet(
            LightWallet::new(
                config.chain,
                WalletBase::Mnemonic {
                    mnemonic: Mnemonic::from_phrase(CHIMNEY_BETTER_SEED.to_string()).unwrap(),
                    no_of_accounts: config.no_of_accounts,
                    seed_derivation: SeedDerivation::Zip32Standard,
                },
                1.into(),
                config.wallet_settings.clone(),
            )
            .unwrap(),
            config,
            false,
        )
        .unwrap_err();

        assert!(matches!(
            lc_file_exists_error,
            LightClientError::FileError(_)
        ));

        // The first transparent address and unified address should be derived
        assert_eq!(
            "tmYd5GP6JxUxTUcz98NLPumEotvaMPaXytz".to_string(),
            lc.transparent_addresses_json().await[0]["encoded_address"]
        );
        assert_eq!(
            "uregtest15en5x5cnsc7ye3wfy0prnh3ut34ns9w40htunlh9htfl6k5p004ja5gprxfz8fygjeax07a8489wzjk8gsx65thcp6d3ku8umgaka6f0"
                .to_string(),
            lc.unified_addresses_json().await[0]["encoded_address"]
        );
    }
}
