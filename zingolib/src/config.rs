//! `ZingConfig`
//! TODO: Add Crate Description Here!

#![forbid(unsafe_code)]
#![warn(missing_docs)]
use std::{
    io::{self, Error},
    num::NonZeroU32,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use log::{LevelFilter, info};
use log4rs::{
    Config,
    append::rolling_file::{
        RollingFileAppender,
        policy::compound::{
            CompoundPolicy, roll::fixed_window::FixedWindowRoller, trigger::size::SizeTrigger,
        },
    },
    config::{Appender, Root},
    encode::pattern::PatternEncoder,
    filter::threshold::ThresholdFilter,
};
use zcash_protocol::consensus::{
    BlockHeight, MAIN_NETWORK, NetworkType, NetworkUpgrade, Parameters,
};

use crate::wallet::WalletSettings;

pub mod indexers;

pub use indexers::{Indexer, IndexerList, IndexerNetwork};
pub use zingo_netutils::discovery;

/// TODO: Add Doc Comment Here!
pub const DEVELOPER_DONATION_ADDRESS: &str = "u1w47nzy4z5g9zvm4h2s4ztpl8vrdmlclqz5sz02742zs5j3tz232u4safvv9kplg7g06wpk5fx0k0rx3r9gg4qk6nkg4c0ey57l0dyxtatqf8403xat7vyge7mmen7zwjcgvryg22khtg3327s6mqqkxnpwlnrt27kxhwg37qys2kpn2d2jl2zkk44l7j7hq9az82594u3qaescr3c9v";
/// Regtest address for donation in test environments
pub const ZENNIES_FOR_ZINGO_REGTEST_ADDRESS: &str = "uregtest14emvr2anyul683p43d0ck55c04r65ld6f0shetcn77z8j7m64hm4ku3wguf60s75f0g3s7r7g89z22f3ff5tsfgr45efj4pe2gyg5krqp5vvl3afu0280zp9ru2379zat5y6nkqkwjxsvpq5900kchcgzaw8v8z3ggt5yymnuj9hymtv3p533fcrk2wnj48g5vg42vle08c2xtanq0e";

/// Gets the appropriate donation address for the given chain type
#[must_use]
pub fn get_donation_address_for_chain(chain: &ChainType) -> &'static str {
    match chain {
        ChainType::Testnet(_) => ZENNIES_FOR_ZINGO_TESTNET_ADDRESS,
        ChainType::Mainnet => ZENNIES_FOR_ZINGO_DONATION_ADDRESS,
        ChainType::Regtest(_) => ZENNIES_FOR_ZINGO_REGTEST_ADDRESS,
    }
}

/// Configurable network upgrade activation heights for a test or regtest network.
///
/// Mirrors the shape of zebra's `testnet::ConfiguredActivationHeights`, extended with the network
/// upgrades the Crosslink node knows about. `None` means the upgrade never activates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[allow(missing_docs)]
pub struct ConfiguredActivationHeights {
    pub before_overwinter: Option<u32>,
    pub overwinter: Option<u32>,
    pub sapling: Option<u32>,
    pub blossom: Option<u32>,
    pub heartwood: Option<u32>,
    pub canopy: Option<u32>,
    pub nu5: Option<u32>,
    pub nu6: Option<u32>,
    pub nu6_1: Option<u32>,
    pub nu6_2: Option<u32>,
    pub nu6_3: Option<u32>,
    /// The first Crosslink featurenet hard fork (power-of-10 bonds, top-level PoS issuance split).
    pub featurenet_upgrade_1: Option<u32>,
    pub nu7: Option<u32>,
}

impl ConfiguredActivationHeights {
    /// The activation schedule of the Crosslink featurenet, as shipped in the node.
    ///
    /// Every upgrade through NU6.3 (which brings the Ironwood pool) activates at height 1, and
    /// `FeaturenetUpgrade1` activates at the height hard-coded in `zcash_protocol`. The wallet and
    /// the node must agree on these, or every transaction the wallet builds carries a consensus
    /// branch ID the node rejects.
    #[must_use]
    pub fn crosslink_featurenet() -> Self {
        Self {
            before_overwinter: Some(1),
            overwinter: Some(1),
            sapling: Some(1),
            blossom: Some(1),
            heartwood: Some(1),
            canopy: Some(1),
            nu5: Some(1),
            nu6: Some(1),
            nu6_1: Some(1),
            nu6_2: Some(1),
            nu6_3: Some(1),
            featurenet_upgrade_1: Some(
                zcash_protocol::consensus::TESTNET_FEATURENET_UPGRADE_1_HEIGHT,
            ),
            nu7: None,
        }
    }

