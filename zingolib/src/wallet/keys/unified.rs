//! TODO: Add Mod Description Here!

use std::io::{self, Read, Write};

use bip0039::Mnemonic;
use byteorder::{ReadBytesExt, WriteBytesExt};

use pepper_sync::keys::transparent::TransparentScope;
use zcash_address::unified::{Encoding as _, Ufvk};
use zcash_client_backend::address::UnifiedAddress;
use zcash_client_backend::keys::{Era, UnifiedSpendingKey};
use zcash_encoding::CompactSize;
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_protocol::consensus::{NetworkConstants, Parameters};
use zcash_transparent::address::TransparentAddress;
use zcash_transparent::keys::{IncomingViewingKey, NonHardenedChildIndex};
use zip32::{AccountId, DiversifierIndex};

use crate::config::ChainType;
use crate::wallet::error::KeyError;
use crate::wallet::traits::ReadableWriteable;

pub(crate) const KEY_TYPE_EMPTY: u8 = 0;
pub(crate) const KEY_TYPE_VIEW: u8 = 1;
pub(crate) const KEY_TYPE_SPEND: u8 = 2;

/// Unique ID for unified addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct UnifiedAddressId {
    pub account_id: AccountId,
    pub address_index: u32,
}

/// How a wallet's spending keys are derived from its BIP-39 seed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SeedDerivation {
    /// Full 64-byte BIP-39 seed, per ZIP-32. Conforming default.
    #[default]
    Zip32Standard,
    /// First 32 bytes of the 64-byte BIP-39 seed. Matches the zebra-crosslink node.
    CrosslinkTruncated32,
}

impl SeedDerivation {
    pub(crate) fn tag(self) -> u8 {
        match self {
            Self::Zip32Standard => 0,
            Self::CrosslinkTruncated32 => 1,
        }
    }
    pub(crate) fn from_tag(tag: u8) -> std::io::Result<Self> {
        match tag {
            0 => Ok(Self::Zip32Standard),
            1 => Ok(Self::CrosslinkTruncated32),
            other => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown SeedDerivation tag {other}"),
            )),
        }
    }
}

/// In-memory store for wallet spending or viewing keys
#[derive(Debug)]
pub enum UnifiedKeyStore {
    /// Wallet with spend capability
    Spend(Box<UnifiedSpendingKey>),
    /// Wallet with view capability
    View(Box<UnifiedFullViewingKey>),
    /// Wallet with no keys
    Empty,
}

impl UnifiedKeyStore {
    /// Create a unified key store from raw entropy (64-byte seed).
    pub fn new_from_seed(
        network: &ChainType,
        seed: &[u8; 64],
        account_index: zip32::AccountId,
        seed_derivation: SeedDerivation,
    ) -> Result<Self, KeyError> {
        if seed_derivation == SeedDerivation::CrosslinkTruncated32
            && matches!(network, ChainType::Mainnet)
        {
            return Err(KeyError::CrosslinkSeedOnMainnet);
        }
        let seed_slice: &[u8] = match seed_derivation {
            SeedDerivation::Zip32Standard => &seed[..],
            SeedDerivation::CrosslinkTruncated32 => &seed[..32],
        };
        let usk = UnifiedSpendingKey::from_seed(network, seed_slice, account_index)
            .map_err(KeyError::KeyDerivationError)?;

        Ok(UnifiedKeyStore::Spend(Box::new(usk)))
    }

    /// Create a unified key store from a mnemonic.
    ///
    /// Refer to BIP-0039 for details on seed generation from mnemonic phrases.
    pub fn new_from_mnemonic(
        network: &ChainType,
        mnemonic: &Mnemonic,
        account_index: zip32::AccountId,
        seed_derivation: SeedDerivation,
    ) -> Result<Self, KeyError> {
        let seed = mnemonic.to_seed("");
        Self::new_from_seed(network, &seed, account_index, seed_derivation)
    }

