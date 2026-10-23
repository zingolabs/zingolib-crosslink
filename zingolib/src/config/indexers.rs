//! Lightwalletd endpoints, grouped by the network they serve.
//!
//! A [`IndexerList`] is an ordered set of [`Indexer`]s per [`IndexerNetwork`]; earlier
//! entries are preferred. [`IndexerList::builtin`] is what the library ships; a consumer adds its
//! own endpoints with [`IndexerList::add`] or [`IndexerList::prefer`], or starts from
//! [`IndexerList::empty`] and supplies everything itself.

use std::{borrow::Cow, fmt, str::FromStr};

use super::{ChainType, ConfiguredActivationHeights};

/// The network a lightwalletd endpoint serves.
///
/// The named variants are the networks the library knows. [`Self::Custom`] names any other
/// deployment, such as a second crosslink network that shares the chain parameters of a known
/// one but not its chain; a consumer registers its endpoints under that name and selects the
/// name with [`super::ZingoConfigBuilder::set_indexer_network`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IndexerNetwork {
    /// Zcash mainnet.
    Mainnet,
    /// The public Zcash testnet.
    Testnet,
    /// The Crosslink featurenet ("stable"): testnet parameters on the crosslink activation
    /// schedule, served by the crosslink node's embedded lightwalletd.
    CrosslinkFeaturenet,
    /// A local regtest network.
    Regtest,
    /// A network known only by name.
    Custom(Cow<'static, str>),
}

impl IndexerNetwork {
    /// The crosslink nightly network: the same genesis and parameters as the featurenet on a
    /// separate chain, run from the node's `dev` branch.
    pub const CROSSLINK_NIGHTLY: Self = Self::Custom(Cow::Borrowed("crosslink-nightly"));

    /// A network known only by `name`. Names are compared case-sensitively; use lowercase.
    pub fn custom(name: impl Into<Cow<'static, str>>) -> Self {
        Self::Custom(name.into())
    }

    /// The network `chain`'s indexers belong to.
    ///
    /// A testnet chain on the crosslink featurenet schedule is [`Self::CrosslinkFeaturenet`];
    /// any other testnet schedule is [`Self::Testnet`]. A chain cannot name a [`Self::Custom`]
    /// network; set one explicitly on the config.
    #[must_use]
    pub fn of_chain(chain: &ChainType) -> Self {
        match chain {
            ChainType::Mainnet => Self::Mainnet,
            ChainType::Testnet(heights)
                if *heights == ConfiguredActivationHeights::crosslink_featurenet() =>
            {
                Self::CrosslinkFeaturenet
            }
            ChainType::Testnet(_) => Self::Testnet,
            ChainType::Regtest(_) => Self::Regtest,
        }
    }

    /// The name [`FromStr`] accepts.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Mainnet => "mainnet",
            Self::Testnet => "testnet",
            Self::CrosslinkFeaturenet => "featurenet",
            Self::Regtest => "regtest",
            Self::Custom(name) => name,
        }
    }

    /// The named networks, in display order.
    #[must_use]
    pub fn all() -> [Self; 4] {
        [
            Self::Mainnet,
            Self::Testnet,
            Self::CrosslinkFeaturenet,
            Self::Regtest,
        ]
    }
}

impl fmt::Display for IndexerNetwork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A network name that is empty.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "empty indexer network name; expected mainnet, testnet, featurenet, regtest or a custom name"
)]
pub struct EmptyIndexerNetwork;

impl FromStr for IndexerNetwork {
    type Err = EmptyIndexerNetwork;

    /// Known names (and the aliases `main`, `test`, `crosslink`, `stable`, `nightly`) give the
    /// named variants; any other non-empty name gives [`Self::Custom`], lowercased.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" => Err(EmptyIndexerNetwork),
            "mainnet" | "main" => Ok(Self::Mainnet),
            "testnet" | "test" => Ok(Self::Testnet),
            "featurenet" | "crosslink" | "crosslink-featurenet" | "stable" => {
                Ok(Self::CrosslinkFeaturenet)
            }
            "regtest" => Ok(Self::Regtest),
            "nightly" | "crosslink-nightly" => Ok(Self::CROSSLINK_NIGHTLY),
            other => Ok(Self::custom(other.to_string())),
        }
    }
}

/// A lightwalletd endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Indexer {
    /// Where to connect: scheme, host and port.
    pub uri: http::Uri,
    /// A short human-readable name, shown next to the URI in listings.
    pub label: String,
}