    /// A regtest schedule with every upgrade through NU6.3 active from height 1 and no
    /// `FeaturenetUpgrade1`.
    #[must_use]
    pub fn regtest_all_active() -> Self {
        Self {
            featurenet_upgrade_1: None,
            ..Self::crosslink_featurenet()
        }
    }
}

/// The networks a zingolib client can run against
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChainType {
    /// Public testnet
    Testnet(ConfiguredActivationHeights),
    /// Mainnet
    Mainnet,
    /// Regtest
    Regtest(ConfiguredActivationHeights),
}

impl std::fmt::Display for ChainType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use ChainType::{Mainnet, Regtest, Testnet};
        let name = match self {
            Testnet(_) => "test",
            Mainnet => "main",
            Regtest(_) => "regtest",
        };
        write!(f, "{name}")
    }
}

impl Parameters for ChainType {
    fn network_type(&self) -> NetworkType {
        match self {
            ChainType::Testnet(_) => NetworkType::Test,
            ChainType::Mainnet => NetworkType::Main,
            ChainType::Regtest(_) => NetworkType::Regtest,
        }
    }

    fn activation_height(&self, nu: NetworkUpgrade) -> Option<BlockHeight> {
        use ChainType::{Mainnet, Regtest, Testnet};
        match self {
            Testnet(activation_heights) => match nu {
                NetworkUpgrade::Overwinter => {
                    activation_heights.overwinter.map(BlockHeight::from_u32)
                }
                NetworkUpgrade::Sapling => activation_heights.sapling.map(BlockHeight::from_u32),
                NetworkUpgrade::Blossom => activation_heights.blossom.map(BlockHeight::from_u32),
                NetworkUpgrade::Heartwood => {
                    activation_heights.heartwood.map(BlockHeight::from_u32)
                }
                NetworkUpgrade::Canopy => activation_heights.canopy.map(BlockHeight::from_u32),
                NetworkUpgrade::Nu5 => activation_heights.nu5.map(BlockHeight::from_u32),
                NetworkUpgrade::Nu6 => activation_heights.nu6.map(BlockHeight::from_u32),
                NetworkUpgrade::Nu6_1 => activation_heights.nu6_1.map(BlockHeight::from_u32),
                NetworkUpgrade::Nu6_2 => activation_heights.nu6_2.map(BlockHeight::from_u32),
                NetworkUpgrade::Nu6_3 => activation_heights.nu6_3.map(BlockHeight::from_u32),
                NetworkUpgrade::FeaturenetUpgrade1 => activation_heights
                    .featurenet_upgrade_1
                    .map(BlockHeight::from_u32),
            },
            Mainnet => MAIN_NETWORK.activation_height(nu),
            Regtest(activation_heights) => match nu {
                NetworkUpgrade::Overwinter => {
                    activation_heights.overwinter.map(BlockHeight::from_u32)
                }
                NetworkUpgrade::Sapling => activation_heights.sapling.map(BlockHeight::from_u32),
                NetworkUpgrade::Blossom => activation_heights.blossom.map(BlockHeight::from_u32),
                NetworkUpgrade::Heartwood => {
                    activation_heights.heartwood.map(BlockHeight::from_u32)
                }
                NetworkUpgrade::Canopy => activation_heights.canopy.map(BlockHeight::from_u32),
                NetworkUpgrade::Nu5 => activation_heights.nu5.map(BlockHeight::from_u32),
                NetworkUpgrade::Nu6 => activation_heights.nu6.map(BlockHeight::from_u32),
                NetworkUpgrade::Nu6_1 => activation_heights.nu6_1.map(BlockHeight::from_u32),
                NetworkUpgrade::Nu6_2 => activation_heights.nu6_2.map(BlockHeight::from_u32),
                NetworkUpgrade::Nu6_3 => activation_heights.nu6_3.map(BlockHeight::from_u32),
                NetworkUpgrade::FeaturenetUpgrade1 => activation_heights
                    .featurenet_upgrade_1
                    .map(BlockHeight::from_u32),
            },
        }
    }
}

/// An error determining chain id and parameters '`ChainType`' from string.
#[derive(thiserror::Error, Debug)]
pub enum ChainFromStringError {
    /// Invalid chain name. Expected one of: mainnet, testnet or regtest.
    #[error("Invalid chain name '{0}'. Expected one of: mainnet, testnet or regtest.")]
    UnknownChain(String),
    /// "regtest" feature is not enabled.
    #[error("\"regtest\" feature is not enabled.")]
    RegtestFeatureNotEnabled,
}