    /// Create a unified key store from unified spending key bytes.
    pub fn new_from_usk(usk: &[u8]) -> Result<Self, KeyError> {
        let usk = UnifiedSpendingKey::from_bytes(Era::Orchard, usk)
            .map_err(|_| KeyError::KeyDecodingError)?;

        Ok(UnifiedKeyStore::Spend(Box::new(usk)))
    }

    /// Create a unified key store from unified full viewing key encoded string.
    pub fn new_from_ufvk(network: &ChainType, ufvk_encoded: String) -> Result<Self, KeyError> {
        if ufvk_encoded.starts_with(network.hrp_sapling_extended_full_viewing_key()) {
            return Err(KeyError::InvalidFormat);
        }
        let (network_type, ufvk) =
            Ufvk::decode(&ufvk_encoded).map_err(|_| KeyError::KeyDecodingError)?;
        if network_type != network.network_type() {
            return Err(KeyError::NetworkMismatch);
        }
        let ufvk = UnifiedFullViewingKey::parse(&ufvk).map_err(|_| KeyError::KeyDecodingError)?;

        Ok(UnifiedKeyStore::View(Box::new(ufvk)))
    }

    /// Returns true if [`UnifiedKeyStore`] is of `Spend` variant
    #[must_use]
    pub fn is_spending_key(&self) -> bool {
        matches!(self, UnifiedKeyStore::Spend(_))
    }

    /// Returns true if [`UnifiedKeyStore`] is of `Empty` variant
    #[must_use]
    pub fn is_empty(&self) -> bool {
        matches!(self, UnifiedKeyStore::Empty)
    }

    /// Returns the default receivers for unified address generation depending on the wallet's capability.
    /// Returns `None` if the wallet does not have viewing capabilities of at least 1 shielded pool.
    #[must_use]
    pub fn default_receivers(&self) -> Option<ReceiverSelection> {
        match self {
            UnifiedKeyStore::Spend(_) => Some(ReceiverSelection::orchard_only()),
            UnifiedKeyStore::View(ufvk) => {
                if ufvk.orchard().is_some() {
                    Some(ReceiverSelection::orchard_only())
                } else if ufvk.sapling().is_some() {
                    Some(ReceiverSelection::sapling_only())
                } else {
                    None
                }
            }
            UnifiedKeyStore::Empty => None,
        }
    }

    /// Generates a unified address for the given `unified_address_index` and `receivers`.
    pub fn generate_unified_address(
        &self,
        unified_address_index: u32,
        receivers: ReceiverSelection,
    ) -> Result<UnifiedAddress, KeyError> {
        let orchard_receiver = if receivers.orchard {
            let fvk = orchard::keys::FullViewingKey::try_from(self)?;
            Some(fvk.address_at(unified_address_index, orchard::keys::Scope::External))
        } else {
            None
        };

        let sapling_receiver = if receivers.sapling {
            Some(self.derive_sapling_address(unified_address_index)?)
        } else {
            None
        };

        let unified_address =
            UnifiedAddress::from_receivers(orchard_receiver, sapling_receiver, None)
                .ok_or(KeyError::UnifiedAddressError)?;

        Ok(unified_address)
    }

    /// Generates a transparent address for the given `address_index` and `scope`.
    pub fn generate_transparent_address(
        &self,
        address_index: NonHardenedChildIndex,
        scope: TransparentScope,
    ) -> Result<TransparentAddress, KeyError> {
        let account_pubkey = UnifiedFullViewingKey::try_from(self)?
            .transparent()
            .ok_or(KeyError::NoViewCapability)?
            .clone();

        let transparent_address = match scope {
            TransparentScope::External => account_pubkey
                .derive_external_ivk()?
                .derive_address(address_index)?,
            TransparentScope::Internal => account_pubkey
                .derive_internal_ivk()?
                .derive_address(address_index)?,
            TransparentScope::Refund => account_pubkey
                .derive_ephemeral_ivk()?
                .derive_ephemeral_address(address_index)?,
        };

        Ok(transparent_address)
    }

