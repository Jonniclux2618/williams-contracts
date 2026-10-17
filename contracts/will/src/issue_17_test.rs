#![cfg(test)]

//! Regression tests for issue #17: `merge_wills` preserved the surviving
//! will's `guardian_threshold` verbatim without validating it against the
//! merged guardian list. A guardian-less survivor kept `0` while inheriting
//! the consumed will's guardians — a quorum any single guardian could satisfy
//! instantly — and a survivor with a weaker threshold silently diluted the
//! stronger quorum both wills' creators had configured. The merge now adopts
//! the stricter (larger) of the two source thresholds and re-validates it
//! against the merged list's total weight.

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::StellarAssetClient,
    vec, Address, Env, Vec as SorobanVec,
};

use crate::{
    storage, Allocation, Beneficiary, GuardianVoteReason, WillContract, WillContractClient,
    WillError, WillStatus,
};

const DAY: u64 = 86_400;

/// Guardian voting is blocked for this long after the guardian list changes.
/// The merge keeps the survivor's original `guardian_list_updated_at`, so the
/// voting test advances past the cooldown before casting votes.
const GUARDIAN_COOLDOWN_DAYS: u64 = 7;

fn setup<'a>() -> (Env, WillContractClient<'a>, Address, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);

    let owner = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(owner.clone());
    let token_address = sac.address();
    StellarAssetClient::new(&env, &token_address).mint(&owner, &100_000_000);

    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    (env.clone(), client, owner, token_address, contract_id)
}

fn advance(env: &Env, days: u64) {
    env.ledger().with_mut(|l| l.timestamp += days * DAY);
}

fn create_will(
    env: &Env,
    client: &WillContractClient,
    owner: &Address,
    token: &Address,
    amount: i128,
    guardians: &[Address],
    threshold: u32,
) -> u64 {
    let mut guardian_list: SorobanVec<Address> = SorobanVec::new(env);
    for guardian in guardians {
        guardian_list.push_back(guardian.clone());
    }
    client.create_will(
        owner,
        &vec![env, (token.clone(), amount)],
        &vec![
            env,
            Beneficiary {
                address: Address::generate(env),
                allocation: Allocation::Percentage(10_000),
            },
        ],
        &90,
        &7,
        &guardian_list,
        &threshold,
        &None,
        &0,
    )
}

/// Overwrites a will's stored threshold directly, bypassing entry-point
/// validation, to construct the unreachable-quorum state the merge must
/// reject defensively.
fn corrupt_threshold(env: &Env, contract_id: &Address, will_id: u64, threshold: u32) {
    let client = WillContractClient::new(env, contract_id);
    let mut will = client.get_will(&will_id);
    will.guardian_threshold = threshold;
    env.as_contract(contract_id, || storage::save_will(env, &will));
}

/// Issue #17: the merged will adopts the stricter of the two source
/// thresholds. Before the fix it kept the survivor's `1` verbatim, diluting
/// the 2-of-2 quorum the consumed will's creator had configured.
#[test]
fn merge_adopts_the_stricter_of_the_two_thresholds() {
    let (env, client, owner, token, _contract_id) = setup();
    let g1 = Address::generate(&env);
    let g2 = Address::generate(&env);
    let g3 = Address::generate(&env);

    let a = create_will(
        &env,
        &client,
        &owner,
        &token,
        1_000_000,
        std::slice::from_ref(&g1),
        1,
    );
    let b = create_will(
        &env,
        &client,
        &owner,
        &token,
        2_000_000,
        &[g2.clone(), g3.clone()],
        2,
    );

    client.merge_wills(&owner, &a, &b);

    let merged = client.get_will(&a);
    assert_eq!(
        merged.guardian_threshold, 2,
        "the merged will must adopt the stricter (2) of the two thresholds"
    );
    assert_eq!(
        merged.guardians.len(),
        3,
        "guardians from both wills are merged by address"
    );
}