/// Converts a chain name string to a `ChainType` variant.
///
/// # Arguments
/// * `chain_name` - The chain name as a string
///
/// # Returns
/// * `Ok(ChainType)` - The corresponding `ChainType` variant
/// * `Err(String)` - An error message if the chain name is invalid
pub fn chain_from_str(chain_name: &str) -> Result<ChainType, ChainFromStringError> {
    match chain_name {
        "testnet" => Ok(ChainType::Testnet(
            ConfiguredActivationHeights::crosslink_featurenet(),
        )),
        "mainnet" => Ok(ChainType::Mainnet),
        "regtest" => Ok(ChainType::Regtest(
            ConfiguredActivationHeights::regtest_all_active(),
        )),
        _ => Err(ChainFromStringError::UnknownChain(chain_name.to_string())),
    }
}

/// TODO: Add Doc Comment Here!
pub const ZENNIES_FOR_ZINGO_TESTNET_ADDRESS: &str = "utest19zd9laj93deq4lkay48xcfyh0tjec786x6yrng38fp6zusgm0c84h3el99fngh8eks4kxv020r2h2njku6pf69anpqmjq5c3suzcjtlyhvpse0aqje09la48xk6a2cnm822s2yhuzfr47pp4dla9rakdk90g0cee070z57d3trqk87wwj4swz6uf6ts6p5z6lep3xyvueuvt7392tww";
/// TODO: Add Doc Comment Here!
pub const ZENNIES_FOR_ZINGO_DONATION_ADDRESS: &str = "u1p32nu0pgev5cr0u6t4ja9lcn29kaw37xch8nyglwvp7grl07f72c46hxvw0u3q58ks43ntg324fmulc2xqf4xl3pv42s232m25vaukp05s6av9z76s3evsstax4u6f5g7tql5yqwuks9t4ef6vdayfmrsymenqtshgxzj59hdydzygesqa7pdpw463hu7afqf4an29m69kfasdwr494";
/// TODO: Add Doc Comment Here!
pub const ZENNIES_FOR_ZINGO_AMOUNT: u64 = 1_000_000;
/// The indexer that handles blockchain requests
pub const DEFAULT_INDEXER_URI: &str = "https://zec.rocks:443";
/// Used for testnet
pub const DEFAULT_TESTNET_INDEXER_URI: &str = "https://testnet.zec.rocks";
/// TODO: Add Doc Comment Here!
pub const DEFAULT_WALLET_NAME: &str = "zingo-wallet.dat";
/// TODO: Add Doc Comment Here!
pub const DEFAULT_LOGFILE_NAME: &str = "zingo-wallet.debug.log";

/// Re-export pepper-sync `SyncConfig` for use with `load_clientconfig`
///
pub use pepper_sync::config::{SyncConfig, TransparentAddressDiscovery};

/// Creates a zingo config for lightclient construction.
pub fn load_clientconfig(
    lightwallet_uri: http::Uri,
    data_dir: Option<PathBuf>,
    chain: ChainType,
    wallet_settings: WalletSettings,
    no_of_accounts: NonZeroU32,
    wallet_name: String,
) -> std::io::Result<ZingoConfig> {
    use std::net::ToSocketAddrs;

    let host = lightwallet_uri.host();
    let port = lightwallet_uri.port();

    if host.is_none() || port.is_none() {
        info!("Using offline mode");
    } else {
        match format!(
            "{}:{}",
            lightwallet_uri.host().unwrap(),
            lightwallet_uri.port().unwrap()
        )
        .to_socket_addrs()
        {
            Ok(_) => {
                info!("Connected to {lightwallet_uri}");
            }
            Err(e) => {
                info!("Couldn't resolve indexer: {e}");
            }
        }
    }

    // if id is empty, the name is the default name
    let wallet_name_config = if wallet_name.is_empty() {
        DEFAULT_WALLET_NAME
    } else {
        &wallet_name
    };

    // The configured endpoint heads the chain's list, ahead of the built-in ones.
    let indexer_network = IndexerNetwork::of_chain(&chain);
    let mut indexers = IndexerList::builtin();
    if lightwallet_uri.host().is_some() && lightwallet_uri.port().is_some() {
        indexers.prefer(
            indexer_network.clone(),
            Indexer::new(lightwallet_uri.clone(), "configured"),
        );
    }

    // Create a Light Client Config
    let config = ZingoConfig {
        lightwalletd_uri: Arc::new(RwLock::new(lightwallet_uri)),
        indexers: Arc::new(RwLock::new(indexers)),
        indexer_network,
        chain,
        wallet_dir: data_dir,
        wallet_name: wallet_name_config.into(),
        logfile_name: DEFAULT_LOGFILE_NAME.into(),
        wallet_settings,
        no_of_accounts,
    };

    Ok(config)
}