    fn derive_sapling_address(
        &self,
        unified_address_index: u32,
    ) -> Result<sapling_crypto::PaymentAddress, KeyError> {
        let fvk = sapling_crypto::zip32::DiversifiableFullViewingKey::try_from(self)?;
        let mut address;
        let mut diversifier_index = DiversifierIndex::new();
        let mut valid_diversifier_count = 0;

        // not all sapling diversifier indexes produce valid sapling diversifiers.
        // therefore, `diversifier_index` may be larger than `unified_address_index` as only the valid payment
        // addresses are counted.
        loop {
            (diversifier_index, address) = fvk
                .find_address(diversifier_index)
                .expect("diversifier index overflow");
            valid_diversifier_count += 1;
            if valid_diversifier_count - 1 == unified_address_index {
                break;
            }

            diversifier_index
                .increment()
                .expect("diversifier index overflow");
        }

        Ok(address)
    }

    /// Returns the number of valid sapling diversifiers when incrementing from 0 to `sapling_diversifier_index` inclusive.
    ///
    /// For example, if 10 is returned, the `sapling_diversifier_index` is associated with the 10th valid sapling
    /// diversifier when incrementing from a diversifier index of 0.
    pub(crate) fn determine_nth_valid_sapling_diversifier(
        &self,
        sapling_diversifier_index: DiversifierIndex,
    ) -> Result<u32, KeyError> {
        let fvk = sapling_crypto::zip32::DiversifiableFullViewingKey::try_from(self)?;
        let mut _address;
        let mut diversifier_index = DiversifierIndex::new();
        let mut valid_diversifier_count = 0;

        loop {
            (diversifier_index, _address) = fvk
                .find_address(diversifier_index)
                .expect("diversifier index overflow");
            valid_diversifier_count += 1;
            if diversifier_index == sapling_diversifier_index {
                break;
            }

            diversifier_index
                .increment()
                .expect("diversifier index overflow");
        }

        Ok(valid_diversifier_count)
    }
}

impl ReadableWriteable<ChainType, ChainType> for UnifiedKeyStore {
    const VERSION: u8 = 0;

    fn read<R: Read>(mut reader: R, input: ChainType) -> io::Result<Self> {
        let _version = Self::get_version(&mut reader)?;
        let key_type = reader.read_u8()?;
        Ok(match key_type {
            KEY_TYPE_SPEND => {
                UnifiedKeyStore::Spend(Box::new(UnifiedSpendingKey::read(reader, ())?))
            }
            KEY_TYPE_VIEW => {
                UnifiedKeyStore::View(Box::new(UnifiedFullViewingKey::read(reader, input)?))
            }
            KEY_TYPE_EMPTY => UnifiedKeyStore::Empty,
            x => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Unknown key type: {x}"),
                ));
            }
        })
    }

    fn write<W: Write>(&self, mut writer: W, input: ChainType) -> io::Result<()> {
        writer.write_u8(Self::VERSION)?;
        match self {
            UnifiedKeyStore::Spend(usk) => {
                writer.write_u8(KEY_TYPE_SPEND)?;
                usk.write(&mut writer, ())
            }
            UnifiedKeyStore::View(ufvk) => {
                writer.write_u8(KEY_TYPE_VIEW)?;
                ufvk.write(&mut writer, input)
            }
            UnifiedKeyStore::Empty => writer.write_u8(KEY_TYPE_EMPTY),
        }
    }
}
impl ReadableWriteable for UnifiedSpendingKey {
    const VERSION: u8 = 0;