impl Indexer {
    /// An endpoint with a label.
    pub fn new(uri: http::Uri, label: impl Into<String>) -> Self {
        Self {
            uri,
            label: label.into(),
        }
    }

    /// Parses `s` as an endpoint, labelled with its host.
    ///
    /// A bare `host:port` gets an `http://` scheme. The scheme must be `http` or `https` and the
    /// host and port must be present, which is what the gRPC client requires.
    pub fn parse(s: &str) -> Result<Self, InvalidIndexer> {
        let s = s.trim();
        let with_scheme = if s.contains("://") {
            s.to_string()
        } else {
            format!("http://{s}")
        };
        let uri: http::Uri = with_scheme
            .parse()
            .map_err(|e: http::uri::InvalidUri| InvalidIndexer(s.to_string(), e.to_string()))?;
        match uri.scheme_str() {
            Some("http" | "https") => {}
            _ => {
                return Err(InvalidIndexer(
                    s.to_string(),
                    "scheme must be http or https".to_string(),
                ));
            }
        }
        let Some(host) = uri.host() else {
            return Err(InvalidIndexer(s.to_string(), "missing host".to_string()));
        };
        if uri.port().is_none() {
            return Err(InvalidIndexer(s.to_string(), "missing port".to_string()));
        }
        let label = host.to_string();
        Ok(Self::new(uri, label))
    }
}

impl fmt::Display for Indexer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.uri, self.label)
    }
}

/// A string that is not a usable endpoint.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid indexer {0:?}: {1}")]
pub struct InvalidIndexer(pub String, pub String);

/// An ordered list of endpoints per network. Earlier entries are preferred.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IndexerList {
    entries: Vec<(IndexerNetwork, Indexer)>,
}

impl IndexerList {
    /// No endpoints at all.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// The endpoints the library ships.
    ///
    /// The crosslink featurenet list opens with the node that served the live chain when this was
    /// written, followed by the hosts the node's own default configuration names (which were on
    /// a stale fork at that time). Probe before trusting any of them; see
    /// [`crate::lightclient::LightClient::select_best_indexer`].
    #[must_use]
    pub fn builtin() -> Self {
        let mut list = Self::empty();
        let add = |list: &mut Self, network, uri: &str, label: &str| {
            list.add(
                network,
                Indexer::new(uri.parse().expect("built-in URI is valid"), label),
            );
        };
        add(
            &mut list,
            IndexerNetwork::Mainnet,
            super::DEFAULT_INDEXER_URI,
            "zec.rocks",
        );
        add(
            &mut list,
            IndexerNetwork::Testnet,
            "https://testnet.zec.rocks:443",
            "zec.rocks testnet",
        );
        add(
            &mut list,
            IndexerNetwork::CrosslinkFeaturenet,
            "http://88.198.67.189:18234",
            "crosslink featurenet (live chain, 2026-10)",
        );
        for host in ["45.63.75.9", "70.34.220.94", "64.177.44.176"] {
            add(
                &mut list,
                IndexerNetwork::CrosslinkFeaturenet,
                &format!("http://{host}:18234"),
                "Shielded Labs (node default peer)",
            );
        }
        for host in ["45.32.186.234", "64.177.122.73", "70.34.201.146"] {
            add(
                &mut list,
                IndexerNetwork::CROSSLINK_NIGHTLY,
                &format!("http://{host}:18234"),
                "crosslink nightly (node dev default peer)",
            );
        }
        add(
            &mut list,
            IndexerNetwork::Regtest,
            "http://127.0.0.1:18234",
            "local crosslink node (P2P port + 10001)",
        );
        list
    }

    /// Appends `indexer` to `network`'s endpoints unless its URI is already listed there.
    pub fn add(&mut self, network: IndexerNetwork, indexer: Indexer) -> &mut Self {
        if !self.contains(&network, &indexer.uri) {
            self.entries.push((network, indexer));
        }
        self
    }

    /// Puts `indexer` first among `network`'s endpoints, replacing any entry with the same URI.
    pub fn prefer(&mut self, network: IndexerNetwork, indexer: Indexer) -> &mut Self {
        self.remove(&network, &indexer.uri);
        let index = self
            .entries
            .iter()
            .position(|(n, _)| *n == network)
            .unwrap_or(self.entries.len());
        self.entries.insert(index, (network, indexer));
        self
    }