/// Issue #17: a guardian-less survivor used to keep threshold `0` while
/// inheriting the consumed will's guardians, letting any single guardian
/// trigger instantly. The merge now adopts the consumed will's threshold and
/// quorum is actually enforced afterwards.
#[test]
fn guardianless_survivor_inherits_the_consumed_wills_threshold() {
    let (env, client, owner, token, _contract_id) = setup();
    let g2 = Address::generate(&env);
    let g3 = Address::generate(&env);

    let a = create_will(&env, &client, &owner, &token, 1_000_000, &[], 0);
    let b = create_will(
        &env,
        &client,
        &owner,
        &token,
        2_000_000,
        &[g2.clone(), g3.clone()],
        2,
    );

    client.merge_wills(&owner, &a, &b);

    let merged = client.get_will(&a);
    assert_eq!(
        merged.guardian_threshold, 2,
        "a threshold of 0 over a live guardian list would make any single vote quorum"
    );
    assert_eq!(merged.guardians.len(), 2);

    // Quorum is enforced after the merge: one guardian's vote is not enough.
    advance(&env, GUARDIAN_COOLDOWN_DAYS + 1);
    client.accept_guardian_role(&a, &g2);
    client.guardian_trigger(&a, &g2, &GuardianVoteReason::Unreachable);
    assert_eq!(
        client.get_will_status(&a),
        WillStatus::Active,
        "1 of 2 weighted votes must not reach the adopted threshold of 2"
    );

    client.accept_guardian_role(&a, &g3);
    client.guardian_trigger(&a, &g3, &GuardianVoteReason::Unreachable);
    assert_eq!(
        client.get_will_status(&a),
        WillStatus::Released,
        "the second vote must reach the adopted threshold of 2"
    );
}

/// Issue #17: when the survivor's threshold is already the stricter one it is
/// kept, so ordinary merges of equally-configured wills are unaffected.
#[test]
fn merge_keeps_the_survivors_stronger_threshold() {
    let (env, client, owner, token, _contract_id) = setup();
    let g1 = Address::generate(&env);
    let g2 = Address::generate(&env);
    let g3 = Address::generate(&env);

    let a = create_will(
        &env,
        &client,
        &owner,
        &token,
        1_000_000,
        &[g1.clone(), g2.clone()],
        2,
    );
    let b = create_will(
        &env,
        &client,
        &owner,
        &token,
        2_000_000,
        std::slice::from_ref(&g3),
        1,
    );

    client.merge_wills(&owner, &a, &b);

    let merged = client.get_will(&a);
    assert_eq!(
        merged.guardian_threshold, 2,
        "the stricter of the two source thresholds must be adopted"
    );
    assert_eq!(merged.guardians.len(), 3);
}

/// Issue #17: a threshold the merged list cannot reach is rejected with the
/// same typed error the guardian-update paths use, instead of silently
/// persisting a quorum that can never be satisfied.
#[test]
fn merge_rejects_a_threshold_the_merged_list_cannot_reach() {
    let (env, client, owner, token, contract_id) = setup();
    let g1 = Address::generate(&env);
    let g2 = Address::generate(&env);

    let a = create_will(
        &env,
        &client,
        &owner,
        &token,
        1_000_000,
        std::slice::from_ref(&g1),
        1,
    );
    let b = create_will(
        &env,
        &client,
        &owner,
        &token,
        2_000_000,
        std::slice::from_ref(&g2),
        1,
    );

    // Reachable on each source list (1 <= 1); corrupt the survivor's stored
    // threshold to 3 — above the merged total weight of 2 — to prove the
    // merge validates the adopted threshold rather than trusting stored
    // state.
    corrupt_threshold(&env, &contract_id, a, 3);

    assert_eq!(
        client.try_merge_wills(&owner, &a, &b),
        Err(Ok(WillError::InvalidGuardianThreshold.into())),
        "a threshold above the merged total weight (2) must be rejected"
    );
}

/// Issue #17: a stray non-zero threshold on a guardian-less survivor (a
/// pre-#363 legacy state) is normalised to `0` by the merge, mirroring
/// `create_will`, instead of persisting an unreachable quorum.
#[test]
fn merge_normalises_a_stray_threshold_when_no_guardians_remain() {
    let (env, client, owner, token, contract_id) = setup();

    let a = create_will(&env, &client, &owner, &token, 1_000_000, &[], 0);
    let b = create_will(&env, &client, &owner, &token, 2_000_000, &[], 0);
    corrupt_threshold(&env, &contract_id, a, 3);

    client.merge_wills(&owner, &a, &b);

    let merged = client.get_will(&a);
    assert_eq!(merged.guardians.len(), 0);
    assert_eq!(
        merged.guardian_threshold, 0,
        "no guardians means the guardian mechanism is disabled and the threshold is 0"
    );
}