/// TODO: Add Doc Comment Here!
#[must_use]
pub fn construct_lightwalletd_uri(indexer: Option<String>) -> http::Uri {
    match indexer {
        Some(s) => {
            if s.is_empty() {
                return http::Uri::default();
            } else {
                let mut s = if s.starts_with("http") {
                    s
                } else {
                    "http://".to_string() + &s
                };
                let uri: http::Uri = s.parse().unwrap();
                if uri.port().is_none() {
                    s += ":9067";
                }
                s
            }
        }
        None => DEFAULT_INDEXER_URI.to_string(),
    }
    .parse()
    .unwrap()
}

/// TODO: Add Doc Comment Here!
#[derive(Clone, Debug)]
pub struct ZingoConfigBuilder {
    /// The endpoint the client connects to. When `None` and `indexers` is set, the first endpoint
    /// listed for the chain's network; when both are unset, no indexer (offline).
    pub lightwalletd_uri: Option<http::Uri>,
    /// The endpoints the client may switch between, per network. Defaults to
    /// [`IndexerList::builtin`] when `None`.
    pub indexers: Option<IndexerList>,
    /// Which network's endpoints this client uses. Defaults to
    /// [`IndexerNetwork::of_chain`] for `chain`; set it to pick a [`IndexerNetwork::Custom`]
    /// deployment that shares the chain's parameters.
    pub indexer_network: Option<IndexerNetwork>,
    /// TODO: Add Doc Comment Here!
    pub chain: ChainType,
    /// TODO: Add Doc Comment Here!
    pub reorg_buffer_offset: Option<u32>,
    /// TODO: Add Doc Comment Here!
    pub monitor_mempool: Option<bool>,
    /// The directory where the wallet and logfiles will be created. By default, this will be in ~/.zcash on Linux and %APPDATA%\Zcash on Windows. For mac it is in: ~/Library/Application Support/Zcash
    pub wallet_dir: Option<PathBuf>,
    /// The filename of the wallet. This will be created in the `wallet_dir`.
    pub wallet_name: Option<PathBuf>,
    /// The filename of the logfile. This will be created in the `wallet_dir`.
    pub logfile_name: Option<PathBuf>,
    /// Wallet settings.
    pub wallet_settings: WalletSettings,
    /// Number of accounts
    pub no_of_accounts: NonZeroU32,
}

/// Configuration data for the creation of a `LightClient`.
// TODO: this config should only be used to create a lightclient, the data should then be moved into fields of
// lightclient or lightwallet if it needs to retained in memory.
#[derive(Clone, Debug)]
pub struct ZingoConfig {
    /// The endpoint every request goes to. Swap it with
    /// [`crate::lightclient::LightClient::set_indexer`].
    pub lightwalletd_uri: Arc<RwLock<http::Uri>>,
    /// The endpoints the client may switch between, per network; see [`IndexerList`].
    pub indexers: Arc<RwLock<IndexerList>>,
    /// Which network's endpoints this client uses.
    pub indexer_network: IndexerNetwork,
    /// TODO: Add Doc Comment Here!
    pub chain: ChainType,
    /// The directory where the wallet and logfiles will be created. By default, this will be in ~/.zcash on Linux and %APPDATA%\Zcash on Windows.
    pub wallet_dir: Option<PathBuf>,
    /// The filename of the wallet. This will be created in the `wallet_dir`.
    pub wallet_name: PathBuf,
    /// The filename of the logfile. This will be created in the `wallet_dir`.
    pub logfile_name: PathBuf,
    /// Wallet settings.
    pub wallet_settings: WalletSettings,
    /// Number of accounts
    pub no_of_accounts: NonZeroU32,
}

impl ZingoConfigBuilder {
    /// Set the URI of the proxy indexer we download blockchain information from.
    /// # Examples
    /// ```
    /// use zingolib::config::ZingoConfigBuilder;
    /// use http::Uri;
    /// assert_eq!(ZingoConfigBuilder::default().set_lightwalletd_uri(("https://zcash.mysideoftheweb.com:19067").parse::<Uri>().unwrap()).lightwalletd_uri.clone().unwrap(), "https://zcash.mysideoftheweb.com:19067");
    /// ```
    pub fn set_lightwalletd_uri(&mut self, lightwalletd_uri: http::Uri) -> &mut Self {
        self.lightwalletd_uri = Some(lightwalletd_uri);
        self
    }

    /// Replaces the endpoint list. Start from [`IndexerList::builtin`] to keep the shipped
    /// endpoints or from [`IndexerList::empty`] to supply every endpoint yourself.
    ///
    /// When no URI is set with [`Self::set_lightwalletd_uri`], the client connects to the first
    /// endpoint listed for the chain's network.
    /// # Examples
    /// ```
    /// use zingolib::config::{ChainType, Indexer, IndexerList, IndexerNetwork, ZingoConfigBuilder};
    /// let mine = Indexer::parse("10.0.0.1:18234").unwrap();
    /// let config = ZingoConfigBuilder::default()
    ///     .set_chain(ChainType::Testnet(zingolib::config::ConfiguredActivationHeights::crosslink_featurenet()))
    ///     .set_indexers(IndexerList::empty().with(IndexerNetwork::CrosslinkFeaturenet, mine.clone()))
    ///     .create();
    /// assert_eq!(config.get_lightwalletd_uri(), mine.uri);
    /// ```
    pub fn set_indexers(&mut self, indexers: IndexerList) -> &mut Self {
        self.indexers = Some(indexers);
        self
    }