    /// Puts `indexers` first among `network`'s endpoints, in the order given.
    pub fn prefer_all(
        &mut self,
        network: IndexerNetwork,
        indexers: impl IntoIterator<Item = Indexer>,
    ) -> &mut Self {
        let indexers: Vec<_> = indexers.into_iter().collect();
        for indexer in indexers.into_iter().rev() {
            self.prefer(network.clone(), indexer);
        }
        self
    }

    /// Puts discovered endpoints first among `network`'s, best first, replacing any entry
    /// with the same URI. The ranking comes from
    /// [`discovery`](crate::config::discovery); the light client fails over down the list.
    pub fn prefer_discovered(
        &mut self,
        network: IndexerNetwork,
        uris: impl IntoIterator<Item = String>,
    ) -> &mut Self {
        let indexers = uris.into_iter().filter_map(|uri| {
            uri.parse::<http::Uri>()
                .ok()
                .map(|uri| Indexer::new(uri, "discovered"))
        });
        self.prefer_all(network, indexers)
    }

    /// [`Self::add`] in builder style.
    #[must_use]
    pub fn with(mut self, network: IndexerNetwork, indexer: Indexer) -> Self {
        self.add(network, indexer);
        self
    }

    /// Removes the endpoint with `uri` from `network`, returning whether one was listed.
    pub fn remove(&mut self, network: &IndexerNetwork, uri: &http::Uri) -> bool {
        let before = self.entries.len();
        self.entries
            .retain(|(n, s)| !(n == network && s.uri == *uri));
        self.entries.len() != before
    }

    /// Appends every endpoint of `other` that is not already listed.
    pub fn extend(&mut self, other: IndexerList) -> &mut Self {
        for (network, indexer) in other.entries {
            self.add(network, indexer);
        }
        self
    }

    /// Whether `network` lists an endpoint with `uri`.
    #[must_use]
    pub fn contains(&self, network: &IndexerNetwork, uri: &http::Uri) -> bool {
        self.entries
            .iter()
            .any(|(n, s)| n == network && s.uri == *uri)
    }

