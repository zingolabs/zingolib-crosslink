//! Gives each output of a transaction its role.
//!
//! zingolib reports an output the wallet sent to itself twice: once as outgoing, decrypted with
//! its outgoing viewing key, and once as received. Both share a position within their pool, the
//! identity zingolib itself uses when it totals what was sent. The outgoing copy is dropped here
//! so every output appears once.
//!
//! The rules for what was sent on purpose and what is change mirror the ones zingolib uses to
//! build value transfers, so the two views agree.

use super::state::{OutputRole, OutputRow, TxKind};

pub fn assign_roles(
    kind: TxKind,
    mut received: Vec<OutputRow>,
    outgoing: Vec<OutputRow>,
) -> Vec<OutputRow> {
    let own = |o: &OutputRow| {
        received
            .iter()
            .any(|r| r.pool == o.pool && r.index == o.index)
    };
    let mut sent: Vec<OutputRow> = outgoing.into_iter().filter(|o| !own(o)).collect();
    sent.iter_mut().for_each(|o| o.role = OutputRole::Sent);

    let external = |o: &OutputRow| {
        o.scope
            .as_deref()
            .is_none_or(|s| s.eq_ignore_ascii_case("external"))
    };
    let any_external = received.iter().any(external);
    for o in &mut received {
        o.role = match kind {
            TxKind::Received => OutputRole::Received,
            // shielded funds and withdrawn bonds come back to the wallet on purpose
            TxKind::Shield | TxKind::WithdrawBond => OutputRole::ToSelf,
            TxKind::SentToSelf if external(o) || !any_external => OutputRole::ToSelf,
            // a bond's value leaves the wallet's pools, like a payment; what comes back to a
            // receiving address was meant, the rest is change
            TxKind::Sent | TxKind::CreateBond | TxKind::BeginUnbond | TxKind::RetargetBond
                if external(o) =>
            {
                OutputRole::ToSelf
            }
            TxKind::SentToSelf
            | TxKind::Sent
            | TxKind::CreateBond
            | TxKind::BeginUnbond
            | TxKind::RetargetBond => OutputRole::Change,
        };
    }
    sent.extend(received);
    sent
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::{Pool, SpendState};

    fn output(pool: Pool, value: u64, index: u32, scope: &str) -> OutputRow {
        OutputRow {
            pool,
            role: OutputRole::Received,
            value,
            index,
            address: None,
            memo: None,
            spend: SpendState::Unspent,
            scope: Some(scope.to_string()),
        }
    }

    fn roles(outputs: &[OutputRow]) -> Vec<(Pool, u64, OutputRole)> {
        outputs.iter().map(|o| (o.pool, o.value, o.role)).collect()
    }

    #[test]
    fn a_bond_lists_each_output_once_and_calls_the_rest_change() {
        // 0.0102 ZEC of Orchard in, 0.01 ZEC bonded, a zero-value Orchard output as padding
        let received = vec![output(Pool::Orchard, 0, 1, "internal")];
        let outgoing = vec![output(Pool::Orchard, 0, 1, "internal")];
        let out = assign_roles(TxKind::CreateBond, received, outgoing);
        assert_eq!(
            roles(&out),
            vec![(Pool::Orchard, 0, OutputRole::Change)],
            "the outgoing copy of the padding is gone, and nothing is tagged sent"
        );
    }

    #[test]
    fn a_withdrawn_bond_comes_back_on_purpose() {
        let received = vec![output(Pool::Orchard, 1_000_000, 0, "internal")];
        let out = assign_roles(TxKind::WithdrawBond, received, vec![]);
        assert_eq!(
            roles(&out),
            vec![(Pool::Orchard, 1_000_000, OutputRole::ToSelf)]
        );
    }

    #[test]
    fn a_payment_keeps_its_recipient_and_calls_the_rest_change() {
        let received = vec![output(Pool::Orchard, 150_000, 1, "internal")];
        let outgoing = vec![output(Pool::Sapling, 10_000, 0, "external")];
        let out = assign_roles(TxKind::Sent, received, outgoing);
        assert_eq!(
            roles(&out),
            vec![
                (Pool::Sapling, 10_000, OutputRole::Sent),
                (Pool::Orchard, 150_000, OutputRole::Change),
            ]
        );
    }

    #[test]
    fn a_payment_to_one_of_the_wallets_own_addresses_is_to_self() {
        let received = vec![
            output(Pool::Transparent, 17_000, 2, "external"),
            output(Pool::Orchard, 150_000, 1, "internal"),
        ];
        let outgoing = vec![
            output(Pool::Transparent, 17_000, 2, "external"),
            output(Pool::Sapling, 23_000, 0, "external"),
        ];
        let out = assign_roles(TxKind::Sent, received, outgoing);
        assert_eq!(
            roles(&out),
            vec![
                (Pool::Sapling, 23_000, OutputRole::Sent),
                (Pool::Transparent, 17_000, OutputRole::ToSelf),
                (Pool::Orchard, 150_000, OutputRole::Change),
            ]
        );
    }

    #[test]
    fn a_same_pool_send_to_self_splits_purpose_from_change() {
        let received = vec![
            output(Pool::Orchard, 40_000, 0, "internal"),
            output(Pool::Orchard, 0, 1, "external"),
        ];
        let out = assign_roles(TxKind::SentToSelf, received, vec![]);
        assert_eq!(
            roles(&out),
            vec![
                (Pool::Orchard, 40_000, OutputRole::Change),
                (Pool::Orchard, 0, OutputRole::ToSelf),
            ]
        );
    }

    #[test]
    fn consolidating_into_change_is_all_to_self() {
        let received = vec![output(Pool::Sapling, 90_000, 0, "internal")];
        let out = assign_roles(TxKind::SentToSelf, received, vec![]);
        assert_eq!(
            roles(&out),
            vec![(Pool::Sapling, 90_000, OutputRole::ToSelf)]
        );
    }

    #[test]
    fn everything_in_a_received_transaction_is_received() {
        let received = vec![output(Pool::Orchard, 5, 0, "external")];
        let out = assign_roles(TxKind::Received, received, vec![]);
        assert_eq!(roles(&out), vec![(Pool::Orchard, 5, OutputRole::Received)]);
    }
}
