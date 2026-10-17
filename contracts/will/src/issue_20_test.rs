#![cfg(test)]

//! Regression tests for issue #20: `batch_create_wills` locked tokens without
//! the read-only `decimals()` SEP-41 interface probe `create_will` runs, so a
//! batch could create wills around non-token addresses that would only ever
//! fail — opaquely — at transfer or release time. Every token in every batch
//! spec is now probed identically, and a failing probe aborts the whole batch
//! with [`WillError::InvalidToken`].

use soroban_sdk::{
    contract, contractimpl,
    testutils::{Address as _, Ledger},
    token::StellarAssetClient,
    vec, Address, Env, Vec as SorobanVec,
};

use crate::{Allocation, Beneficiary, WillContract, WillContractClient, WillError};

/// A contract that deliberately does not implement the SEP-41 token
/// interface — in particular, no `decimals()` and no `transfer`.
#[contract]
pub struct NotAToken;

#[contractimpl]
impl NotAToken {
    pub fn ping(_env: Env) -> u32 {
        1
    }
}

fn setup<'a>() -> (Env, WillContractClient<'a>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);

    let owner = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(owner.clone());
    let token_address = sac.address();
    StellarAssetClient::new(&env, &token_address).mint(&owner, &100_000_000);

    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    (env.clone(), client, owner, token_address)
}

fn beneficiary(env: &Env) -> Beneficiary {
    Beneficiary {
        address: Address::generate(env),
        allocation: Allocation::Percentage(10_000),
    }
}

type WillSpec = (
    SorobanVec<(Address, i128)>,
    SorobanVec<Beneficiary>,
    u64,
    u64,
    SorobanVec<Address>,
    u32,
);

fn spec(env: &Env, tokens: SorobanVec<(Address, i128)>) -> WillSpec {
    (tokens, vec![env, beneficiary(env)], 90, 7, vec![env], 0)
}

/// Issue #20: an invalid token in a *secondary* slot of a batch spec aborts
/// the batch with the same typed `InvalidToken` `create_will` raises — before
/// the fix the probe was skipped entirely and the batch either failed
/// opaquely inside `transfer` or (for a transfer-shaped non-token) locked a
/// will whose payouts could never succeed.
#[test]
fn batch_create_wills_probes_secondary_tokens() {
    let (env, client, owner, token_address) = setup();
    let not_a_token = env.register(NotAToken, ());

    let specs = vec![
        &env,
        spec(
            &env,
            vec![
                &env,
                (token_address.clone(), 1_000_i128),
                (not_a_token, 1_000_i128),
            ],
        ),
    ];

    assert_eq!(
        client.try_batch_create_wills(&owner, &specs),
        Err(Ok(WillError::InvalidToken.into())),
        "a non-SEP-41 address in any token slot must abort the batch with InvalidToken"
    );
}

/// Issue #20: an invalid token in the *first* slot of any batch spec aborts
/// the batch the same way, and a later spec is never reached.
#[test]
fn batch_create_wills_probes_every_specs_first_token() {
    let (env, client, owner, token_address) = setup();
    let not_a_token = env.register(NotAToken, ());

    let specs = vec![
        &env,
        spec(&env, vec![&env, (token_address, 1_000_i128)]),
        spec(&env, vec![&env, (not_a_token, 1_000_i128)]),
    ];

    assert_eq!(
        client.try_batch_create_wills(&owner, &specs),
        Err(Ok(WillError::InvalidToken.into()))
    );
}

/// The probe must not over-reject: batches of genuine SEP-41 tokens still
/// succeed, exactly as before.
#[test]
fn batch_create_wills_still_accepts_standard_tokens() {
    let (env, client, owner, token_address) = setup();

    let specs = vec![
        &env,
        spec(&env, vec![&env, (token_address.clone(), 1_000_i128)]),
        spec(&env, vec![&env, (token_address, 2_000_i128)]),
    ];

    let ids = client.batch_create_wills(&owner, &specs);
    assert_eq!(ids.len(), 2);
}