    fn read<R: Read>(mut reader: R, _input: ()) -> io::Result<Self> {
        let len = CompactSize::read(&mut reader)?;
        let mut usk = vec![0u8; len as usize];
        reader.read_exact(&mut usk)?;

        UnifiedSpendingKey::from_bytes(Era::Orchard, &usk)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "USK bytes are invalid"))
    }

    fn write<W: Write>(&self, mut writer: W, _input: ()) -> io::Result<()> {
        let usk_bytes = self.to_bytes(Era::Orchard);
        CompactSize::write(&mut writer, usk_bytes.len())?;
        writer.write_all(&usk_bytes)?;
        Ok(())
    }
}
impl ReadableWriteable<ChainType, ChainType> for UnifiedFullViewingKey {
    const VERSION: u8 = 0;

    fn read<R: Read>(mut reader: R, input: ChainType) -> io::Result<Self> {
        let len = CompactSize::read(&mut reader)?;
        let mut ufvk = vec![0u8; len as usize];
        reader.read_exact(&mut ufvk)?;
        let ufvk_encoded = std::str::from_utf8(&ufvk)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

        UnifiedFullViewingKey::decode(&input, ufvk_encoded).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("UFVK decoding error: {e}"),
            )
        })
    }

    fn write<W: Write>(&self, mut writer: W, input: ChainType) -> io::Result<()> {
        let ufvk_bytes = self.encode(&input).as_bytes().to_vec();
        CompactSize::write(&mut writer, ufvk_bytes.len())?;
        writer.write_all(&ufvk_bytes)?;
        Ok(())
    }
}

impl TryFrom<&UnifiedKeyStore> for UnifiedSpendingKey {
    type Error = KeyError;
    fn try_from(unified_key_store: &UnifiedKeyStore) -> Result<Self, Self::Error> {
        match unified_key_store {
            UnifiedKeyStore::Spend(usk) => Ok(*usk.clone()),
            _ => Err(KeyError::NoSpendCapability),
        }
    }
}
impl TryFrom<&UnifiedKeyStore> for orchard::keys::SpendingKey {
    type Error = KeyError;
    fn try_from(unified_key_store: &UnifiedKeyStore) -> Result<Self, Self::Error> {
        let usk = UnifiedSpendingKey::try_from(unified_key_store)?;
        Ok(*usk.orchard())
    }
}
impl TryFrom<&UnifiedKeyStore> for sapling_crypto::zip32::ExtendedSpendingKey {
    type Error = KeyError;
    fn try_from(unified_key_store: &UnifiedKeyStore) -> Result<Self, Self::Error> {
        let usk = UnifiedSpendingKey::try_from(unified_key_store)?;
        Ok(usk.sapling().clone())
    }
}
impl TryFrom<&UnifiedKeyStore> for zcash_transparent::keys::AccountPrivKey {
    type Error = KeyError;
    fn try_from(unified_key_store: &UnifiedKeyStore) -> Result<Self, Self::Error> {
        let usk = UnifiedSpendingKey::try_from(unified_key_store)?;
        Ok(usk.transparent().clone())
    }
}

impl TryFrom<&UnifiedKeyStore> for UnifiedFullViewingKey {
    type Error = KeyError;
    fn try_from(unified_key_store: &UnifiedKeyStore) -> Result<Self, Self::Error> {
        match unified_key_store {
            UnifiedKeyStore::Spend(usk) => Ok(usk.to_unified_full_viewing_key()),
            UnifiedKeyStore::View(ufvk) => Ok(*ufvk.clone()),
            UnifiedKeyStore::Empty => Err(KeyError::NoViewCapability),
        }
    }
}
impl TryFrom<&UnifiedKeyStore> for orchard::keys::FullViewingKey {
    type Error = KeyError;
    fn try_from(unified_key_store: &UnifiedKeyStore) -> Result<Self, Self::Error> {
        let ufvk = UnifiedFullViewingKey::try_from(unified_key_store)?;
        ufvk.orchard().ok_or(KeyError::NoViewCapability).cloned()
    }
}
impl TryFrom<&UnifiedKeyStore> for sapling_crypto::zip32::DiversifiableFullViewingKey {
    type Error = KeyError;
    fn try_from(unified_key_store: &UnifiedKeyStore) -> Result<Self, Self::Error> {
        let ufvk = UnifiedFullViewingKey::try_from(unified_key_store)?;
        ufvk.sapling().ok_or(KeyError::NoViewCapability).cloned()
    }
}
impl TryFrom<&UnifiedKeyStore> for zcash_transparent::keys::AccountPubKey {
    type Error = KeyError;
    fn try_from(unified_key_store: &UnifiedKeyStore) -> Result<Self, Self::Error> {
        let ufvk = UnifiedFullViewingKey::try_from(unified_key_store)?;
        ufvk.transparent()
            .ok_or(KeyError::NoViewCapability)
            .cloned()
    }
}

