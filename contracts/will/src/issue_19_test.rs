#![cfg(test)]

//! Regression tests for issue #19: `retry_failed_payout` could only ever
//! deliver to the hardcoded recipient recorded at failure time, so a
//! permanently frozen, compromised, or migrated destination stranded the
//! funds with no recovery path. `redirect_failed_payout` moves the record to
//! a new destination, authorized by the original recipient or the will's
//! owner — never by an unauthorized third party.

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    vec, Address, Env,
};

use crate::{
    issue_18_test::{FailableToken, FailableTokenClient},
    storage, Allocation, Beneficiary, WillContract, WillContractClient, WillError,
};

const DAY: u64 = 86_400;

/// Registers a will whose single 100% beneficiary's payout will fail during
/// release, leaving one recorded failed payout of 500_000 for `beneficiary`.
#[allow(clippy::type_complexity)]
fn setup_with_failed_payout<'a>() -> (
    Env,
    WillContractClient<'a>,
    FailableTokenClient<'a>,
    Address,
    Address,
    Address,
    Address,
    u64,
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);

    let owner = Address::generate(&env);
    let beneficiary = Address::generate(&env);

    let token_id = env.register(FailableToken, ());
    let token_client = FailableTokenClient::new(&env, &token_id);
    token_client.mint(&owner, &500_000);

    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_id.clone(), 500_000_i128)],
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                allocation: Allocation::Percentage(10_000),
            },
        ],
        &90,
        &7,
        &vec![&env],
        &0,
        &None,
        &0,
    );

    // The token refuses transfers by the time the will is released, so
    // `distribute` records the beneficiary's share as a failed payout.
    token_client.set_fail_all(&true);
    advance(&env, 91);
    client.trigger_will(&will_id);
    advance(&env, 8);
    client.release_inheritance(&will_id, &None);
    token_client.set_fail_all(&false);

    assert_eq!(
        env.as_contract(&contract_id, || storage::get_failed_payout(
            &env,
            will_id,
            &token_id,
            &beneficiary
        )),
        Some(500_000),
        "precondition: distribute must have recorded the failed payout"
    );

    (
        env.clone(),
        client,
        token_client,
        owner,
        token_id,
        beneficiary,
        contract_id,
        will_id,
    )
}

fn advance(env: &Env, days: u64) {
    env.ledger().with_mut(|l| l.timestamp += days * DAY);
}

/// Issue #19: an unauthorized third party cannot redirect funds, no matter
/// which arguments they supply.
#[test]
fn unauthorized_caller_cannot_redirect_a_failed_payout() {
    let (env, client, _token_client, _owner, token_address, beneficiary, contract_id, will_id) =
        setup_with_failed_payout();
    let stranger = Address::generate(&env);
    let recovery = Address::generate(&env);

    assert_eq!(
        client.try_redirect_failed_payout(
            &will_id,
            &token_address,
            &beneficiary,
            &recovery,
            &stranger
        ),
        Err(Ok(WillError::NotAuthorized.into())),
        "a third party must never be able to redirect a stalled payout"
    );

    // The record is untouched.
    assert_eq!(
        env.as_contract(&contract_id, || storage::get_failed_payout(
            &env,
            will_id,
            &token_address,
            &beneficiary
        )),
        Some(500_000)
    );
}

/// Issue #19: the will's owner redirects the stalled payout to a recovery
/// address, and a retry delivers it there — exactly once, while the old
/// tuple can no longer be retried.
#[test]
fn owner_can_redirect_and_the_retry_delivers_to_the_new_recipient() {
    let (env, client, token_client, owner, token_address, beneficiary, contract_id, will_id) =
        setup_with_failed_payout();
    let recovery = Address::generate(&env);

    client.redirect_failed_payout(&will_id, &token_address, &beneficiary, &recovery, &owner);

    assert_eq!(
        env.as_contract(&contract_id, || storage::get_failed_payout(
            &env,
            will_id,
            &token_address,
            &beneficiary
        )),
        None,
        "the old tuple must be cleared by the redirect"
    );
    assert_eq!(
        env.as_contract(&contract_id, || storage::get_failed_payout(
            &env,
            will_id,
            &token_address,
            &recovery
        )),
        Some(500_000),
        "the record must move to the new recipient with the same amount"
    );

    // The old tuple is exhausted and the new one is payable.
    assert_eq!(
        client.try_retry_failed_payout(&will_id, &token_address, &beneficiary),
        Err(Ok(WillError::NoFailedPayout.into()))
    );
    client.retry_failed_payout(&will_id, &token_address, &recovery);
    assert_eq!(token_client.balance(&recovery), 500_000);
    assert_eq!(token_client.balance(&beneficiary), 0);
}

/// Issue #19: the recorded recipient can redirect their own stalled payout —
/// they are the party the funds were computed for.
#[test]
fn recorded_recipient_can_redirect_their_own_payout() {
    let (env, client, _token_client, _owner, token_address, beneficiary, _contract_id, will_id) =
        setup_with_failed_payout();
    let recovery = Address::generate(&env);

    client.redirect_failed_payout(
        &will_id,
        &token_address,
        &beneficiary,
        &recovery,
        &beneficiary,
    );

    assert_eq!(
        env.as_contract(&_contract_id, || storage::get_failed_payout(
            &env,
            will_id,
            &token_address,
            &recovery
        )),
        Some(500_000)
    );
}

/// Issue #19: an authorized caller with nothing recorded is told so, rather
/// than silently succeeding.
#[test]
fn authorized_redirect_without_a_record_fails() {
    let (env, client, _token_client, owner, token_address, beneficiary, _contract_id, will_id) =
        setup_with_failed_payout();
    let recovery = Address::generate(&env);

    // Clear the record by delivering it once.
    client.retry_failed_payout(&will_id, &token_address, &beneficiary);

    assert_eq!(
        client.try_redirect_failed_payout(
            &will_id,
            &token_address,
            &beneficiary,
            &recovery,
            &owner
        ),
        Err(Ok(WillError::NoFailedPayout.into()))
    );
}

/// Issue #19: the owner can still redirect after the will has been archived —
/// the lookup falls back to the archived namespace, mirroring
/// `retry_failed_payout`'s independence from the live will.
#[test]
fn owner_can_redirect_after_the_will_is_archived() {
    let (env, client, token_client, owner, token_address, beneficiary, _contract_id, will_id) =
        setup_with_failed_payout();
    let recovery = Address::generate(&env);

    client.close_will(&will_id, &owner);
    client.archive_will(&will_id);

    client.redirect_failed_payout(&will_id, &token_address, &beneficiary, &recovery, &owner);
    client.retry_failed_payout(&will_id, &token_address, &recovery);
    assert_eq!(token_client.balance(&recovery), 500_000);
}
