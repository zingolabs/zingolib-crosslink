pub mod types;

use std::io::Result;

use serde::{Deserialize, Serialize};
use zebra_chain::{amount::NonNegative, block::Height};
use zebra_rpc::methods::{GetBestBlockHeightAndHash, GetBlockHash, GetTxHash, types::zec::Zec};

use crate::types::TFLBlockFinality;

pub trait TFLClient {
    fn new() -> Self;

    /// Placeholder function for checking whether the TFL has been activated.
    ///
    /// zcashd reference: none
    /// method: post
    /// tags: tfl
    ///
    /// ## Example Usage
    /// ```shell
    /// curl -X POST -H "Content-Type: application/json" -d \
    /// '{ "jsonrpc": "2.0", "method": "is_tfl_activated", "params": [], "id": 1 }' \
    /// http://127.0.0.1:8232
    /// ```
    /// *(The `address:port` matches the value in `zebrad.toml > [rpc] > listen_addr`)*
    async fn is_tfl_activated(&self) -> Option<bool>;

    /// Get the roster with stake presented as decimal/precise floating point ZEC
    ///
    /// zcashd reference: none
    /// method: post
    /// tags: tfl
    ///
    /// ## Example Usage
    /// ```shell
    /// curl -X POST -H "Content-Type: application/json" -d \
    /// '{ "jsonrpc": "2.0", "method": "get_tfl_roster_zec", "params": [], "id": 1 }' \
    /// http://127.0.0.1:8232
    /// ```
    /// *(The `address:port` matches the value in `zebrad.toml > [rpc] > listen_addr`)*
    async fn get_tfl_roster_zec(&self) -> Option<Vec<TFLStakerZec>>;

    /// Get the roster with stake presented as an integer number of zatoshis
    ///
    /// zcashd reference: none
    /// method: post
    /// tags: tfl
    ///
    /// ## Example Usage
    /// ```shell
    /// curl -X POST -H "Content-Type: application/json" -d \
    /// '{ "jsonrpc": "2.0", "method": "get_tfl_roster_zats", "params": [], "id": 1 }' \
    /// http://127.0.0.1:8232
    /// ```
    /// *(The `address:port` matches the value in `zebrad.toml > [rpc] > listen_addr`)*
    async fn get_tfl_roster_zats(&self) -> Option<Vec<TFLStakerZats>>;

    /// Get the fat pointer to the BFT Chain tip. TODO: Example
    async fn get_tfl_fat_pointer_to_bft_chain_tip(&self) -> Option<FatPointerToBftBlock>;

    /// Get BFT command buffer
    async fn staking_command(&self, string: String) -> Result<String>;

    /// Placeholder function for getting actual final block.
    /// For the sake of testing, this currently treats pre-reorg block as final.
    ///
    /// zcashd reference: none
    /// method: post
    /// tags: tfl
    ///
    /// ## Example Usage
    /// ```bash
    /// curl -X POST -H "Content-Type: application/json" -d \
    /// '{ "jsonrpc": "2.0", "method": "get_tfl_final_block_hash", "params": [], "id": 1 }' \
    /// http://127.0.0.1:8232
    /// ```
    /// *(The `address:port` matches the value in `zebrad.toml > [rpc] > listen_addr`)*
    async fn get_tfl_final_block_hash(&self) -> Option<GetBlockHash>;

    /// Placeholder function for getting actual final block hash & height.
    /// For the sake of testing, this currently treats pre-reorg block as final.
    ///
    /// zcashd reference: none
    /// method: post
    /// tags: tfl
    ///
    /// ## Example Usage
    /// ```bash
    /// curl -X POST -H "Content-Type: application/json" -d \
    /// '{ "jsonrpc": "2.0", "method": "get_tfl_final_block_height_and_hash", "params": [], "id": 1 }' \
    /// http://127.0.0.1:8232
    /// ```
    /// *(The `address:port` matches the value in `zebrad.toml > [rpc] > listen_addr`)*
    async fn get_tfl_final_block_height_and_hash(&self) -> Option<GetBestBlockHeightAndHash>;

