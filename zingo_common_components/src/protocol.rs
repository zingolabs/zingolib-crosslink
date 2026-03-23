//! Module for types associated with the zcash protocol and consensus.

/// Network types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkType {
    /// Mainnet
    Mainnet,
    /// Testnet
    Testnet,
    /// Regtest
    Regtest(ActivationHeights),
}

impl std::fmt::Display for NetworkType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let chain = match self {
            NetworkType::Mainnet => "mainnet",
            NetworkType::Testnet => "testnet",
            NetworkType::Regtest(_) => "regtest",
        };
        write!(f, "{chain}")
    }
}

/// Network upgrade activation heights for custom testnet and regtest network configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivationHeights {
    overwinter: Option<u32>,
    sapling: Option<u32>,
    blossom: Option<u32>,
    heartwood: Option<u32>,
    canopy: Option<u32>,
    nu5: Option<u32>,
    nu6: Option<u32>,
    nu6_1: Option<u32>,
    nu7: Option<u32>,
}

impl Default for ActivationHeights {
    fn default() -> Self {
        Self::builder()
            .set_overwinter(Some(1))
            .set_sapling(Some(1))
            .set_blossom(Some(1))
            .set_heartwood(Some(1))
            .set_canopy(Some(1))
            .set_nu5(Some(1))
            .set_nu6(Some(1))
            .set_nu6_1(Some(1))
            .set_nu7(None)
            .build()
    }
}

impl ActivationHeights {
    /// Constructs new builder.
    pub fn builder() -> ActivationHeightsBuilder {
        ActivationHeightsBuilder::new()
    }

    /// Returns overwinter network upgrade activation height.
    pub fn overwinter(&self) -> Option<u32> {
        self.overwinter
    }

    /// Returns sapling network upgrade activation height.
    pub fn sapling(&self) -> Option<u32> {
        self.sapling
    }

    /// Returns blossom network upgrade activation height.
    pub fn blossom(&self) -> Option<u32> {
        self.blossom
    }

    /// Returns heartwood network upgrade activation height.
    pub fn heartwood(&self) -> Option<u32> {
        self.heartwood
    }

    /// Returns canopy network upgrade activation height.
    pub fn canopy(&self) -> Option<u32> {
        self.canopy
    }

    /// Returns nu5 network upgrade activation height.
    pub fn nu5(&self) -> Option<u32> {
        self.nu5
    }

    /// Returns nu6 network upgrade activation height.
    pub fn nu6(&self) -> Option<u32> {
        self.nu6
    }

    /// Returns nu6.1 network upgrade activation height.
    pub fn nu6_1(&self) -> Option<u32> {
        self.nu6_1
    }

    /// Returns nu7 network upgrade activation height.
    pub fn nu7(&self) -> Option<u32> {
        self.nu7
    }
}

/// Leverages a builder method to avoid new network upgrades from causing breaking changes to the public API.
pub struct ActivationHeightsBuilder {
    overwinter: Option<u32>,
    sapling: Option<u32>,
    blossom: Option<u32>,
    heartwood: Option<u32>,
    canopy: Option<u32>,
    nu5: Option<u32>,
    nu6: Option<u32>,
    nu6_1: Option<u32>,
    nu7: Option<u32>,
}

impl Default for ActivationHeightsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ActivationHeightsBuilder {
    /// Constructs a builder with all fields set to `None`.
    pub fn new() -> Self {
        Self {
            overwinter: None,
            sapling: None,
            blossom: None,
            heartwood: None,
            canopy: None,
            nu5: None,
            nu6: None,
            nu6_1: None,
            nu7: None,
        }
    }

    /// Set `overwinter` field.
    pub fn set_overwinter(mut self, height: Option<u32>) -> Self {
        self.overwinter = height;

        self
    }

    /// Set `sapling` field.
    pub fn set_sapling(mut self, height: Option<u32>) -> Self {
        self.sapling = height;

        self
    }

    /// Set `blossom` field.
    pub fn set_blossom(mut self, height: Option<u32>) -> Self {
        self.blossom = height;

        self
    }

    /// Set `heartwood` field.
    pub fn set_heartwood(mut self, height: Option<u32>) -> Self {
        self.heartwood = height;

        self
    }

    /// Set `canopy` field.
    pub fn set_canopy(mut self, height: Option<u32>) -> Self {
        self.canopy = height;

        self
    }

    /// Set `nu5` field.
    pub fn set_nu5(mut self, height: Option<u32>) -> Self {
        self.nu5 = height;

        self
    }

    /// Set `nu6` field.
    pub fn set_nu6(mut self, height: Option<u32>) -> Self {
        self.nu6 = height;

        self
    }

    /// Set `nu6_1` field.
    pub fn set_nu6_1(mut self, height: Option<u32>) -> Self {
        self.nu6_1 = height;

        self
    }

    /// Set `nu7` field.
    pub fn set_nu7(mut self, height: Option<u32>) -> Self {
        self.nu7 = height;

        self
    }

    /// Builds `ActivationHeights` with assertions to ensure all earlier network upgrades are active with an activation
    /// height equal to or lower than the later network upgrades.
    pub fn build(self) -> ActivationHeights {
        if let Some(b) = self.sapling {
            assert!(self.overwinter.is_some_and(|a| a <= b));
        }
        if let Some(b) = self.blossom {
            assert!(self.sapling.is_some_and(|a| a <= b));
        }
        if let Some(b) = self.heartwood {
            assert!(self.blossom.is_some_and(|a| a <= b));
        }
        if let Some(b) = self.canopy {
            assert!(self.heartwood.is_some_and(|a| a <= b));
        }
        if let Some(b) = self.nu5 {
            assert!(self.canopy.is_some_and(|a| a <= b));
        }
        if let Some(b) = self.nu6 {
            assert!(self.nu5.is_some_and(|a| a <= b));
        }
        if let Some(b) = self.nu6_1 {
            assert!(self.nu6.is_some_and(|a| a <= b));
        }
        if let Some(b) = self.nu7 {
            assert!(self.nu6_1.is_some_and(|a| a <= b));
        }

        ActivationHeights {
            overwinter: self.overwinter,
            sapling: self.sapling,
            blossom: self.blossom,
            heartwood: self.heartwood,
            canopy: self.canopy,
            nu5: self.nu5,
            nu6: self.nu6,
            nu6_1: self.nu6_1,
            nu7: self.nu7,
        }
    }
}
