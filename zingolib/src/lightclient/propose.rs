//! `LightClient` function `do_propose` generates a proposal to send to specified addresses.

use zcash_address::ZcashAddress;
use zcash_client_backend::zip321::TransactionRequest;
use zcash_primitives::bft::FinalizerAddress;
use zcash_protocol::{PoolType, ShieldedPool, value::Zatoshis};

use crate::config::ZENNIES_FOR_ZINGO_AMOUNT;
use crate::config::get_donation_address_for_chain;
use crate::data::proposal::ExtraFeeProposal;
use crate::data::proposal::ProportionalFeeProposal;
use crate::data::proposal::ProportionalFeeShieldProposal;
use crate::data::proposal::ZingoProposal;
use crate::data::receivers::Receiver;
use crate::data::receivers::transaction_request_from_receivers;
use crate::lightclient::LightClient;
use crate::wallet::error::ProposeSendError;
use crate::wallet::error::ProposeShieldError;
use crate::wallet::error::WithdrawError;
use crate::wallet::propose::StakingProposal;

impl LightClient {
    fn append_zingo_zenny_receiver(&self, receivers: &mut Vec<Receiver>) {
        let zfz_address = get_donation_address_for_chain(&self.config().chain);
        let dev_donation_receiver = Receiver::new(
            crate::utils::conversion::address_from_str(zfz_address).expect("Hard coded str"),
            Zatoshis::from_u64(ZENNIES_FOR_ZINGO_AMOUNT).expect("Hard coded u64."),
            None,
        );
        receivers.push(dev_donation_receiver);
    }

    /// Creates and stores a proposal from a transaction request.
    pub async fn propose_send(
        &mut self,
        request: TransactionRequest,
        account_id: zip32::AccountId,
    ) -> Result<ProportionalFeeProposal, ProposeSendError> {
        let _ignore_error = self.pause_sync();
        let mut wallet = self.wallet.write().await;
        let proposal = wallet.create_send_proposal(request, account_id)?;
        wallet.store_proposal(ZingoProposal::Send {
            proposal: proposal.clone(),
            sending_account: account_id,
        });

        Ok(proposal)
    }

    /// Creates and stores a proposal that moves `amount` between two of this wallet's own pools.
    ///
    /// See `LightWallet::create_pool_transfer_proposal`. Confirm with
    /// [`LightClient::send_stored_proposal`].
    pub async fn propose_pool_transfer(
        &mut self,
        from: ShieldedPool,
        to: PoolType,
        amount: Zatoshis,
        account_id: zip32::AccountId,
    ) -> Result<ProportionalFeeProposal, ProposeSendError> {
        let _ignore_error = self.pause_sync();
        let mut wallet = self.wallet.write().await;
        let proposal = wallet.create_pool_transfer_proposal(from, to, amount, account_id)?;
        wallet.store_proposal(ZingoProposal::Send {
            proposal: proposal.clone(),
            sending_account: account_id,
        });

        Ok(proposal)
    }

    /// Stores a staking proposal so that [`LightClient::send_stored_proposal`] builds and sends it.
    async fn store_staking_proposal(
        &mut self,
        proposal: &StakingProposal<ExtraFeeProposal>,
        account_id: zip32::AccountId,
    ) {
        let mut wallet = self.wallet.write().await;
        wallet.store_proposal(ZingoProposal::Crosslink {
            proposal: proposal.proportional_fee_proposal().clone(),
            staking_action: *proposal.staking_action(),
            sending_account: account_id,
        });
    }

    /// Creates and stores a proposal that stakes `amount` to `target_finalizer`.
    ///
    /// `target_finalizer` is the finalizer's self-certifying address; parse one from its `zfinv1`
    /// string with [`crate::crosslink::parse_finalizer_address`]. From `FeaturenetUpgrade1` the
    /// amount must be a power of 10 zats.
    pub async fn propose_stake(
        &mut self,
        amount: Zatoshis,
        target_finalizer: FinalizerAddress,
        account_id: zip32::AccountId,
    ) -> Result<StakingProposal<ExtraFeeProposal>, ProposeSendError> {
        let _ignore_error = self.pause_sync();
        let proposal = self
            .wallet
            .write()
            .await
            .create_stake_proposal(amount, target_finalizer, account_id)
            .await?;
        self.store_staking_proposal(&proposal, account_id).await;

        Ok(proposal)
    }

    /// The node's view of a bond, or `None` when the node cannot be asked; the proposal is then
    /// built from the wallet's own history and the node has the final say.
    async fn node_bond_info(&self, bond_key: [u8; 32]) -> Option<crate::crosslink::BondInfo> {
        match self.get_bond_info(bond_key).await {
            Ok(info) => Some(info),
            Err(err) => {
                tracing::warn!("building without the node's bond info: {err}");
                None
            }
        }
    }

