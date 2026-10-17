#![cfg(test)]

//! Tests for hashed beneficiary functionality.
//! Issue #182: add_hashed_beneficiary validation with percentage beneficiaries
//! Issue #181: Hashed beneficiaries' share preservation during release
//! Issue #92: renounce_hashed_beneficiary with pre-image

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

fn release(env: &Env, client: &WillContractClient, will_id: u64) {
    advance(env, 91);
    client.trigger_will(&will_id);
    advance(env, 8);
    client.release_inheritance(&will_id, &None);
}

/// Issue #182 / #371: a percentage-based beneficiary list already consumes the
/// whole 10,000 bps, so the only hashed share that fits is zero — and a zero
/// percentage is now rejected outright (it reserves nothing yet still occupies
/// a slot and dilutes every other hashed beneficiary's share of the withheld
/// pool). Before #371 this test asserted that a 0% slot was accepted.
#[test]
fn zero_percentage_hashed_beneficiary_is_rejected_on_a_full_percentage_list() {
    let (env, client, owner, _token, token_address) = setup();
    let reg_beneficiary = Address::generate(&env);

    let beneficiaries: SorobanVec<Beneficiary> = vec![
        &env,
        Beneficiary {
            address: reg_beneficiary.clone(),
            allocation: Allocation::Percentage(10_000),
        },
    ];
    let tokens: SorobanVec<(Address, i128)> = vec![&env, (token_address, 1_000_000_i128)];
    let will_id = client.create_will(
        &owner,
        &tokens,
        &beneficiaries,
        &90,
        &7,
        &vec![&env],
        &0,
        &None,
        &0,
    );

    let hashed_commitment = env.crypto().sha256(&soroban_sdk::Bytes::new(&env));
    let hashed_bytes = soroban_sdk::Bytes::from_array(&env, &hashed_commitment.to_array());

    // Zero is rejected for being zero, not for overflowing the 10,000 bps cap.
    assert_eq!(
        client.try_add_hashed_beneficiary(&will_id, &owner, &hashed_bytes, &0),
        Err(Ok(WillError::InvalidPercentages.into())),
    );
    assert_eq!(client.get_will(&will_id).hashed_beneficiaries.len(), 0);
}

/// Issue #182: Test that `add_hashed_beneficiary` validates combined percentages correctly.
///
/// Ensures that percentage-based beneficiaries plus hashed beneficiary percentages
/// cannot exceed 10,000 basis points.
#[test]
fn hashed_beneficiary_percentage_exceeds_limit() {
    let (env, client, owner, _, token_address) = setup();
    let reg_beneficiary_a = Address::generate(&env);
    let reg_beneficiary_b = Address::generate(&env);

    let beneficiaries: SorobanVec<Beneficiary> = vec![
        &env,
        Beneficiary {
            address: reg_beneficiary_a,
            allocation: Allocation::Percentage(5_000),
        },
        Beneficiary {
            address: reg_beneficiary_b,
            allocation: Allocation::Percentage(5_000),
        },
    ];
    let tokens: SorobanVec<(Address, i128)> = vec![&env, (token_address, 1_000_000_i128)];
    let will_id = client.create_will(
        &owner,
        &tokens,
        &beneficiaries,
        &90,
        &7,
        &vec![&env],
        &0,
        &None,
        &0,
    );

    let hashed_commitment = env.crypto().sha256(&soroban_sdk::Bytes::new(&env));
    let hashed_bytes = soroban_sdk::Bytes::from_array(&env, &hashed_commitment.to_array());

    // This should be rejected with InvalidPercentages. Soroban's panic
    // message only shows the numeric error code, never the enum variant
    // name, so #[should_panic(expected = "InvalidPercentages")] can never
    // match -- use try_add_hashed_beneficiary instead.
    assert_eq!(
        client.try_add_hashed_beneficiary(&will_id, &owner, &hashed_bytes, &100),
        Err(Ok(WillError::InvalidPercentages.into())),
    );
}

/// Issue #182: Test that `add_hashed_beneficiary` works with fixed-amount beneficiaries.
///
/// Hashed beneficiaries can be added with percentage allocations when regular
/// beneficiaries use fixed amounts, since fixed amounts don't consume the
/// percentage pool.
#[test]
fn hashed_beneficiary_with_fixed_amount_beneficiaries() {
    let (env, client, owner, _token, token_address) = setup();
    let reg_beneficiary = Address::generate(&env);

    let beneficiaries: SorobanVec<Beneficiary> = vec![
        &env,
        Beneficiary {
            address: reg_beneficiary,
            allocation: Allocation::FixedAmount(500_000),
        },
    ];
    let tokens: SorobanVec<(Address, i128)> = vec![&env, (token_address, 500_000_i128)];
    let will_id = client.create_will(
        &owner,
        &tokens,
        &beneficiaries,
        &90,
        &7,
        &vec![&env],
        &0,
        &None,
        &0,
    );

    let hashed_commitment = env.crypto().sha256(&soroban_sdk::Bytes::new(&env));
    let hashed_bytes = soroban_sdk::Bytes::from_array(&env, &hashed_commitment.to_array());

    // Add hashed beneficiary with a percentage share
    client.add_hashed_beneficiary(&will_id, &owner, &hashed_bytes, &5_000);

    let will = client.get_will(&will_id);
    assert_eq!(will.hashed_beneficiaries.len(), 1);
    assert_eq!(will.hashed_beneficiaries.get_unchecked(0).percentage, 5_000);
}

