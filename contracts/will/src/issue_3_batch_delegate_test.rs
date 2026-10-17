#![cfg(test)]

//! Tests for issue #3: `batch_check_in` must accept an authorized delegate as
//! caller, mirroring the single-will `check_in` entrypoint.
//!
//! The single-will `check_in` allows either the owner or an authorized delegate
//! (`will.delegate`) to record a check-in. Before this fix, `batch_check_in`
//! only accepted the owner, making it impossible for automated delegates
//! (hot-wallet cron jobs, custody services) to batch check-ins across multiple
//! wills without individually calling `check_in` for each.

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient},
    vec, Address, Env,
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

/// Helper: create a simple active will owned by `owner`.
fn make_will<'a>(
    env: &Env,
    client: &WillContractClient<'a>,
    owner: &Address,
    token_address: &Address,
) -> u64 {
    client.create_will(
        owner,
        &vec![env, (token_address.clone(), 100_000_i128)],
        &vec![
            env,
            Beneficiary {
                address: Address::generate(env),
                allocation: Allocation::Percentage(10_000),
            },
        ],
        &90,
        &7,
        &vec![env],
        &0,
        &None,
        &0,
    )
}

/// Issue #3 — core case: an authorized delegate can call `batch_check_in`
/// across three wills for which they are appointed.
#[test]
fn delegate_can_batch_check_in_across_multiple_wills() {
    let (env, client, owner, _token, token_address) = setup();
    let delegate = Address::generate(&env);

    // Create three wills and appoint the same delegate to each.
    let w1 = make_will(&env, &client, &owner, &token_address);
    let w2 = make_will(&env, &client, &owner, &token_address);
    let w3 = make_will(&env, &client, &owner, &token_address);

    client.set_delegate(&w1, &owner, &Some(delegate.clone()));
    client.set_delegate(&w2, &owner, &Some(delegate.clone()));
    client.set_delegate(&w3, &owner, &Some(delegate.clone()));

    // Advance near the deadline.
    advance(&env, 80);

    // The delegate batch-checks in on behalf of the owner.
    client.batch_check_in(&vec![&env, w1, w2, w3], &delegate);

    // All wills remain Active.
    for &id in &[w1, w2, w3] {
        assert_eq!(
            client.get_will(&id).status,
            WillStatus::Active,
            "will {} must remain Active after delegate batch check-in",
            id
        );
    }
}

/// The will owner can still call `batch_check_in` with their own address —
/// the rename from `owner` to `caller` must not break the owner path.
#[test]
fn owner_can_still_batch_check_in() {
    let (env, client, owner, _token, token_address) = setup();

    let w1 = make_will(&env, &client, &owner, &token_address);
    let w2 = make_will(&env, &client, &owner, &token_address);

    advance(&env, 80);

    // Owner calls batch_check_in — must still succeed.
    client.batch_check_in(&vec![&env, w1, w2], &owner);

    for &id in &[w1, w2] {
        assert_eq!(client.get_will(&id).status, WillStatus::Active);
    }
}

/// A caller who is neither the owner nor the delegate of a will in the batch
/// must be rejected with `NotOwner`.
#[test]
fn unauthorized_caller_rejected_in_batch_check_in() {
    let (env, client, owner, _token, token_address) = setup();
    let delegate = Address::generate(&env);
    let intruder = Address::generate(&env);

    let w1 = make_will(&env, &client, &owner, &token_address);
    let w2 = make_will(&env, &client, &owner, &token_address);

    // Delegate is only set on w1, not w2.
    client.set_delegate(&w1, &owner, &Some(delegate.clone()));

    advance(&env, 80);

    // A random address that is neither owner nor delegate of any will must fail.
    assert_eq!(
        client.try_batch_check_in(&vec![&env, w1, w2], &intruder),
        Err(Ok(WillError::NotOwner.into())),
        "an unauthorized caller must be rejected with NotOwner"
    );
}

/// A delegate who is not appointed to one will in the batch must fail.
/// The delegate is appointed to w1 but not w2; calling batch_check_in for
/// both must fail when w2 is reached.
#[test]
fn delegate_rejected_for_will_without_delegation() {
    let (env, client, owner, _token, token_address) = setup();
    let delegate = Address::generate(&env);

    let w1 = make_will(&env, &client, &owner, &token_address);
    let w2 = make_will(&env, &client, &owner, &token_address);

    // Delegate only appointed to w1.
    client.set_delegate(&w1, &owner, &Some(delegate.clone()));

    advance(&env, 80);

    // The delegate is not authorized for w2.
    assert_eq!(
        client.try_batch_check_in(&vec![&env, w1, w2], &delegate),
        Err(Ok(WillError::NotOwner.into())),
        "delegate must be rejected when not appointed to all wills in the batch"
    );
}

/// Removing the delegate and then calling batch_check_in as the former
/// delegate must fail — clearing the delegate revokes the permission.
#[test]
fn cleared_delegate_cannot_batch_check_in() {
    let (env, client, owner, _token, token_address) = setup();
    let delegate = Address::generate(&env);

    let w1 = make_will(&env, &client, &owner, &token_address);
    client.set_delegate(&w1, &owner, &Some(delegate.clone()));

    // Immediately clear the delegate.
    client.set_delegate(&w1, &owner, &None);

    advance(&env, 80);

    assert_eq!(
        client.try_batch_check_in(&vec![&env, w1], &delegate),
        Err(Ok(WillError::NotOwner.into())),
        "a cleared delegate must no longer be able to batch check-in"
    );
}