    /// Creates and stores a proposal that begins unbonding the bond `bond_key`.
    pub async fn propose_begin_unstake(
        &mut self,
        bond_key: [u8; 32],
        account_id: zip32::AccountId,
    ) -> Result<StakingProposal<ExtraFeeProposal>, ProposeSendError> {
        let _ignore_error = self.pause_sync();
        let bond_info = self.node_bond_info(bond_key).await;
        let proposal = self
            .wallet
            .write()
            .await
            .create_unbonding_start_proposal(bond_key, bond_info.as_ref(), account_id)
            .await?;
        self.store_staking_proposal(&proposal, account_id).await;

        Ok(proposal)
    }

    /// Creates and stores a proposal that retargets the bond `bond_key` to `to_finalizer`.
    pub async fn propose_retarget_bond(
        &mut self,
        bond_key: [u8; 32],
        to_finalizer: FinalizerAddress,
        account_id: zip32::AccountId,
    ) -> Result<StakingProposal<ExtraFeeProposal>, ProposeSendError> {
        let _ignore_error = self.pause_sync();
        let bond_info = self.node_bond_info(bond_key).await;
        let proposal = self
            .wallet
            .write()
            .await
            .create_retarget_proposal(bond_key, to_finalizer, bond_info.as_ref(), account_id)
            .await?;
        self.store_staking_proposal(&proposal, account_id).await;

        Ok(proposal)
    }

    /// Withdraws the unbonded bond `bond_key` into this wallet's Ironwood pool.
    ///
    /// Unlike the other staking operations this builds and broadcasts immediately, because the
    /// bond's value enters the transaction from the staking pool rather than from a note; see
    /// [`crate::wallet::LightWallet::withdraw_bond`].
    pub async fn withdraw_bond(
        &mut self,
        bond_key: [u8; 32],
        account_id: zip32::AccountId,
    ) -> Result<zcash_protocol::TxId, WithdrawError> {
        let _ignore_error = self.pause_sync();
        let mut client = zingo_netutils::get_client(self.indexer_uri())
            .await
            .map_err(|e| WithdrawError::RecordFailed(e.to_string()))?;
        let txid = self
            .wallet
            .write()
            .await
            .withdraw_bond(&mut client, bond_key, account_id)
            .await?;
        let _ignore_error = self.resume_sync();
        Ok(txid)
    }

    /// Creates and stores a proposal for sending all shielded funds from a specified account to a given `address`.
    pub async fn propose_send_all(
        &mut self,
        address: ZcashAddress,
        zennies_for_zingo: bool,
        memo: Option<zcash_protocol::memo::MemoBytes>,
        account_id: zip32::AccountId,
    ) -> Result<ProportionalFeeProposal, ProposeSendError> {
        let max_send_value = self
            .max_send_value(address.clone(), zennies_for_zingo, account_id)
            .await?;
        if max_send_value == Zatoshis::ZERO {
            return Err(ProposeSendError::ZeroValueSendAll);
        }
        let mut receivers = vec![Receiver::new(address, max_send_value, memo)];
        if zennies_for_zingo {
            self.append_zingo_zenny_receiver(&mut receivers);
        }
        let request = transaction_request_from_receivers(receivers)
            .map_err(ProposeSendError::TransactionRequestFailed)?;
        let _ignore_error = self.pause_sync();
        let mut wallet = self.wallet.write().await;
        let proposal = wallet.create_send_proposal(request, account_id)?;
        wallet.store_proposal(ZingoProposal::Send {
            proposal: proposal.clone(),
            sending_account: account_id,
        });

        Ok(proposal)
    }

    /// Creates and stores a proposal for shielding all transparent funds..
    pub async fn propose_shield(
        &mut self,
        account_id: zip32::AccountId,
    ) -> Result<ProportionalFeeShieldProposal, ProposeShieldError> {
        let mut wallet = self.wallet.write().await;
        let proposal = wallet.create_shield_proposal(account_id).await?;
        wallet.store_proposal(ZingoProposal::Shield {
            proposal: proposal.clone(),
            shielding_account: account_id,
        });

        Ok(proposal)
    }

