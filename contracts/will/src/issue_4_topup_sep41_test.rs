#![cfg(test)]

//! Tests for issue #4: `top_up` must validate new token addresses against the
//! SEP-41 interface before accepting a transfer.
//!
//! `create_will` already probes every token with `token::Client::new(&env,
//! &token).decimals()` and rejects non-compliant addresses with
//! `WillError::InvalidToken`. Before this fix, `top_up` skipped that probe for
//! new tokens, allowing a non-SEP-41 address to enter `will.balances`. A
//! subsequent `distribute()` call would then fail when iterating the balances
//! map, trapping all funds in the contract.

use soroban_sdk::{
    contract, contractimpl,
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient},
    vec, Address, Env,
};

use crate::{Allocation, Beneficiary, WillContract, WillContractClient, WillError};

/// A contract that deliberately does not implement the SEP-41 token interface
/// (no `decimals()`, no `transfer()`). Used to verify that `top_up` rejects
/// it before any state is modified.
#[contract]
pub struct NotATokenContract;

#[contractimpl]
impl NotATokenContract {
    pub fn ping(_env: Env) -> u32 {
        42
    }
}

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

fn make_will<'a>(
    env: &Env,
    client: &WillContractClient<'a>,
    owner: &Address,
    token_address: &Address,
) -> u64 {
    client.create_will(
        owner,
        &vec![env, (token_address.clone(), 1_000_000_i128)],
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

/// Issue #4 — core case: `top_up` with a non-SEP-41 address as a *new* token
/// must fail with `WillError::InvalidToken`.
#[test]
fn top_up_rejects_non_sep41_token_address() {
    let (env, client, owner, _token, token_address) = setup();
    let will_id = make_will(&env, &client, &owner, &token_address);

    // Register a contract that has no `decimals()` entrypoint.
    let not_a_token = env.register(NotATokenContract, ());

    // top_up with the non-token address must fail before any transfer occurs.
    assert_eq!(
        client.try_top_up(&will_id, &owner, &not_a_token, &1_000_i128),
        Err(Ok(WillError::InvalidToken.into())),
        "top_up must reject a non-SEP-41 token address with InvalidToken"
    );

    // The will's balances map must not have grown.
    let will = client.get_will(&will_id);
    assert_eq!(
        will.balances.len(),
        1,
        "will must still hold only the original token after a rejected top_up"
    );
}

/// Topping up with a valid SEP-41 token that is ALREADY in the will's balances
/// must not re-probe and must succeed normally — the probe is skipped for known
/// tokens to save CPU.
#[test]
fn top_up_existing_token_bypasses_probe_and_succeeds() {
    let (env, client, owner, _token, token_address) = setup();
    let will_id = make_will(&env, &client, &owner, &token_address);

    let before = client.get_will(&will_id).balance;

    // Top up with the same token already held.
    client.top_up(&will_id, &owner, &token_address, &500_000_i128);

    let will = client.get_will(&will_id);
    assert_eq!(
        will.balance,
        before + 500_000,
        "existing-token top_up must increase the balance"
    );
    assert_eq!(
        will.balances.len(),
        1,
        "topping up an existing token must not grow the balances map"
    );
}

/// Topping up with a *new* valid SEP-41 token must succeed — the probe passes
/// and the token is added to the balances map.
#[test]
fn top_up_new_valid_sep41_token_succeeds() {
    let (env, client, owner, _token, token_address) = setup();
    let will_id = make_will(&env, &client, &owner, &token_address);

    // Register a second real SAC token and mint some for the owner.
    let second_sac = env.register_stellar_asset_contract_v2(owner.clone());
    let second_token = second_sac.address();
    StellarAssetClient::new(&env, &second_token).mint(&owner, &500_000_i128);

    // top_up with the new valid token must succeed.
    client.top_up(&will_id, &owner, &second_token, &500_000_i128);

    let will = client.get_will(&will_id);
    assert_eq!(
        will.balances.len(),
        2,
        "a new valid SEP-41 token must be added to the balances map"
    );
    assert_eq!(
        will.balances.get(second_token).unwrap_or(0),
        500_000,
        "the new token's balance must be recorded correctly"
    );
}

/// The probe must not affect the `InvalidTokenCount` path: once the will holds
/// `MAX_TOKENS` distinct tokens, adding a new one (even a valid SEP-41 token)
/// must still fail with `InvalidTokenCount`.
#[test]
fn top_up_at_max_tokens_still_rejects_new_token() {
    use crate::MAX_TOKENS;

    let (env, client, owner, _token, token_address) = setup();
    let will_id = make_will(&env, &client, &owner, &token_address);

    // Fill the will to MAX_TOKENS by adding (MAX_TOKENS - 1) more SAC tokens.
    for _ in 1..MAX_TOKENS {
        let extra_sac = env.register_stellar_asset_contract_v2(owner.clone());
        let extra_token = extra_sac.address();
        StellarAssetClient::new(&env, &extra_token).mint(&owner, &1_000_i128);
        client.top_up(&will_id, &owner, &extra_token, &1_000_i128);
    }

    assert_eq!(
        client.get_will(&will_id).balances.len(),
        MAX_TOKENS,
        "will must hold exactly MAX_TOKENS tokens after setup"
    );

    // Now try to add one more valid SEP-41 token.
    let one_more_sac = env.register_stellar_asset_contract_v2(owner.clone());
    let one_more_token = one_more_sac.address();
    StellarAssetClient::new(&env, &one_more_token).mint(&owner, &1_000_i128);

    assert_eq!(
        client.try_top_up(&will_id, &owner, &one_more_token, &1_000_i128),
        Err(Ok(WillError::InvalidTokenCount.into())),
        "adding a token beyond MAX_TOKENS must fail with InvalidTokenCount"
    );
}

/// `top_up` on an `Active` will with the original token must still work —
/// no regression for the most common case.
#[test]
fn top_up_primary_token_regression() {
    let (env, client, owner, _token, token_address) = setup();
    let will_id = make_will(&env, &client, &owner, &token_address);

    client.top_up(&will_id, &owner, &token_address, &100_000_i128);

    assert_eq!(
        client.get_will(&will_id).balance,
        1_100_000,
        "primary-token top_up must increase balance by the supplied amount"
    );
}