    /// Placeholder function for polling finality status of a specific block.
    /// (Uses [`GetBlockHash`] as a wrapper around [`block::Hash`] so that hashes can be passed as
    /// a string rather than an array in the json `params`.)
    ///
    /// zcashd reference: none
    /// method: post
    /// tags: tfl
    ///
    /// ## Example Usage
    /// ```bash
    /// curl -X POST -H "Content-Type: application/json" -d \
    /// '{ "jsonrpc": "2.0", "method": "get_tfl_block_finality_from_hash", "params": ["000000000ec8908cff52ae51841273e79f08d140b41ae4a4827575ed28b7b34a"], "id": 1 }' \
    /// http://127.0.0.1:8232
    /// ```
    /// *(The `address:port` matches the value in `zebrad.toml > [rpc] > listen_addr`)*
    async fn get_tfl_block_finality_from_hash(
        &self,
        hash: GetBlockHash,
    ) -> Option<TFLBlockFinality>;

    /// Placeholder function for polling finality status of a specific transaction.
    /// (Uses [`GetTxHash`] as a wrapper around [`transaction::Hash`] so that hashes can be passed as
    /// a string rather than an array in the json `params`.)
    ///
    /// zcashd reference: none
    /// method: post
    /// tags: tfl
    ///
    /// ## Example Usage
    /// ```bash
    /// curl -X POST -H "Content-Type: application/json" -d \
    /// '{ "jsonrpc": "2.0", "method": "get_tfl_tx_finality_from_hash", "params":
    /// ["8217e0aac8f864947eb120634cf4e6225609d4c9590ae79b72b5d3ab4c1035e0"], "id": 1 }' \
    /// http://127.0.0.1:823
    /// ```
    /// *(The `address:port` matches the value in `zebrad.toml > [rpc] > listen_addr`)*
    ///
    /// For experimenting, the [`getblock`](RpcServer::get_block) method's result includes transactions.
    async fn get_tfl_tx_finality_from_hash(&self, hash: GetTxHash) -> Option<TFLBlockFinality>;

    /// Specify finalized block for testing
    /// TODO: Regtest mode only
    ///
    /// zcashd reference: none
    /// method: post
    /// tags: tfl
    ///
    /// ## Example Usage
    /// ```bash
    /// curl -X POST -H "Content-Type: application/json" -d \
    /// '{ "jsonrpc": "2.0", "method": "set_tfl_finality_by_hash", "params": ["000000000ec8908cff52ae51841273e79f08d140b41ae4a4827575ed28b7b34a"], "id": 1 }' \
    /// http://127.0.0.1:8232
    /// ```
    /// *(The `address:port` matches the value in `zebrad.toml > [rpc] > listen_addr`)*
    ///
    /// For experimenting, the [`getbestblockhash`](RpcServer::get_best_block_hash) method provides the tip, which won't yet be final.
    async fn set_tfl_finality_by_hash(&self, hash: GetBlockHash) -> Result<Height>;

    /// Placeholder function for subscribing to new final block changes.
    /// (JSON-RPC pub-sub not implemented, as that will be obviated my move to gRPC).
    ///
    /// zcashd reference: none
    /// method: ?
    /// tags: tfl
    ///
    /// ## Example Usage
    /// ```bash
    /// curl -X POST -H "Content-Type: application/json" -d \
    /// '{ "jsonrpc": "2.0", "method": "subscribe_tfl_new_final_block_hash", "params": [], "id": 1 }' \
    /// http://127.0.0.1:8232
    /// ```
    /// *(The `address:port` matches the value in `zebrad.toml > [rpc] > listen_addr`)*
    /// TODO: #[subscription(name = "subscribe_tfl_new_final_block_hash", item = String)]
    fn subscribe_tfl_new_final_block_hash(&self);

    /// Placeholder function for streaming new final block changes (for testing).
    /// (JSON-RPC pub-sub not implemented, as that will be obviated my move to gRPC).
    ///
    /// zcashd reference: none
    /// method: ?
    /// tags: tfl
    ///
    /// ## Example Usage
    /// ```bash
    /// curl -X POST -H "Content-Type: application/json" -d \
    /// '{ "jsonrpc": "2.0", "method": "stream_tfl_new_final_block_hash", "params": [], "id": 1 }' \
    /// http://127.0.0.1:8232
    /// ```
    /// *(The `address:port` matches the value in `zebrad.toml > [rpc] > listen_addr`)*
    async fn stream_tfl_new_final_block_hash(&self);

