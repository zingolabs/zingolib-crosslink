//! Crosslink helpers: finalizer addresses and staking actions.
//!
//! A finalizer address is a self-certifying capability: the finalizer's ed25519 public key plus
//! that key's signature over a fixed message, encoded as `zfinv1` followed by 128 base64url
//! characters (134 characters in total). Consensus rejects a staking action whose target address
//! does not verify, so a wallet must verify an address before it builds a transaction around it.

use zcash_primitives::transaction::{StakingAction, StakingActionKind};

pub use zcash_primitives::bft::{FinalizerAddress, PubKeyID};
pub use zcash_primitives::transaction::is_power_of_10;

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
