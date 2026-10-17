#![cfg(test)]

//! Regression tests for issue #1: `archive_will` must not be callable on a
//! `Released` will that still has unclaimed hashed beneficiary shares.
//!
//! `reveal_and_claim` loads the will by id to verify the commitment; if
//! `archive_will` deleted that entry first, the reserved funds would be
//! trapped in the contract with no retrieval path. The fix guards archival
//! with a check that `unclaimed_hashed_bps > 0` → reject with
//! `WillError::UnclaimedHashedBeneficiary`.

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient},
    vec,
    xdr::ToXdr,
    Address, Bytes, Env, Vec as SorobanVec,
};

use crate::{
    Allocation, Beneficiary, WillContract, WillContractClient, WillError, WillStatus,
    PREIMAGE_ADDRESS_LENGTH, PREIMAGE_LENGTH,
};

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

/// Builds the documented pre-image for `beneficiary`: the first
/// `PREIMAGE_ADDRESS_LENGTH` bytes are `sha256(xdr(beneficiary))`, and the
/// remaining bytes are a fixed salt — the same derivation `reveal_and_claim`
/// recomputes internally (#369).
fn preimage_for(env: &Env, beneficiary: &Address, salt: u8) -> Bytes {
    let mut raw = [salt; PREIMAGE_LENGTH as usize];
    let digest = env.crypto().sha256(&beneficiary.clone().to_xdr(env));
    raw[..PREIMAGE_ADDRESS_LENGTH as usize].copy_from_slice(&digest.to_array());
    Bytes::from_array(env, &raw)
}

/// Computes the SHA-256 commitment from a pre-image.
fn commitment_for(env: &Env, preimage: &Bytes) -> Bytes {
    let digest = env.crypto().sha256(preimage);
    Bytes::from_array(env, &digest.to_array())
}

/// Issue #1 — core regression: `archive_will` must be rejected with
/// `UnclaimedHashedBeneficiary` while any hashed slot is still unclaimed.
///
/// Steps:
/// 1. Create a will with one regular beneficiary (50 %) and one hashed
///    beneficiary (50 %).
/// 2. Release the will.
/// 3. Attempt `archive_will` → must fail with `UnclaimedHashedBeneficiary`.
/// 4. Call `reveal_and_claim` to claim the hashed slot.
/// 5. Now `archive_will` must succeed.
#[test]
fn archive_will_blocked_while_hashed_beneficiary_unclaimed() {
    let (env, client, owner, _token, token_address) = setup();
    let visible_beneficiary = Address::generate(&env);
    let hashed_claimant = Address::generate(&env);

    let tokens: SorobanVec<(Address, i128)> = vec![&env, (token_address, 1_000_000_i128)];
    let beneficiaries: SorobanVec<Beneficiary> = vec![
        &env,
        Beneficiary {
            address: visible_beneficiary,
            allocation: Allocation::Percentage(5_000),
        },
    ];

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

    // Add a hashed beneficiary (50 %).
    let preimage = preimage_for(&env, &hashed_claimant, 0xab);
    let commitment = commitment_for(&env, &preimage);
    client.add_hashed_beneficiary(&will_id, &owner, &commitment, &5_000);

    // Trigger and release the will.
    advance(&env, 91);
    client.trigger_will(&will_id);
    advance(&env, 8);
    client.release_inheritance(&will_id, &None);

    // The will is Released but the hashed slot is still unclaimed.
    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);

    // archive_will must be rejected while any unclaimed hashed share exists.
    assert_eq!(
        client.try_archive_will(&will_id),
        Err(Ok(WillError::UnclaimedHashedBeneficiary.into())),
        "archive_will must fail with UnclaimedHashedBeneficiary while a hashed slot is unclaimed"
    );

    // The hashed beneficiary claims their share.
    client.reveal_and_claim(&will_id, &hashed_claimant, &preimage);

    // Now all hashed slots are claimed — archive_will must succeed.
    client.archive_will(&will_id);

    // The will entry is gone from active storage.
    assert!(
        client.try_get_will(&will_id).is_err(),
        "archived will must no longer be retrievable via get_will"
    );
}

/// Cancelled wills never have residual hashed-beneficiary state that could
/// be claimed (the will was never released and no funds were distributed),
/// so `archive_will` on a `Cancelled` will must always succeed regardless of
/// whether the `hashed_beneficiaries` vec is populated.
#[test]
fn archive_will_allows_cancelled_will_with_hashed_beneficiary_slots() {
    let (env, client, owner, _token, token_address) = setup();
    let visible_beneficiary = Address::generate(&env);
    let hashed_claimant = Address::generate(&env);

    let tokens: SorobanVec<(Address, i128)> = vec![&env, (token_address, 1_000_000_i128)];
    let beneficiaries: SorobanVec<Beneficiary> = vec![
        &env,
        Beneficiary {
            address: visible_beneficiary,
            allocation: Allocation::Percentage(5_000),
        },
    ];

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

    // Add a hashed beneficiary before cancelling.
    let preimage = preimage_for(&env, &hashed_claimant, 0xcd);
    let commitment = commitment_for(&env, &preimage);
    client.add_hashed_beneficiary(&will_id, &owner, &commitment, &5_000);

    // Cancel rather than release — no funds were distributed.
    client.cancel_will(&will_id, &owner);

    // Cancelled will must be archivable; the guard only blocks Released wills
    // with an unclaimed hashed share, not Cancelled ones.
    client.archive_will(&will_id);
}

/// A released will with NO hashed beneficiaries must still be archivable —
/// the guard must not block ordinary wills with only visible beneficiaries.
#[test]
fn archive_will_released_without_hashed_beneficiaries_succeeds() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

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
        &vec![&env],
        &0,
        &None,
        &0,
    );

    advance(&env, 91);
    client.trigger_will(&will_id);
    advance(&env, 8);
    client.release_inheritance(&will_id, &None);

    // No hashed beneficiaries — archive must succeed immediately.
    client.archive_will(&will_id);
}