    /// Placeholder function for streaming new final transaction changes (for testing).
    /// (JSON-RPC pub-sub not implemented, as that will be obviated my move to gRPC).
    ///
    /// zcashd reference: none
    /// method: ?
    /// tags: tfl
    ///
    /// ## Example Usage
    /// ```bash
    /// curl -X POST -H "Content-Type: application/json" -d \
    /// '{ "jsonrpc": "2.0", "method": "stream_tfl_new_final_txs", "params": [], "id": 1 }' \
    /// http://127.0.0.1:8232
    /// ```
    /// *(The `address:port` matches the value in `zebrad.toml > [rpc] > listen_addr`)*
    async fn stream_tfl_new_final_txs(&self);

    /// Placeholder function for blocking until a particular block becomes final.
    ///
    /// zcashd reference: none
    /// method: ?
    /// tags: tfl
    ///
    /// ## Example Usage
    /// ```bash
    /// curl -X POST -H "Content-Type: application/json" -d \
    /// '{ "jsonrpc": "2.0", "method": "notify_tfl_block_becomes_final_by_hash", "params": ["000000000ec8908cff52ae51841273e79f08d140b41ae4a4827575ed28b7b34a"], "id": 1 }' \
    /// http://127.0.0.1:8232
    /// ```
    /// *(The `address:port` matches the value in `zebrad.toml > [rpc] > listen_addr`)*
    ///
    /// For experimenting, this is easiest to trigger by manually calling the
    /// [`set_tfl_finality_by_hash`](RpcServer::set_tfl_finality_by_hash) method from another terminal
    /// for the block hash passed here (e.g. the tip).
    async fn notify_tfl_block_becomes_final_by_hash(
        &self,
        hash: GetBlockHash,
    ) -> Option<TFLBlockFinality>;

    /// Placeholder function for blocking until a particular transaction becomes final.
    ///
    /// zcashd reference: none
    /// method: ?
    /// tags: tfl
    ///
    /// ## Example Usage
    /// ```bash
    /// curl -X POST -H "Content-Type: application/json" -d \
    /// '{ "jsonrpc": "2.0", "method": "notify_tfl_block_becomes_final_by_hash", "params": ["000000000ec8908cff52ae51841273e79f08d140b41ae4a4827575ed28b7b34a"], "id": 1 }' \
    /// http://127.0.0.1:8232
    /// ```
    /// *(The `address:port` matches the value in `zebrad.toml > [rpc] > listen_addr`)*
    ///
    /// For experimenting, the [`getblock`](RpcServer::get_block) method's result includes transactions.
    ///
    /// This is easiest to trigger by manually calling the [`set_tfl_finality_by_hash`](RpcServer::set_tfl_finality_by_hash)
    /// method from another terminal for the block that contains the transaction hash
    /// passed here (e.g. the tip).
    // TODO: "by_id"?
    async fn notify_tfl_tx_becomes_final_by_hash(
        &self,
        hash: GetTxHash,
    ) -> Option<TFLBlockFinality>;
}

/// TODO: #[serde(with = "hex")]
#[derive(Clone)]
pub struct TFLStakerZec([u8; 32], Zec<NonNegative>);

/// TODO: #[serde(with = "hex")]
#[derive(Clone)]
pub struct TFLStakerZats([u8; 32], u64);

/// A bundle of signed votes for a block
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FatPointerToBftBlock {
    /// The fixed size portion of the the fat pointer.
    // #[serde(with = "serde_big_array::BigArray")]
    pub vote_for_block_without_finalizer_public_key: [u8; 76 - 32],

    /// The array of signatures in the fat pointer.
    pub signatures: Vec<FatPointerSignature>,
}

/// A signature inside a fat pointer to a BFT Block.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FatPointerSignature {
    /// The public key associated with this signature.
    pub public_key: [u8; 32],

    // #[serde(with = "serde_big_array::BigArray")]
    /// The actual ed25519 signature itself.
    pub vote_signature: [u8; 64],
}
