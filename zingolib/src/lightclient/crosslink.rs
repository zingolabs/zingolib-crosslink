use json::JsonValue;
use serde::Serialize;
use zcash_primitives::transaction::RosterMember;
use zcash_protocol::TxId;

use crate::crosslink::{BondStatus, encode_finalizer_address, finalizer_pubkey_hex};

#[derive(Debug, Clone, Serialize)]
pub struct RosterMembers {
    pub members: Vec<RosterMember>,
}

impl RosterMembers {
    pub fn from_parts(members: Vec<RosterMember>) -> Self {
        Self { members }
    }
}

impl From<RosterMembers> for JsonValue {
    fn from(roster_members: RosterMembers) -> Self {
        let mut members = JsonValue::new_array();

        for member in roster_members.members {
            let pubkey = finalizer_pubkey_hex(&member.pub_key);
            // The roster carries each finalizer's self-certifying address when the node knows it;
            // this is the string to pass to `stake`.
            let finalizer_address = member
                .finalizer_address
                .as_ref()
                .map_or(JsonValue::Null, |address| {
                    JsonValue::from(encode_finalizer_address(address))
                });

            let mut txids = JsonValue::new_array();
            for entry in member.txids {
                txids
                    .push(json::object! {
                        "txid" => hex::encode(entry.txid),
                        "accumulated_zats" => entry.zats
                    })
                    .unwrap();
            }

            members
                .push(json::object! {
                    "pubkey" => pubkey,
                    "finalizer_address" => finalizer_address,
                    "voting_power" => member.voting_power,
                    "txids" => txids
                })
                .unwrap();
        }

        json::object! {
            "roster_members" => members
        }
    }
}

pub struct WalletBonds {
    pub bonds: Vec<WalletBond>,
}

#[derive(Debug, Copy, Clone)]
pub struct WalletBond {
    pub created_in_txid: TxId,

    pub pubkey: [u8; 32],

    /// The bond's value: the node's current figure (rewards grow it) when it could be asked,
    /// else the created amount.
    pub amount_zats: u64,

    /// The node's status when it could be asked, else the status the wallet's own actions imply.
    pub status: BondStatus,

    /// The height of the action that put the bond in its current state, from the node.
    pub last_action_height: Option<u32>,

    /// The first height at which consensus accepts the bond's next action: an unbond while
    /// active, a withdrawal while unbonding. `None` when nothing remains to do, or when the
    /// node or the calendar is unavailable.
    pub next_action_height: Option<u32>,

    /// Finalizer / target this bond is delegated to (public key, node display byte order)
    pub finalizer: [u8; 32],

    /// The target's self-certifying address, when the wallet's own actions carry it.
    pub finalizer_address: Option<zcash_primitives::bft::FinalizerAddress>,
}

impl From<WalletBond> for JsonValue {
    fn from(wallet_bond: WalletBond) -> Self {
        json::object! {
            "created_in_txid" => wallet_bond.created_in_txid.to_string(),
            "bond_key" => hex::encode(wallet_bond.pubkey),
            "amount_zats" => wallet_bond.amount_zats,
            "status" => wallet_bond.status.wire(),
            "status_name" => wallet_bond.status.to_string(),
            "last_action_height" => wallet_bond.last_action_height,
            "next_action_height" => wallet_bond.next_action_height,
            "finalizer" => hex::encode(wallet_bond.finalizer),
            "finalizer_address" => wallet_bond
                .finalizer_address
                .as_ref()
                .map_or(JsonValue::Null, |address| JsonValue::from(encode_finalizer_address(address)))
        }
    }
}

impl From<WalletBonds> for JsonValue {
    fn from(wallet_bonds: WalletBonds) -> Self {
        json::object! {
            "bonds" => wallet_bonds.bonds
        }
    }
}
