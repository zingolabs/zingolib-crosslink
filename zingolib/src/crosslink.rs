//! Crosslink helpers: finalizer addresses and staking actions.
//!
//! A finalizer address is a self-certifying capability: the finalizer's ed25519 public key plus
//! that key's signature over a fixed message, encoded as `zfinv1` followed by 128 base64url
//! characters (134 characters in total). Consensus rejects a staking action whose target address
//! does not verify, so a wallet must verify an address before it builds a transaction around it.

use zcash_primitives::transaction::{StakingAction, StakingActionKind};

pub use zcash_primitives::bft::{FinalizerAddress, PubKeyID, StakingParameters};
pub use zcash_primitives::transaction::is_power_of_10;

/// The staking calendar of the public networks; see
/// [`zcash_primitives::bft::PROTOTYPE_STAKING`].
pub const PUBLIC_STAKING_CALENDAR: StakingParameters = zcash_primitives::bft::PROTOTYPE_STAKING;

/// Whether a staking action other than a retarget or a reward conversion may be mined at
/// `height`: consensus allows them only while `height % period < day_window`.
#[must_use]
pub fn staking_window_open(height: u32, staking: &StakingParameters) -> bool {
    height % staking.period < staking.day_window
}

/// The first height at or after `height` where the staking window is open.
#[must_use]
pub fn next_staking_window_start(height: u32, staking: &StakingParameters) -> u32 {
    if staking_window_open(height, staking) {
        height
    } else {
        (height / staking.period + 1) * staking.period
    }
}

/// The first height at which consensus accepts the next action on a bond whose previous action
/// was mined at `last_action_height`: the action delay must have elapsed and the staking window
/// must be open.
#[must_use]
pub fn earliest_bond_action_height(last_action_height: u32, staking: &StakingParameters) -> u32 {
    next_staking_window_start(
        last_action_height.saturating_add(staking.action_delay),
        staking,
    )
}

/// The staking calendar consensus applies on `chain`, or `None` where the wallet cannot know
/// it (regtest nets may run a shrunk calendar).
#[must_use]
pub fn staking_calendar(chain: &crate::config::ChainType) -> Option<&'static StakingParameters> {
    match chain {
        crate::config::ChainType::Mainnet | crate::config::ChainType::Testnet(_) => {
            Some(&PUBLIC_STAKING_CALENDAR)
        }
        crate::config::ChainType::Regtest(_) => None,
    }
}

/// A delegation bond's state as the node reports it in `GetBondInfo`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BondStatus {
    /// Delegated; may be retargeted, or unbonded once the action delay has passed.
    Active,
    /// Unbonding; may be withdrawn once the action delay has passed.
    Unbonding,
    /// Withdrawn into the wallet; nothing more can be done with it.
    Withdrawn,
    /// Burned by a slashing hardfork.
    Burned,
    /// A status code this wallet does not know.
    Unknown(u32),
}

impl BondStatus {
    /// Decodes the wire value of `BondInfoResponse.status`.
    #[must_use]
    pub fn from_wire(status: u32) -> Self {
        match status {
            0 => Self::Active,
            1 => Self::Unbonding,
            2 => Self::Withdrawn,
            3 => Self::Burned,
            other => Self::Unknown(other),
        }
    }

    /// The wire value of `BondInfoResponse.status`.
    #[must_use]
    pub fn wire(self) -> u32 {
        match self {
            Self::Active => 0,
            Self::Unbonding => 1,
            Self::Withdrawn => 2,
            Self::Burned => 3,
            Self::Unknown(other) => other,
        }
    }

    /// Whether the bond's value is still at stake (active or unbonding).
    #[must_use]
    pub fn is_staked(self) -> bool {
        matches!(self, Self::Active | Self::Unbonding)
    }
}

impl std::fmt::Display for BondStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Unbonding => write!(f, "unbonding"),
            Self::Withdrawn => write!(f, "withdrawn"),
            Self::Burned => write!(f, "burned"),
            Self::Unknown(other) => write!(f, "unknown({other})"),
        }
    }
}

/// What the node knows about a bond: the `GetBondInfo` response, typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BondInfo {
    /// The bond's current value, which rewards grow.
    pub amount_zats: u64,
    /// The bond's state.
    pub status: BondStatus,
    /// The height of the action that put the bond in its current state: creation while active,
    /// the unbond while unbonding, the withdrawal once withdrawn.
    pub last_action_height: u32,
}

impl BondInfo {
    /// The first height at which consensus accepts the bond's next action (an unbond while
    /// active, a withdrawal while unbonding), or `None` when no action remains.
    #[must_use]
    pub fn next_action_height(&self, staking: &StakingParameters) -> Option<u32> {
        self.status
            .is_staked()
            .then(|| earliest_bond_action_height(self.last_action_height, staking))
    }
}

impl From<zcash_client_backend::proto::service::BondInfoResponse> for BondInfo {
    fn from(response: zcash_client_backend::proto::service::BondInfoResponse) -> Self {
        Self {
            amount_zats: response.amount,
            status: BondStatus::from_wire(response.status),
            last_action_height: response.last_action_height,
        }
    }
}

/// Why a finalizer address string was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FinalizerAddressError {
    /// The string is not `zfinv1` followed by 128 base64url characters.
    #[error(
        "invalid finalizer address encoding: expected `zfinv1` followed by 128 base64url characters"
    )]
    Encoding,
    /// The embedded signature does not verify under the embedded public key, so the address was
    /// not minted by the holder of that key.
    #[error("finalizer address signature does not verify")]
    Signature,
}