    /// Selects which network's endpoints the client uses, instead of the one
    /// [`IndexerNetwork::of_chain`] derives from the chain.
    ///
    /// Needed for a [`IndexerNetwork::Custom`] deployment, which no chain can name: for example
    /// the crosslink nightly network shares the featurenet's parameters but not its chain.
    /// # Examples
    /// ```
    /// use zingolib::config::{ChainType, ConfiguredActivationHeights, IndexerNetwork, ZingoConfigBuilder};
    /// let config = ZingoConfigBuilder::default()
    ///     .set_chain(ChainType::Testnet(ConfiguredActivationHeights::crosslink_featurenet()))
    ///     .set_indexer_network(IndexerNetwork::CROSSLINK_NIGHTLY)
    ///     .create();
    /// assert_eq!(config.indexer_network, IndexerNetwork::CROSSLINK_NIGHTLY);
    /// assert!(config.get_lightwalletd_uri().host().is_none(), "no URI chosen without an indexer list");
    /// ```
    pub fn set_indexer_network(&mut self, network: IndexerNetwork) -> &mut Self {
        self.indexer_network = Some(network);
        self
    }

    /// Adds an endpoint to the list (the built-in list when none was set yet), after the ones
    /// already listed for `network`.
    pub fn add_indexer(&mut self, network: IndexerNetwork, indexer: Indexer) -> &mut Self {
        self.indexers
            .get_or_insert_with(IndexerList::builtin)
            .add(network, indexer);
        self
    }

    /// Adds an endpoint ahead of the ones already listed for `network`, so it is the one the
    /// client connects to when no URI is set explicitly.
    pub fn prefer_indexer(&mut self, network: IndexerNetwork, indexer: Indexer) -> &mut Self {
        self.indexers
            .get_or_insert_with(IndexerList::builtin)
            .prefer(network, indexer);
        self
    }

    /// Set the chain the consuming client will interact with.
    /// See <https://github.com/bitcoin/bips/blob/master/bip-0087.mediawiki#coin-type>
    /// for chain types.
    /// Note "chain type" is not a formal standard.
    /// # Examples
    /// ```
    /// use zingolib::config::{ChainType, ConfiguredActivationHeights, ZingoConfigBuilder};
    /// let testnet = ChainType::Testnet(ConfiguredActivationHeights::crosslink_featurenet());
    /// assert_eq!(ZingoConfigBuilder::default().set_chain(testnet).create().chain, testnet);
    /// ```
    pub fn set_chain(&mut self, chain: ChainType) -> &mut Self {
        self.chain = chain;
        self
    }

    /// Set the wallet directory where client transaction data will be stored in a wallet.
    /// # Examples
    /// ```
    /// use zingolib::config::ZingoConfigBuilder;
    /// use tempfile::TempDir;
    /// let dir = tempfile::TempDir::with_prefix("zingo_doc_test").unwrap().into_path();
    /// let config = ZingoConfigBuilder::default().set_wallet_dir(dir.clone()).create();
    /// assert_eq!(config.wallet_dir.clone().unwrap(), dir);
    /// ```
    pub fn set_wallet_dir(&mut self, dir: PathBuf) -> &mut Self {
        self.wallet_dir = Some(dir);
        self
    }

    /// TODO: Add Doc Comment Here!
    pub fn set_wallet_settings(&mut self, wallet_settings: WalletSettings) -> &mut Self {
        self.wallet_settings = wallet_settings;
        self
    }

    /// TODO: Add Doc Comment Here!
    pub fn set_no_of_accounts(&mut self, no_of_accounts: NonZeroU32) -> &mut Self {
        self.no_of_accounts = no_of_accounts;
        self
    }

