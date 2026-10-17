#![cfg(test)]

//! Tests for issue #2: guardian recovery mechanism for wills stuck in
//! `PendingConfirmation`.
//!
//! When a will is created with a confirmation delay and the owner becomes
//! incapacitated before calling `confirm_will`, guardians should be able to
//! cancel the will via quorum vote and return funds to the owner, without
//! waiting for the full `confirmation_deadline + 7 day` timeout that
//! `refund_unconfirmed_will` requires.

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient},
    vec, Address, Env, Vec as SorobanVec,
};

use crate::{Allocation, Beneficiary, WillContract, WillContractClient, WillError, WillStatus};

const DAY: u64 = 86_400;

fn setup<'a>() -> (
    Env,
    WillContractClient<'a>,
    Address,
    TokenClient<'a>,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);

    let owner = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(owner.clone());
    let token_address = sac.address();
    StellarAssetClient::new(&env, &token_address).mint(&owner, &1_000_000_000);

    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    (
        env.clone(),
        client,
        owner,
        TokenClient::new(&env, &token_address),
        token_address,
    )
}

fn advance(env: &Env, days: u64) {
    env.ledger().with_mut(|l| l.timestamp += days * DAY);
}

fn advance_seconds(env: &Env, secs: u64) {
    env.ledger().with_mut(|l| l.timestamp += secs);
}

/// Create a will in `PendingConfirmation` with two guardians at threshold 2.
/// Returns (will_id, guardian_a, guardian_b).
fn create_pending_with_guardians<'a>(
    env: &Env,
    client: &WillContractClient<'a>,
    owner: &Address,
    token_address: &Address,
    confirmation_window_days: u64,
) -> (u64, Address, Address) {
    let beneficiary = Address::generate(env);
    let guardian_a = Address::generate(env);
    let guardian_b = Address::generate(env);

    let beneficiaries: SorobanVec<Beneficiary> = vec![
        env,
        Beneficiary {
            address: beneficiary,
            allocation: Allocation::Percentage(10_000),
        },
    ];

    let will_id = client.create_will(
        owner,
        &vec![env, (token_address.clone(), 1_000_000_i128)],
        &beneficiaries,
        &90,
        &7,
        &vec![env, guardian_a.clone(), guardian_b.clone()],
        &2,
        &None,
        &(confirmation_window_days * DAY),
    );

    // Guardians accept their roles.
    client.accept_guardian_role(&will_id, &guardian_a);
    client.accept_guardian_role(&will_id, &guardian_b);

    (will_id, guardian_a, guardian_b)
}

/// Issue #2 — core case: a guardian quorum can cancel a `PendingConfirmation`
/// will after the confirmation deadline has elapsed, refunding the owner.
#[test]
fn guardian_quorum_cancels_pending_will_after_deadline() {
    let (env, client, owner, token, token_address) = setup();
    let initial_balance = token.balance(&owner);
    let deposit = 1_000_000_i128;

    let (will_id, guardian_a, guardian_b) =
        create_pending_with_guardians(&env, &client, &owner, &token_address, 30);

    // Owner's balance is reduced by the deposit.
    assert_eq!(token.balance(&owner), initial_balance - deposit);
    assert_eq!(
        client.get_will(&will_id).status,
        WillStatus::PendingConfirmation
    );

    // Advance past the 30-day confirmation deadline.
    advance(&env, 31);

    // First guardian votes.
    client.guardian_cancel_pending_will(&will_id, &guardian_a);

    // Will is still PendingConfirmation (quorum not yet reached).
    assert_eq!(
        client.get_will(&will_id).status,
        WillStatus::PendingConfirmation,
        "will must still be PendingConfirmation after one vote below quorum"
    );

    // Second guardian reaches quorum — will is cancelled and funds refunded.
    client.guardian_cancel_pending_will(&will_id, &guardian_b);

    let will = client.get_will(&will_id);
    assert_eq!(
        will.status,
        WillStatus::Cancelled,
        "will must be Cancelled once guardian quorum is reached"
    );
    assert_eq!(will.balance, 0, "cancelled will must have zero balance");

    // Tokens returned to owner.
    assert_eq!(
        token.balance(&owner),
        initial_balance,
        "owner must receive full refund on guardian cancellation"
    );
}

