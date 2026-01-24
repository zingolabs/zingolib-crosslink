use json::JsonValue;
use serde::Serialize;
use zcash_primitives::transaction::RosterMember;
use zcash_protocol::TxId;

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
            let pubkey = hex::encode(member.pub_key);

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

pub struct WalletBond {
    pub created_in_txid: TxId,

    pub pubkey: [u8; 32],

    pub amount_zats: u64,

    /// 0 = Active, 1 = Unbonding, 2 = Withdrawn
    pub status: u32,
}

impl From<WalletBond> for JsonValue {
    fn from(wallet_bond: WalletBond) -> Self {
        json::object! {
            "created_in_txid" => wallet_bond.created_in_txid.to_string(),
            "pub_key" => hex::encode(wallet_bond.pubkey),
            "amount_zats" => wallet_bond.amount_zats,
            "status" => wallet_bond.status
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