    /// TODO: Add Doc Comment Here!
    pub fn create(&self) -> ZingoConfig {
        let indexer_network = self
            .indexer_network
            .clone()
            .unwrap_or_else(|| IndexerNetwork::of_chain(&self.chain));
        let lightwalletd_uri = self
            .lightwalletd_uri
            .clone()
            .or_else(|| {
                self.indexers
                    .as_ref()?
                    .first(&indexer_network)
                    .map(|indexer| indexer.uri.clone())
            })
            .unwrap_or_default();
        let indexers = self.indexers.clone().unwrap_or_else(IndexerList::builtin);
        ZingoConfig {
            lightwalletd_uri: Arc::new(RwLock::new(lightwalletd_uri)),
            indexers: Arc::new(RwLock::new(indexers)),
            indexer_network,
            chain: self.chain,
            wallet_dir: self.wallet_dir.clone(),
            wallet_name: DEFAULT_WALLET_NAME.into(),
            logfile_name: DEFAULT_LOGFILE_NAME.into(),
            wallet_settings: self.wallet_settings.clone(),
            no_of_accounts: self.no_of_accounts,
        }
    }
}

impl Default for ZingoConfigBuilder {
    fn default() -> Self {
        ZingoConfigBuilder {
            lightwalletd_uri: None,
            indexers: None,
            indexer_network: None,
            monitor_mempool: None,
            reorg_buffer_offset: None,
            wallet_dir: None,
            wallet_name: None,
            logfile_name: None,
            chain: ChainType::Mainnet,
            wallet_settings: WalletSettings {
                sync_config: pepper_sync::config::SyncConfig {
                    transparent_address_discovery:
                        pepper_sync::config::TransparentAddressDiscovery::minimal(),
                    performance_level: pepper_sync::config::PerformanceLevel::High,
                },
                min_confirmations: NonZeroU32::try_from(3).unwrap(),
            },
            no_of_accounts: NonZeroU32::try_from(1).expect("hard coded non-zero integer"),
        }
    }
}

impl ZingoConfig {
    /// TODO: Add Doc Comment Here!
    #[must_use]
    pub fn build(chain: ChainType) -> ZingoConfigBuilder {
        ZingoConfigBuilder {
            chain,
            ..ZingoConfigBuilder::default()
        }
    }

    #[cfg(any(test, feature = "testutils"))]
    /// create a `ZingoConfig` that helps a `LightClient` connect to an indexer.
    #[must_use]
    pub fn create_testnet() -> ZingoConfig {
        ZingoConfig::build(ChainType::Testnet(
            ConfiguredActivationHeights::crosslink_featurenet(),
        ))
        .set_lightwalletd_uri((DEFAULT_TESTNET_INDEXER_URI).parse::<http::Uri>().unwrap())
        .create()
    }

    #[cfg(any(test, feature = "testutils"))]
    /// create a `ZingoConfig` that helps a `LightClient` connect to an indexer.
    #[must_use]
    pub fn create_mainnet() -> ZingoConfig {
        ZingoConfig::build(ChainType::Mainnet)
            .set_lightwalletd_uri((DEFAULT_INDEXER_URI).parse::<http::Uri>().unwrap())
            .create()
    }

    #[cfg(feature = "testutils")]
    /// create a `ZingoConfig` that signals a `LightClient` not to connect to an indexer.
    #[must_use]
    pub fn create_unconnected(chain: ChainType, dir: Option<PathBuf>) -> ZingoConfig {
        if let Some(dir) = dir {
            ZingoConfig::build(chain).set_wallet_dir(dir).create()
        } else {
            ZingoConfig::build(chain).create()
        }
    }

    /// Convenience wrapper
    #[must_use]
    pub fn sapling_activation_height(&self) -> u64 {
        self.chain
            .activation_height(NetworkUpgrade::Sapling)
            .unwrap()
            .into()
    }

    /// TODO: Add Doc Comment Here!
    #[must_use]
    pub fn orchard_activation_height(&self) -> u64 {
        self.chain
            .activation_height(NetworkUpgrade::Nu5)
            .unwrap()
            .into()
    }

    /// TODO: Add Doc Comment Here!
    pub fn set_data_dir(&mut self, dir_str: String) {
        self.wallet_dir = Some(PathBuf::from(dir_str));
    }

    /// Build the Logging config
    pub fn get_log_config(&self) -> io::Result<Config> {
        let window_size = 3; // log0, log1, log2
        let fixed_window_roller = FixedWindowRoller::builder()
            .build("zingo-wallet-log{}", window_size)
            .unwrap();
        let size_limit = 5 * 1024 * 1024; // 5MB as max log file size to roll
        let size_trigger = SizeTrigger::new(size_limit);
        let compound_policy =
            CompoundPolicy::new(Box::new(size_trigger), Box::new(fixed_window_roller));

        Config::builder()
            .appender(
                Appender::builder()
                    .filter(Box::new(ThresholdFilter::new(LevelFilter::Info)))
                    .build(
                        "logfile",
                        Box::new(
                            RollingFileAppender::builder()
                                .encoder(Box::new(PatternEncoder::new("{d} {l}::{m}{n}")))
                                .build(self.get_log_path(), Box::new(compound_policy))?,
                        ),
                    ),
            )
            .build(
                Root::builder()
                    .appender("logfile")
                    .build(LevelFilter::Debug),
            )
            .map_err(|e| Error::other(format!("{e}")))
    }