/// Issue #182: Test that multiple hashed beneficiaries can be added.
///
/// Multiple hashed beneficiaries can coexist on the same will as long as
/// their combined percentages don't exceed the validation limit. Since #371
/// each must also carry a *distinct* 32-byte commitment — a duplicate would be
/// permanently unclaimable, since `reveal_and_claim` always matches the first
/// matching slot.
#[test]
fn multiple_hashed_beneficiaries() {
    let (env, client, owner, _token, token_address) = setup();
    let reg_beneficiary = Address::generate(&env);

    let beneficiaries: SorobanVec<Beneficiary> = vec![
        &env,
        Beneficiary {
            address: reg_beneficiary,
            allocation: Allocation::FixedAmount(500_000),
        },
    ];
    let tokens: SorobanVec<(Address, i128)> = vec![&env, (token_address, 500_000_i128)];
    let will_id = client.create_will(
        &owner,
        &tokens,
        &beneficiaries,
        &90,
        &7,
        &vec![&env],
        &0,
        &None,
        &0,
    );

    // Add multiple hashed beneficiaries, each with its own commitment.
    for i in 0..3u8 {
        let preimage = soroban_sdk::Bytes::from_array(&env, &[i; 64]);
        let hashed_commitment = env.crypto().sha256(&preimage);
        let hashed_bytes = soroban_sdk::Bytes::from_array(&env, &hashed_commitment.to_array());
        client.add_hashed_beneficiary(&will_id, &owner, &hashed_bytes, &2_000);
    }

    let will = client.get_will(&will_id);
    assert_eq!(will.hashed_beneficiaries.len(), 3);
}

/// Issue #92: A hashed beneficiary can renounce their claim by presenting the
/// pre-image. The commitment is removed from storage and its basis points are
/// returned to the estate pool.
#[test]
fn renounce_hashed_beneficiary_with_preimage() {
    let (env, client, owner, _token, token_address) = setup();
    let reg_beneficiary = Address::generate(&env);

    let beneficiaries: SorobanVec<Beneficiary> = vec![
        &env,
        Beneficiary {
            address: reg_beneficiary,
            allocation: Allocation::FixedAmount(500_000),
        },
    ];
    let tokens: SorobanVec<(Address, i128)> = vec![&env, (token_address, 500_000_i128)];
    let will_id = client.create_will(
        &owner,
        &tokens,
        &beneficiaries,
        &90,
        &7,
        &vec![&env],
        &0,
        &None,
        &0,
    );

    let preimage = soroban_sdk::Bytes::from_array(&env, &[7u8; 64]);
    let hashed_commitment = env.crypto().sha256(&preimage);
    let hashed_bytes = soroban_sdk::Bytes::from_array(&env, &hashed_commitment.to_array());
    client.add_hashed_beneficiary(&will_id, &owner, &hashed_bytes, &2_500);

    let will = client.get_will(&will_id);
    assert_eq!(will.hashed_beneficiaries.len(), 1);

    client.renounce_hashed_beneficiary(&will_id, &preimage);

    let will = client.get_will(&will_id);
    assert_eq!(will.hashed_beneficiaries.len(), 0);
}

/// Issue #92: Renouncing with a pre-image that does not match any commitment
/// is rejected.
#[test]
fn renounce_hashed_beneficiary_unknown_preimage_fails() {
    let (env, client, owner, _token, token_address) = setup();
    let reg_beneficiary = Address::generate(&env);

    let beneficiaries: SorobanVec<Beneficiary> = vec![
        &env,
        Beneficiary {
            address: reg_beneficiary,
            allocation: Allocation::FixedAmount(500_000),
        },
    ];
    let tokens: SorobanVec<(Address, i128)> = vec![&env, (token_address, 500_000_i128)];
    let will_id = client.create_will(
        &owner,
        &tokens,
        &beneficiaries,
        &90,
        &7,
        &vec![&env],
        &0,
        &None,
        &0,
    );

    let preimage = soroban_sdk::Bytes::from_array(&env, &[7u8; 64]);
    let hashed_commitment = env.crypto().sha256(&preimage);
    let hashed_bytes = soroban_sdk::Bytes::from_array(&env, &hashed_commitment.to_array());
    client.add_hashed_beneficiary(&will_id, &owner, &hashed_bytes, &2_500);

    let wrong_preimage = soroban_sdk::Bytes::from_array(&env, &[9u8; 64]);
    assert!(client
        .try_renounce_hashed_beneficiary(&will_id, &wrong_preimage)
        .is_err());

    let will = client.get_will(&will_id);
    assert_eq!(will.hashed_beneficiaries.len(), 1);
}