/// Decodes and verifies an encoded finalizer address.
///
/// Surrounding whitespace is ignored. The address is only returned if its capability signature
/// verifies, so the result can be used as a staking target directly.
pub fn parse_finalizer_address(encoded: &str) -> Result<FinalizerAddress, FinalizerAddressError> {
    let address =
        FinalizerAddress::decode(encoded.trim()).ok_or(FinalizerAddressError::Encoding)?;
    if !address.verify() {
        return Err(FinalizerAddressError::Signature);
    }
    Ok(address)
}

/// Encodes a finalizer address in its `zfinv1` string form.
#[must_use]
pub fn encode_finalizer_address(address: &FinalizerAddress) -> String {
    address.encode()
}

/// The finalizer public key in the reversed-hex form the node displays.
#[must_use]
pub fn finalizer_pubkey_hex(pub_key: &[u8; 32]) -> String {
    let mut reversed = *pub_key;
    reversed.reverse();
    hex::encode(reversed)
}

/// The finalizer address a staking action points stake at, when it carries one.
///
/// `CreateNewDelegationBond` and `RetargetDelegationBond` carry the full capability; the other
/// actions name no target or only a bare key.
#[must_use]
pub fn action_target_address(action: &StakingAction) -> Option<FinalizerAddress> {
    match action {
        StakingAction::CreateNewDelegationBond {
            target_finalizer, ..
        } => Some(*target_finalizer),
        StakingAction::RetargetDelegationBond { to_finalizer, .. } => Some(*to_finalizer),
        _ => None,
    }
}

/// A human readable name for a staking action kind.
#[must_use]
pub fn staking_action_kind_name(kind: StakingActionKind) -> &'static str {
    match kind {
        StakingActionKind::Null => "null",
        StakingActionKind::CreateNewDelegationBond => "create_bond",
        StakingActionKind::BeginDelegationUnbonding => "begin_unbonding",
        StakingActionKind::WithdrawDelegationBond => "withdraw_bond",
        StakingActionKind::RetargetDelegationBond => "retarget_bond",
        StakingActionKind::ConvertFinalizerRewardToDelegationBond => "convert_finalizer_reward",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zcash_primitives::bft::finalizer_key_from_seed;

    const CALENDAR: StakingParameters = StakingParameters {
        period: 100,
        day_window: 10,
        action_delay: 11,
    };

    #[test]
    fn staking_window_follows_the_calendar() {
        assert!(staking_window_open(0, &CALENDAR));
        assert!(staking_window_open(9, &CALENDAR));
        assert!(!staking_window_open(10, &CALENDAR));
        assert!(!staking_window_open(99, &CALENDAR));
        assert!(staking_window_open(100, &CALENDAR));

        assert_eq!(next_staking_window_start(5, &CALENDAR), 5);
        assert_eq!(next_staking_window_start(10, &CALENDAR), 100);
        assert_eq!(next_staking_window_start(199, &CALENDAR), 200);
    }

    #[test]
    fn next_bond_action_waits_for_the_delay_and_the_window() {
        // created at 5: delay ends at 16, outside the window, so the next day.
        assert_eq!(earliest_bond_action_height(5, &CALENDAR), 100);
        // unbonded at 95: delay ends at 106, inside the window.
        assert_eq!(earliest_bond_action_height(95, &CALENDAR), 106);

        let active = BondInfo {
            amount_zats: 1,
            status: BondStatus::Active,
            last_action_height: 5,
        };
        assert_eq!(active.next_action_height(&CALENDAR), Some(100));
        let withdrawn = BondInfo {
            status: BondStatus::Withdrawn,
            ..active
        };
        assert_eq!(withdrawn.next_action_height(&CALENDAR), None);
    }

    #[test]
    fn bond_status_round_trips_the_wire_value() {
        for wire in 0..5 {
            assert_eq!(BondStatus::from_wire(wire).wire(), wire);
        }
        assert_eq!(BondStatus::from_wire(3), BondStatus::Burned);
        assert!(BondStatus::Unbonding.is_staked());
        assert!(!BondStatus::Withdrawn.is_staked());
    }

    #[test]
    fn round_trips_a_finalizer_address() {
        let (_, signing_key, pub_key) = finalizer_key_from_seed(b"zingolib finalizer test");
        let address = FinalizerAddress::create(&signing_key);
        let encoded = encode_finalizer_address(&address);

        assert!(encoded.starts_with("zfinv1"));
        assert_eq!(encoded.len(), 134);
        let decoded = parse_finalizer_address(&format!("  {encoded}\n")).unwrap();
        assert_eq!(decoded, address);
        assert_eq!(decoded.pub_key, pub_key);
    }

    #[test]
    fn rejects_bad_encodings_and_bad_signatures() {
        assert_eq!(
            parse_finalizer_address("zfinv1tooshort"),
            Err(FinalizerAddressError::Encoding)
        );
        assert_eq!(
            parse_finalizer_address(&"x".repeat(134)),
            Err(FinalizerAddressError::Encoding)
        );

        let (_, signing_key, _) = finalizer_key_from_seed(b"zingolib finalizer test");
        let mut encoded = encode_finalizer_address(&FinalizerAddress::create(&signing_key));
        // Flip one character inside the signature part of the payload.
        let last = encoded.pop().unwrap();
        encoded.push(if last == 'A' { 'B' } else { 'A' });
        assert_eq!(
            parse_finalizer_address(&encoded),
            Err(FinalizerAddressError::Signature)
        );
    }

    #[test]
    fn a_version_two_prefix_is_rejected() {
        let (_, signing_key, _) = finalizer_key_from_seed(b"zingolib finalizer test");
        let encoded = encode_finalizer_address(&FinalizerAddress::create(&signing_key));
        let future = encoded.replacen("zfinv1", "zfinv2", 1);
        assert_eq!(
            parse_finalizer_address(&future),
            Err(FinalizerAddressError::Encoding)
        );
    }
}