/// Selects the receivers for the creation of a new unified address.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Deserialize, serde::Serialize)]
pub struct ReceiverSelection {
    /// Orchard
    pub orchard: bool,
    /// Sapling
    pub sapling: bool,
}

impl ReceiverSelection {
    /// All shielded receivers.
    #[must_use]
    pub fn all_shielded() -> Self {
        Self {
            orchard: true,
            sapling: true,
        }
    }

    /// Only orchard receiver.
    #[must_use]
    pub fn orchard_only() -> Self {
        Self {
            orchard: true,
            sapling: false,
        }
    }

    /// Only sapling receiver.
    #[must_use]
    pub fn sapling_only() -> Self {
        Self {
            orchard: false,
            sapling: true,
        }
    }
}

impl ReadableWriteable for ReceiverSelection {
    const VERSION: u8 = 2;

    fn read<R: Read>(mut reader: R, _input: ()) -> io::Result<Self> {
        let _version = Self::get_version(&mut reader)?;
        let receivers = reader.read_u8()?;
        Ok(Self {
            orchard: receivers & 0b1 != 0,
            sapling: receivers & 0b10 != 0,
        })
    }

    fn write<W: Write>(&self, mut writer: W, _input: ()) -> io::Result<()> {
        writer.write_u8(Self::VERSION)?;
        let mut receivers = 0;
        if self.orchard {
            receivers |= 0b1;
        }
        if self.sapling {
            receivers |= 0b10;
        }
        writer.write_u8(receivers)?;
        Ok(())
    }
}

#[test]
fn read_write_receiver_selections() {
    for (i, receivers_selected) in (0..4)
        .map(|n| ReceiverSelection::read([2, n].as_slice(), ()).unwrap())
        .enumerate()
    {
        let mut receivers_selected_bytes = [0; 2];
        receivers_selected
            .write(receivers_selected_bytes.as_mut_slice(), ())
            .unwrap();
        assert_eq!(i as u8, receivers_selected_bytes[1]);
    }
}

#[cfg(test)]
mod seed_derivation_tests {
    use super::{ReceiverSelection, SeedDerivation, UnifiedKeyStore};
    use crate::config::ChainType;
    use crate::wallet::error::KeyError;
    use bip0039::Mnemonic;
    use zip32::AccountId;

    /// Canonical BIP-39 24-word test vector (all-zero entropy, checksum word "art").
    const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon \
         abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon \
         abandon abandon abandon abandon art";

    /// A non-mainnet network to exercise crosslink derivation (rejected on mainnet).
    fn non_mainnet() -> ChainType {
        crate::config::ZingoConfig::create_testnet().chain
    }

    /// Derives account 0's first orchard-only unified address, encoded for `network`.
    fn encoded_unified_address(network: &ChainType, seed_derivation: SeedDerivation) -> String {
        let mnemonic = Mnemonic::from_phrase(TEST_MNEMONIC.to_string()).unwrap();
        let key_store =
            UnifiedKeyStore::new_from_mnemonic(network, &mnemonic, AccountId::ZERO, seed_derivation)
                .unwrap();
        key_store
            .generate_unified_address(0, ReceiverSelection::orchard_only())
            .unwrap()
            .encode(network)
    }