/// A guardian cannot vote while the confirmation window is still open — the
/// owner still has time to confirm; guardian interference is premature.
#[test]
fn guardian_cancel_pending_will_rejected_before_deadline() {
    let (env, client, owner, _token, token_address) = setup();

    let (will_id, guardian_a, _guardian_b) =
        create_pending_with_guardians(&env, &client, &owner, &token_address, 30);

    // Advance to just before the deadline.
    advance_seconds(&env, 30 * DAY - 100);

    assert_eq!(
        client.try_guardian_cancel_pending_will(&will_id, &guardian_a),
        Err(Ok(WillError::CheckinNotDue.into())),
        "guardian_cancel_pending_will must be rejected while the confirmation window is still open"
    );
}

/// Calling `guardian_cancel_pending_will` on an Active (already confirmed)
/// will must fail with `WillNotConfirmed`.
#[test]
fn guardian_cancel_pending_will_rejected_on_active_will() {
    let (env, client, owner, _token, token_address) = setup();

    // Create a will that goes immediately Active (no confirmation delay).
    let beneficiary = Address::generate(&env);
    let guardian = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_000_i128)],
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                allocation: Allocation::Percentage(10_000),
            },
        ],
        &90,
        &7,
        &vec![&env, guardian.clone()],
        &1,
        &None,
        &0, // no confirmation delay
    );

    client.accept_guardian_role(&will_id, &guardian);

    assert_eq!(
        client.try_guardian_cancel_pending_will(&will_id, &guardian),
        Err(Ok(WillError::WillNotConfirmed.into())),
        "guardian_cancel_pending_will must fail on an Active will"
    );
}

/// A non-guardian address is rejected with `NotGuardian`.
#[test]
fn guardian_cancel_pending_will_rejected_for_non_guardian() {
    let (env, client, owner, _token, token_address) = setup();
    let not_a_guardian = Address::generate(&env);

    let (will_id, _guardian_a, _guardian_b) =
        create_pending_with_guardians(&env, &client, &owner, &token_address, 30);

    advance(&env, 31);

    assert_eq!(
        client.try_guardian_cancel_pending_will(&will_id, &not_a_guardian),
        Err(Ok(WillError::NotGuardian.into())),
        "a non-guardian must be rejected with NotGuardian"
    );
}

/// A guardian who has not yet accepted their role cannot vote.
#[test]
fn guardian_cancel_pending_will_requires_consent() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let guardian = Address::generate(&env);

    // Create will but do NOT call accept_guardian_role.
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_000_i128)],
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                allocation: Allocation::Percentage(10_000),
            },
        ],
        &90,
        &7,
        &vec![&env, guardian.clone()],
        &1,
        &None,
        &(30 * DAY),
    );

    advance(&env, 31);

    assert_eq!(
        client.try_guardian_cancel_pending_will(&will_id, &guardian),
        Err(Ok(WillError::GuardianNotConsented.into())),
        "guardian without accepted consent must be rejected"
    );
}

/// Duplicate votes from the same guardian are rejected.
#[test]
fn guardian_cancel_pending_will_rejects_duplicate_vote() {
    let (env, client, owner, _token, token_address) = setup();

    let (will_id, guardian_a, _guardian_b) =
        create_pending_with_guardians(&env, &client, &owner, &token_address, 30);

    advance(&env, 31);

    // First vote succeeds.
    client.guardian_cancel_pending_will(&will_id, &guardian_a);

    // Second vote from the same guardian is rejected.
    assert_eq!(
        client.try_guardian_cancel_pending_will(&will_id, &guardian_a),
        Err(Ok(WillError::AlreadyVoted.into())),
        "the same guardian must not be able to vote twice"
    );
}

/// A single-guardian will with threshold 1 reaches quorum on the first vote.
#[test]
fn guardian_cancel_pending_will_single_guardian_threshold_one() {
    let (env, client, owner, token, token_address) = setup();
    let initial_balance = token.balance(&owner);
    let deposit = 1_000_000_i128;

    let beneficiary = Address::generate(&env);
    let guardian = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, deposit)],
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                allocation: Allocation::Percentage(10_000),
            },
        ],
        &90,
        &7,
        &vec![&env, guardian.clone()],
        &1,
        &None,
        &(30 * DAY),
    );

    client.accept_guardian_role(&will_id, &guardian);

    advance(&env, 31);

    // Single guardian at threshold 1 — quorum on first vote.
    client.guardian_cancel_pending_will(&will_id, &guardian);

    assert_eq!(
        client.get_will(&will_id).status,
        WillStatus::Cancelled,
        "will must be Cancelled after single guardian reaches threshold 1"
    );
    assert_eq!(
        token.balance(&owner),
        initial_balance,
        "owner must be fully refunded"
    );
}