    /// Returns the maximum value that can be sent from the given `account_id`.
    ///
    /// This value is calculated from the shielded spendable balance minus any fees required to send those funds to
    /// the given `address`. If the wallet is still syncing, the spendable balance may be less than the confirmed
    /// balance - minus the fee - due to notes being above the minimum confirmation threshold or not being able to
    /// construct a witness from the current state of the wallet's note commitment tree.
    /// If `zennies_for_zingo` is set true, an additional payment of `1_000_000` ZAT to the `ZingoLabs` developer address
    /// will be taken into account.
    ///
    /// # Error
    ///
    /// Will return an error if this method fails to calculate the total wallet balance or create the
    /// proposal needed to calculate the fee
    pub async fn max_send_value(
        &self,
        address: ZcashAddress,
        zennies_for_zingo: bool,
        account_id: zip32::AccountId,
    ) -> Result<Zatoshis, ProposeSendError> {
        let mut wallet = self.wallet.write().await;
        let confirmed_balance = wallet.shielded_spendable_balance(account_id, false)?;
        let mut spendable_balance = confirmed_balance;

        loop {
            let mut receivers = vec![Receiver::new(address.clone(), spendable_balance, None)];
            if zennies_for_zingo {
                self.append_zingo_zenny_receiver(&mut receivers);
            }
            let request = transaction_request_from_receivers(receivers)?;
            let trial_proposal = wallet.create_send_proposal(request, account_id);

            match trial_proposal {
                Err(ProposeSendError::Proposal(
                    zcash_client_backend::data_api::error::Error::InsufficientFunds {
                        available,
                        required,
                    },
                )) => {
                    if let Some(shortfall) = required - confirmed_balance {
                        match spendable_balance - shortfall {
                            Some(updated_spendable) => {
                                spendable_balance = updated_spendable;
                            }
                            None => {
                                return Err(ProposeSendError::Proposal(
                                zcash_client_backend::data_api::error::Error::InsufficientFunds {
                                    available: confirmed_balance,
                                    required,
                                },
                            ));
                            }
                        }
                    } else {
                        // bugged underflow case, required should always be larger than confirmed shielded balance to cause
                        // insufficient funds error.
                        // returns insufficient funds error with same values from original error for debugging
                        return Err(ProposeSendError::Proposal(
                            zcash_client_backend::data_api::error::Error::InsufficientFunds {
                                available,
                                required,
                            },
                        ));
                    }
                }
                Err(e) => {
                    return Err(e);
                }
                Ok(_) => {
                    break;
                }
            }
        }

        Ok(spendable_balance)
    }
}

#[cfg(test)]
mod shielding {
    use std::num::NonZeroU32;

    use bip0039::Mnemonic;
    use pepper_sync::config::SyncConfig;
    use zcash_protocol::consensus::Parameters;
    use zingo_test_vectors::seeds;

    use crate::{
        config::ZingoConfigBuilder,
        lightclient::LightClient,
        wallet::{
            LightWallet, SeedDerivation, WalletBase, WalletSettings, error::ProposeShieldError,
        },
    };

    fn create_basic_client() -> LightClient {
        let config = ZingoConfigBuilder::default().create();
        LightClient::create_from_wallet(
            LightWallet::new(
                config.chain,
                WalletBase::Mnemonic {
                    mnemonic: Mnemonic::from_phrase(seeds::HOSPITAL_MUSEUM_SEED.to_string())
                        .unwrap(),
                    no_of_accounts: 1.try_into().unwrap(),
                    seed_derivation: SeedDerivation::Zip32Standard,
                },
                419200.into(),
                WalletSettings {
                    sync_config: SyncConfig {
                        transparent_address_discovery:
                            pepper_sync::config::TransparentAddressDiscovery::minimal(),
                        performance_level: pepper_sync::config::PerformanceLevel::High,
                    },
                    min_confirmations: NonZeroU32::try_from(1).unwrap(),
                },
            )
            .unwrap(),
            config,
            true,
        )
        .unwrap()
    }
    #[tokio::test]
    async fn propose_shield_missing_scan_prerequisite() {
        let basic_client = create_basic_client();
        let propose_shield_result = basic_client
            .wallet
            .write()
            .await
            .create_shield_proposal(zip32::AccountId::ZERO)
            .await;
        match propose_shield_result {
            Err(ProposeShieldError::Component(
                zcash_client_backend::data_api::error::Error::ScanRequired,
            )) => true,
            _ => panic!("Unexpected error state!"),
        };
    }
    #[tokio::test]
    async fn get_transparent_addresses() {
        let basic_client = create_basic_client();
        let network = basic_client.wallet.read().await.network;

        // TODO: store t addrs as concrete types instead of encoded
        let transparent_addresses = basic_client
            .wallet
            .read()
            .await
            .transparent_addresses()
            .values()
            .map(|address| {
                Ok(zcash_address::ZcashAddress::try_from_encoded(address)?
                    .convert_if_network::<zcash_transparent::address::TransparentAddress>(
                        network.network_type(),
                    )
                    .expect("incorrect network should be checked on wallet load"))
            })
            .collect::<Result<Vec<_>, zcash_address::ParseError>>()
            .unwrap();

        assert_eq!(
            transparent_addresses,
            [
                zcash_transparent::address::TransparentAddress::PublicKeyHash([
                    161, 138, 222, 242, 254, 121, 71, 105, 93, 131, 177, 31, 59, 185, 120, 148,
                    255, 189, 198, 33
                ])
            ]
        );
    }
}