    #[test]
    fn crosslink_truncation_differs() {
        let network = non_mainnet();
        let standard = encoded_unified_address(&network, SeedDerivation::Zip32Standard);
        let truncated = encoded_unified_address(&network, SeedDerivation::CrosslinkTruncated32);
        assert_ne!(
            standard, truncated,
            "truncated-32 derivation must yield a different address than standard zip32"
        );
    }

    #[test]
    fn crosslink_truncation_is_deterministic() {
        let network = non_mainnet();
        let first = encoded_unified_address(&network, SeedDerivation::CrosslinkTruncated32);
        let second = encoded_unified_address(&network, SeedDerivation::CrosslinkTruncated32);
        assert_eq!(first, second);
    }

    #[test]
    fn crosslink_rejected_on_mainnet() {
        let mnemonic = Mnemonic::from_phrase(TEST_MNEMONIC.to_string()).unwrap();
        let result = UnifiedKeyStore::new_from_mnemonic(
            &ChainType::Mainnet,
            &mnemonic,
            AccountId::ZERO,
            SeedDerivation::CrosslinkTruncated32,
        );
        assert!(matches!(result, Err(KeyError::CrosslinkSeedOnMainnet)));
    }

    /// Golden interop vector: the `(mnemonic -> transparent address, unified address)` triple that a
    /// real zebra-crosslink node reports for the same seed under crosslink (truncated-32) derivation.
    #[test]
    #[ignore = "TODO: paste node-reported addresses; see issue on seed-derivation interop"]
    fn crosslink_golden_vector_matches_zebra_node() {
        use pepper_sync::keys::transparent::{self, TransparentScope};
        use zcash_transparent::keys::NonHardenedChildIndex;

        // The zebra-crosslink faucet MINER wallet uses this canonical 12-word phrase
        // (hardcoded upstream in zebra-crosslink `wallet/src/lib.rs`, entropy = 16 zero
        // bytes). It flows through the node's truncating `stuff_from_seed_phrase`, so its
        // addresses ARE a valid crosslink-truncated golden vector. The node only prints
        // them at runtime ("MINER WALLET T-ADDRESS:" / "MINER WALLET ADDRESS:") — never in
        // source — so paste those exact strings below.
        const GOLDEN_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon \
             abandon abandon abandon abandon about";
        // TODO: paste the addresses a real zebra-crosslink node reports for GOLDEN_MNEMONIC.
        //       DO NOT invent these; obtain them from the node's MINER WALLET output.
        const EXPECTED_TRANSPARENT_ADDRESS: &str = "TODO_PASTE_NODE_TRANSPARENT_ADDRESS";
        const EXPECTED_UNIFIED_ADDRESS: &str = "TODO_PASTE_NODE_UNIFIED_ADDRESS";

        let network = non_mainnet();
        let mnemonic = Mnemonic::from_phrase(GOLDEN_MNEMONIC.to_string()).unwrap();
        let key_store = UnifiedKeyStore::new_from_mnemonic(
            &network,
            &mnemonic,
            AccountId::ZERO,
            SeedDerivation::CrosslinkTruncated32,
        )
        .unwrap();

        let unified_address = key_store
            .generate_unified_address(0, ReceiverSelection::orchard_only())
            .unwrap()
            .encode(&network);
        let transparent_address = transparent::encode_address(
            &network,
            key_store
                .generate_transparent_address(
                    NonHardenedChildIndex::ZERO,
                    TransparentScope::External,
                )
                .unwrap(),
        );

        assert_eq!(transparent_address, EXPECTED_TRANSPARENT_ADDRESS);
        assert_eq!(unified_address, EXPECTED_UNIFIED_ADDRESS);
    }
}
