//! The types of transaction Proposal that Zingo! uses.

use std::convert::Infallible;

use zcash_client_backend::{fees::zip317::Zip317FeeRule, proposal::Proposal};
use zcash_primitives::transaction::{
    StakingAction,
    fees::{FeeRule, transparent::InputSize, zip317},
};
use zcash_protocol::{
    consensus::{self, BlockHeight},
    value::{BalanceError, Zatoshis},
};

use crate::wallet::output::OutputRef;

#[derive(Clone, Debug)]
pub struct ExtraFee<R> {
    pub base: R,
    pub extra: Zatoshis,
}

impl<R: FeeRule> FeeRule for ExtraFee<R> {
    type Error = R::Error;

    fn fee_required<P: consensus::Parameters>(
        &self,
        params: &P,
        target_height: BlockHeight,
        transparent_input_sizes: impl IntoIterator<Item = InputSize>,
        transparent_output_sizes: impl IntoIterator<Item = usize>,
        sapling_input_count: usize,
        sapling_output_count: usize,
        orchard_action_count: usize,
    ) -> Result<Zatoshis, Self::Error> {
        let base_fee = self.base.fee_required(
            params,
            target_height,
            transparent_input_sizes,
            transparent_output_sizes,
            sapling_input_count,
            sapling_output_count,
            orchard_action_count,
        )?;

        // Zatoshis + Zatoshis returns Option<Zatoshis> (checked arithmetic).
        // See Zatoshis::into_u64/from_u64 + Add impl details. :contentReference[oaicite:2]{index=2}
        Ok((base_fee + self.extra).expect("fee overflow (base_fee + extra)"))
    }
}

impl<R: Zip317FeeRule> Zip317FeeRule for ExtraFee<R> {
    fn marginal_fee(&self) -> Zatoshis {
        self.base.marginal_fee()
    }
    fn grace_actions(&self) -> usize {
        self.base.grace_actions()
    }
}

/// A proposed send to addresses.
/// Identifies the notes to spend by txid, pool, and `output_index`.
/// This type alias, specifies the ZIP317 "Proportional Transfer Fee Mechanism" structure
/// <https://zips.z.cash/zip-0317>
/// as the fee structure for a transaction series. This innovation was created in response
/// "Binance Constraint" that t-addresses that only receive from t-addresses be supported.
/// <https://zips.z.cash/zip-0320>
pub(crate) type ProportionalFeeProposal = Proposal<zip317::FeeRule, OutputRef>;

pub(crate) type ExtraFeeProposal = Proposal<ExtraFee<zip317::FeeRule>, OutputRef>;

/// A proposed shielding.
/// The `zcash_client_backend` Proposal type exposes a "`NoteRef`" generic
/// parameter to track Shielded inputs to the proposal these are
/// disallowed in Zingo `ShieldedProposals`
pub(crate) type ProportionalFeeShieldProposal = Proposal<zip317::FeeRule, Infallible>;

/// The `LightClient` holds one proposal at a time while the user decides whether to accept the fee.
#[derive(Debug, Clone)]
pub(crate) enum ZingoProposal {
    /// Send proposal.
    Send {
        proposal: ProportionalFeeProposal,
        sending_account: zip32::AccountId,
    },
    /// Shield proposal.
    Shield {
        proposal: ProportionalFeeShieldProposal,
        shielding_account: zip32::AccountId,
    },

    Crosslink {
        proposal: ExtraFeeProposal,
        staking_action: StakingAction,
        sending_account: zip32::AccountId,
    },
}

/// total sum of all transaction request payment amounts in a proposal
pub fn total_payment_amount(proposal: &ProportionalFeeProposal) -> Result<Zatoshis, BalanceError> {
    proposal
        .steps()
        .iter()
        .map(zcash_client_backend::proposal::Step::transaction_request)
        .try_fold(Zatoshis::ZERO, |acc, request| {
            (acc + request.total()?).ok_or(BalanceError::Overflow)
        })
}

/// total sum of all fees in a proposal
pub fn total_fee<R>(proposal: &Proposal<R, OutputRef>) -> Result<Zatoshis, BalanceError> {
    proposal
        .steps()
        .iter()
        .map(|step| step.balance().fee_required())
        .try_fold(Zatoshis::ZERO, |acc, fee| {
            (acc + fee).ok_or(BalanceError::Overflow)
        })
}

#[cfg(test)]
mod tests {
    use zcash_protocol::value::Zatoshis;

    use crate::mocks;

    #[test]
    fn total_payment_amount() {
        let proposal = mocks::proposal::ProposalBuilder::default().build();
        assert_eq!(
            super::total_payment_amount(&proposal).unwrap(),
            Zatoshis::from_u64(100_000).unwrap()
        );
    }
    #[test]
    fn total_fee() {
        let proposal = mocks::proposal::ProposalBuilder::default().build();
        assert_eq!(
            super::total_fee(&proposal).unwrap(),
            Zatoshis::from_u64(20_000).unwrap()
        );
    }
}