    /// TODO: Add Doc Comment Here!
    #[must_use]
    pub fn get_zingo_wallet_dir(&self) -> Box<Path> {
        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            PathBuf::from(&self.wallet_dir.as_ref().unwrap()).into_boxed_path()
        }

        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            let mut zcash_data_location;
            // If there's some --data-dir path provided, use it
            if self.wallet_dir.is_some() {
                zcash_data_location = PathBuf::from(&self.wallet_dir.as_ref().unwrap());
            } else {
                #[cfg(any(target_os = "macos", target_os = "windows"))]
                {
                    zcash_data_location =
                        dirs::data_dir().expect("Couldn't determine app data directory!");
                    zcash_data_location.push("Zcash");
                }

                #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                {
                    if dirs::home_dir().is_none() {
                        log::info!("Couldn't determine home dir!");
                    }
                    zcash_data_location =
                        dirs::home_dir().expect("Couldn't determine home directory!");
                    zcash_data_location.push(".zcash");
                }

                match &self.chain {
                    ChainType::Testnet(_) => zcash_data_location.push("testnet3"),
                    ChainType::Mainnet => {}
                    ChainType::Regtest(_) => zcash_data_location.push("regtest"),
                }
            }

            // Create directory if it doesn't exist on non-mobile platforms
            match std::fs::create_dir_all(zcash_data_location.clone()) {
                Ok(()) => {}
                Err(e) => {
                    tracing::error!("Couldn't create zcash directory!\n{e}");
                    panic!("Couldn't create zcash directory!");
                }
            }

            zcash_data_location.into_boxed_path()
        }
    }

    /// TODO: Add Doc Comment Here!
    pub fn get_zcash_params_path(&self) -> io::Result<Box<Path>> {
        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            Ok(PathBuf::from(&self.wallet_dir.as_ref().unwrap()).into_boxed_path())
        }

        //TODO:  This fn is not correct for regtest mode
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            if dirs::home_dir().is_none() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Couldn't determine Home Dir",
                ));
            }

            let zcash_params_dir = zcash_proofs::default_params_folder().unwrap();

            Ok(zcash_params_dir.into_boxed_path())
        }
    }

    /// The endpoint every request goes to.
    #[must_use]
    pub fn get_lightwalletd_uri(&self) -> http::Uri {
        self.lightwalletd_uri
            .read()
            .expect("Couldn't read configured indexer URI!")
            .clone()
    }

    /// The network whose endpoints this config uses.
    #[must_use]
    pub fn indexer_network(&self) -> &IndexerNetwork {
        &self.indexer_network
    }

    /// A copy of the endpoint list.
    #[must_use]
    pub fn indexers(&self) -> IndexerList {
        self.indexers
            .read()
            .expect("Couldn't read configured indexer list!")
            .clone()
    }

    /// The endpoints listed for this config's network, most preferred first.
    #[must_use]
    pub fn indexers_for_chain(&self) -> Vec<Indexer> {
        self.indexers()
            .for_network(&self.indexer_network)
            .cloned()
            .collect()
    }

    /// Adds an endpoint for this config's network, after the ones already listed.
    pub fn add_indexer(&self, indexer: Indexer) {
        self.indexers
            .write()
            .expect("Couldn't write configured indexer list!")
            .add(self.indexer_network.clone(), indexer);
    }

    /// Adds an endpoint for this config's network ahead of the ones already listed.
    pub fn prefer_indexer(&self, indexer: Indexer) {
        self.indexers
            .write()
            .expect("Couldn't write configured indexer list!")
            .prefer(self.indexer_network.clone(), indexer);
    }

    /// TODO: Add Doc Comment Here!
    #[must_use]
    pub fn get_wallet_with_name_pathbuf(&self, wallet_name: String) -> PathBuf {
        let mut wallet_location = self.get_zingo_wallet_dir().into_path_buf();
        // if id is empty, the name is the default name
        if wallet_name.is_empty() {
            wallet_location.push(&self.wallet_name);
        } else {
            wallet_location.push(wallet_name);
        }
        wallet_location
    }

    /// TODO: Add Doc Comment Here!
    #[must_use]
    pub fn get_wallet_with_name_path(&self, wallet_name: String) -> Box<Path> {
        self.get_wallet_with_name_pathbuf(wallet_name)
            .into_boxed_path()
    }

    /// TODO: Add Doc Comment Here!
    #[must_use]
    pub fn wallet_with_name_path_exists(&self, wallet_name: String) -> bool {
        self.get_wallet_with_name_path(wallet_name).exists()
    }

    /// TODO: Add Doc Comment Here!
    #[must_use]
    pub fn get_wallet_pathbuf(&self) -> PathBuf {
        let mut wallet_location = self.get_zingo_wallet_dir().into_path_buf();
        wallet_location.push(&self.wallet_name);
        wallet_location
    }

    /// TODO: Add Doc Comment Here!
    #[must_use]
    pub fn get_wallet_path(&self) -> Box<Path> {
        self.get_wallet_pathbuf().into_boxed_path()
    }

    /// TODO: Add Doc Comment Here!
    #[must_use]
    pub fn wallet_path_exists(&self) -> bool {
        self.get_wallet_path().exists()
    }

    /// TODO: Add Doc Comment Here!
    #[deprecated(note = "this method was renamed 'wallet_path_exists' for clarity")]
    #[must_use]
    pub fn wallet_exists(&self) -> bool {
        self.wallet_path_exists()
    }

    /// TODO: Add Doc Comment Here!
    pub fn backup_existing_wallet(&self) -> Result<String, String> {
        if !self.wallet_path_exists() {
            return Err(format!(
                "Couldn't find existing wallet to backup. Looked in {:?}",
                self.get_wallet_path().to_str()
            ));
        }

        let mut backup_file_path = self.get_zingo_wallet_dir().into_path_buf();
        backup_file_path.push(format!(
            "zingo-wallet.backup.{}.dat",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        ));

        let backup_file_str = backup_file_path.to_string_lossy().to_string();
        std::fs::copy(self.get_wallet_path(), backup_file_path).map_err(|e| format!("{e}"))?;

        Ok(backup_file_str)
    }

    /// TODO: Add Doc Comment Here!
    #[must_use]
    pub fn get_log_path(&self) -> Box<Path> {
        let mut log_path = self.get_zingo_wallet_dir().into_path_buf();
        log_path.push(&self.logfile_name);
        //tracing::info!("LogFile:\n{}", log_path.to_str().unwrap());

        log_path.into_boxed_path()
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use crate::wallet::WalletSettings;

    /// Validate that the `load_clientconfig` function creates a valid config from an empty uri
    #[tokio::test]
    async fn test_load_clientconfig() {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok(); // another test in this process may already have installed it
        let _ = tracing_subscriber::fmt().try_init();

        let valid_uri = crate::config::construct_lightwalletd_uri(Some(String::new()));

        let temp_dir = tempfile::TempDir::new().unwrap();

        let temp_path = temp_dir.path().to_path_buf();

        let valid_config = crate::config::load_clientconfig(
            valid_uri.clone(),
            Some(temp_path),
            crate::config::ChainType::Mainnet,
            WalletSettings {
                sync_config: pepper_sync::config::SyncConfig {
                    transparent_address_discovery:
                        pepper_sync::config::TransparentAddressDiscovery::minimal(),
                    performance_level: pepper_sync::config::PerformanceLevel::High,
                },
                min_confirmations: NonZeroU32::try_from(1).unwrap(),
            },
            1.try_into().unwrap(),
            "".to_string(),
        );

        assert!(valid_config.is_ok());
    }

    #[tokio::test]
    async fn test_load_clientconfig_offline() {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok(); // another test in this process may already have installed it
        let _ = tracing_subscriber::fmt().try_init();

        let valid_uri = crate::config::construct_lightwalletd_uri(Some(
            crate::config::DEFAULT_INDEXER_URI.to_string(),
        ));
        // let invalid_uri = construct_lightwalletd_uri(Some("Invalid URI".to_string()));
        let temp_dir = tempfile::TempDir::new().unwrap();

        let temp_path = temp_dir.path().to_path_buf();
        // let temp_path_invalid = temp_dir.path().to_path_buf();

        let valid_config = crate::config::load_clientconfig(
            valid_uri.clone(),
            Some(temp_path),
            crate::config::ChainType::Mainnet,
            WalletSettings {
                sync_config: pepper_sync::config::SyncConfig {
                    transparent_address_discovery:
                        pepper_sync::config::TransparentAddressDiscovery::minimal(),
                    performance_level: pepper_sync::config::PerformanceLevel::High,
                },
                min_confirmations: NonZeroU32::try_from(1).unwrap(),
            },
            1.try_into().unwrap(),
            "".to_string(),
        )
        .unwrap();

        assert_eq!(valid_config.get_lightwalletd_uri(), valid_uri);
        assert_eq!(valid_config.chain, crate::config::ChainType::Mainnet);

        // let invalid_config = load_clientconfig_offline(
        //     invalid_uri.clone(),
        //     Some(temp_path_invalid),
        //     ChainType::Mainnet,
        //     true,
        // );
        // assert_eq!(invalid_config.is_err(), true);
    }
}
