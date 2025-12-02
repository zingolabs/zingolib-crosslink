use std::str::FromStr;

use bip0039::{English, Mnemonic};
use zcash_address::ZcashAddress;
use zcash_keys::keys::{UnifiedAddressRequest, UnifiedFullViewingKey, UnifiedSpendingKey};
use zcash_protocol::{consensus::BlockHeight, local_consensus::LocalNetwork};
use zip32::DiversifierIndex;

const LOCAL_NETWORK: LocalNetwork = LocalNetwork {
    overwinter: Some(BlockHeight::from_u32(1)),
    sapling: Some(BlockHeight::from_u32(1)),
    blossom: Some(BlockHeight::from_u32(1)),
    heartwood: Some(BlockHeight::from_u32(1)),
    canopy: Some(BlockHeight::from_u32(1)),
    nu5: Some(BlockHeight::from_u32(1)),
    nu6: Some(BlockHeight::from_u32(1)),
    nu6_1: None,
};

const CROSSLINK_TESTNET: LocalNetwork = LocalNetwork {
    overwinter: Some(BlockHeight::from_u32(1)),
    sapling: Some(BlockHeight::from_u32(1)),
    blossom: Some(BlockHeight::from_u32(1)),
    heartwood: Some(BlockHeight::from_u32(1)),
    canopy: Some(BlockHeight::from_u32(1)),
    nu5: Some(BlockHeight::from_u32(1)),
    nu6: Some(BlockHeight::from_u32(1)),
    nu6_1: None,
};

pub fn miner_unified_address() -> ZcashAddress {
    let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    let mnemonic: Mnemonic<English> = Mnemonic::from_str(phrase).unwrap();

    // Same as your wallet code: take first 32 bytes
    let seed64 = mnemonic.to_seed("");
    let seed32 = &seed64[..32];
    let account_id = zip32::AccountId::try_from(0).unwrap();

    let usk = UnifiedSpendingKey::from_seed(&LOCAL_NETWORK, seed32, account_id).unwrap();
    let ufvk = usk.to_unified_full_viewing_key();

    // Orchard-only address at diversifier index 0, to match `addrs_from_account`
    let (ua, _di) = ufvk
        .find_address(DiversifierIndex::new(), UnifiedAddressRequest::ORCHARD)
        .unwrap();

    let encoded_ua = ua.encode(&LOCAL_NETWORK);

    ZcashAddress::from_str(&encoded_ua).unwrap()
}