    /// `network`'s endpoints, most preferred first.
    pub fn for_network<'a>(
        &'a self,
        network: &IndexerNetwork,
    ) -> impl Iterator<Item = &'a Indexer> + 'a {
        let network = network.clone();
        self.entries
            .iter()
            .filter(move |(n, _)| *n == network)
            .map(|(_, s)| s)
    }

    /// The endpoints for `chain`'s network, most preferred first.
    pub fn for_chain(&self, chain: &ChainType) -> Vec<&Indexer> {
        self.for_network(&IndexerNetwork::of_chain(chain)).collect()
    }

    /// `network`'s most preferred endpoint.
    #[must_use]
    pub fn first(&self, network: &IndexerNetwork) -> Option<&Indexer> {
        self.for_network(network).next()
    }

    /// Every endpoint with its network, most preferred first within a network.
    pub fn iter(&self) -> impl Iterator<Item = (IndexerNetwork, &Indexer)> + '_ {
        self.entries.iter().map(|(n, s)| (n.clone(), s))
    }

    /// The number of endpoints across all networks.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no endpoint is listed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn indexer(uri: &str) -> Indexer {
        Indexer::parse(uri).unwrap()
    }

    #[test]
    fn chain_maps_to_network() {
        assert_eq!(
            IndexerNetwork::of_chain(&ChainType::Mainnet),
            IndexerNetwork::Mainnet
        );
        assert_eq!(
            IndexerNetwork::of_chain(&ChainType::Testnet(
                ConfiguredActivationHeights::crosslink_featurenet()
            )),
            IndexerNetwork::CrosslinkFeaturenet
        );
        assert_eq!(
            IndexerNetwork::of_chain(&ChainType::Testnet(ConfiguredActivationHeights::default())),
            IndexerNetwork::Testnet
        );
        assert_eq!(
            IndexerNetwork::of_chain(&ChainType::Regtest(
                ConfiguredActivationHeights::regtest_all_active()
            )),
            IndexerNetwork::Regtest
        );
    }

    #[test]
    fn network_names_round_trip() {
        for network in IndexerNetwork::all() {
            assert_eq!(network.name().parse::<IndexerNetwork>().unwrap(), network);
        }
        assert_eq!(
            "crosslink".parse::<IndexerNetwork>().unwrap(),
            IndexerNetwork::CrosslinkFeaturenet
        );
        assert_eq!(
            "stable".parse::<IndexerNetwork>().unwrap(),
            IndexerNetwork::CrosslinkFeaturenet
        );
        assert_eq!(
            "nightly".parse::<IndexerNetwork>().unwrap(),
            IndexerNetwork::CROSSLINK_NIGHTLY
        );
        assert_eq!(
            "Moon".parse::<IndexerNetwork>().unwrap(),
            IndexerNetwork::custom("moon")
        );
        assert_eq!(IndexerNetwork::custom("moon").name(), "moon");
        assert!(" ".parse::<IndexerNetwork>().is_err());
    }

    #[test]
    fn parse_requires_scheme_host_and_port() {
        let bare = indexer("10.0.0.1:18234");
        assert_eq!(bare.uri, "http://10.0.0.1:18234");
        assert_eq!(bare.label, "10.0.0.1");
        assert_eq!(indexer("https://zec.rocks:443").label, "zec.rocks");
        assert!(Indexer::parse("https://zec.rocks").is_err());
        assert!(Indexer::parse("ftp://zec.rocks:443").is_err());
    }

    #[test]
    fn builtin_lists_every_network() {
        let list = IndexerList::builtin();
        for network in IndexerNetwork::all() {
            assert!(list.first(&network).is_some(), "{network}");
        }
        assert!(list.first(&IndexerNetwork::CROSSLINK_NIGHTLY).is_some());
    }

    #[test]
    fn consumer_endpoints_are_preferred_and_deduplicated() {
        let mut list = IndexerList::builtin();
        let builtin_count = list.len();
        let mine = indexer("10.0.0.1:18234");

        list.add(IndexerNetwork::CrosslinkFeaturenet, mine.clone());
        assert_eq!(list.len(), builtin_count + 1);
        list.add(IndexerNetwork::CrosslinkFeaturenet, mine.clone());
        assert_eq!(list.len(), builtin_count + 1);
        assert_ne!(
            list.first(&IndexerNetwork::CrosslinkFeaturenet),
            Some(&mine)
        );

        list.prefer(IndexerNetwork::CrosslinkFeaturenet, mine.clone());
        assert_eq!(list.len(), builtin_count + 1);
        assert_eq!(
            list.first(&IndexerNetwork::CrosslinkFeaturenet),
            Some(&mine)
        );
        assert_eq!(
            list.first(&IndexerNetwork::Mainnet).unwrap().label,
            "zec.rocks"
        );

        assert!(list.remove(&IndexerNetwork::CrosslinkFeaturenet, &mine.uri));
        assert!(!list.remove(&IndexerNetwork::CrosslinkFeaturenet, &mine.uri));
        assert_eq!(list.len(), builtin_count);
    }

    #[test]
    fn custom_networks_keep_their_own_lists() {
        let nightly = IndexerNetwork::custom("nightly-2");
        let list = IndexerList::empty()
            .with(nightly.clone(), indexer("10.0.0.2:18234"))
            .with(
                IndexerNetwork::CrosslinkFeaturenet,
                indexer("10.0.0.1:18234"),
            );
        assert_eq!(list.for_network(&nightly).count(), 1);
        assert_eq!(list.first(&nightly).unwrap().uri, "http://10.0.0.2:18234");
        assert_eq!(
            list.for_network(&IndexerNetwork::CrosslinkFeaturenet)
                .count(),
            1
        );
    }

    #[test]
    fn prefer_all_keeps_the_given_order() {
        let mut list = IndexerList::empty();
        list.add(IndexerNetwork::Regtest, indexer("127.0.0.1:1"));
        list.prefer_all(
            IndexerNetwork::Regtest,
            [indexer("127.0.0.1:2"), indexer("127.0.0.1:3")],
        );
        let ports: Vec<_> = list
            .for_network(&IndexerNetwork::Regtest)
            .map(|s| s.uri.port_u16().unwrap())
            .collect();
        assert_eq!(ports, [2, 3, 1]);
    }

    #[test]
    fn a_fully_custom_list_needs_no_builtins() {
        let list =
            IndexerList::empty().with(IndexerNetwork::Mainnet, indexer("https://my.node:443"));
        assert_eq!(list.len(), 1);
        assert!(list.first(&IndexerNetwork::Testnet).is_none());
    }
}
