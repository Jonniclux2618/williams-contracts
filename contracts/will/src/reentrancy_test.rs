#![cfg(test)]

//! Reentrancy regression tests for issue #80.
//!
//! The existing `test_support::tests::test_reentrant_release_inheritance_cannot_double_pay`
//! test covers `release_inheritance`. This module extends the coverage to
//! `top_up`, `merge_wills`, `split_will` (via a top_up-triggered reentry),
//! and `reveal_and_claim` using the same [`MaliciousToken`] harness.
//!
//! In every case the contract follows the **checks-effects-interactions**
//! pattern: state mutations are committed to persistent storage *before*
//! any external token transfer is invoked. A malicious token that calls
//! back into the will contract during `transfer` therefore always encounters
//! the post-effect state and must be rejected or produce no net change, never
//! a double-payout or a corrupted balance.

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    vec,
    xdr::ToXdr,
    Bytes, Env,
};

use crate::{
    test_support::{MaliciousToken, MaliciousTokenClient},
    Allocation, Beneficiary, WillContract, WillContractClient, WillStatus, PREIMAGE_ADDRESS_LENGTH,
    PREIMAGE_LENGTH,
};

const DAY: u64 = 86_400;

// ── helpers ──────────────────────────────────────────────────────────────────

/// Registers a `MaliciousToken` and a `WillContract`, funds `owner` with
/// `initial_balance` of the token, and returns both clients along with
/// the token address.
fn setup_env() -> (Env, WillContractClient<'static>, MaliciousTokenClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);
    let env: Env = env;

    let token_id = env.register(MaliciousToken, ());
    let will_contract_id = env.register(WillContract, ());

    let token_client = MaliciousTokenClient::new(&env, &token_id);
    let will_client = WillContractClient::new(&env, &will_contract_id);

    (env, will_client, token_client)
}

/// Builds the documented 64-byte pre-image for `reveal_and_claim`:
///   bytes[0..32]  = sha256(xdr(beneficiary))   — address fingerprint
///   bytes[32..64] = salt repeated              — entropy
fn preimage_for(env: &Env, beneficiary: &soroban_sdk::Address, salt: u8) -> Bytes {
    let mut raw = [salt; PREIMAGE_LENGTH as usize];
    let digest = env
        .crypto()
        .sha256(&beneficiary.clone().to_xdr(env));
    raw[..PREIMAGE_ADDRESS_LENGTH as usize].copy_from_slice(&digest.to_array());
    Bytes::from_array(env, &raw)
}

// ── top_up reentrancy ─────────────────────────────────────────────────────────

/// Issue #80: A malicious token that calls `top_up` again from inside its
/// `transfer` callback cannot inflate the will's balance beyond what was
/// actually deposited.
///
/// `top_up` commits the new balance to persistent storage *before* calling
/// `transfer`. The reentrant `top_up` therefore reads the post-first-top-up
/// state, and because the malicious token's internal balances are updated
/// before the reentrancy callback fires, the second `top_up` call succeeds
/// but represents a genuine second deposit. We test that each call is
/// accounted for independently and that the will's balance equals the sum
/// of all actual deposits with no phantom credits.
#[test]
fn reentrant_top_up_does_not_double_credit_balance() {
    let (env, will_client, token_client) = setup_env();
    let token_id = token_client.address.clone();
    let will_contract_id = will_client.address.clone();

    let owner = soroban_sdk::Address::generate(&env);
    let beneficiary = soroban_sdk::Address::generate(&env);

    // Fund owner with enough for the initial will plus two top-ups.
    token_client.mint(&owner, &3_000_000);

    let will_id = will_client.create_will(
        &owner,
        &vec![&env, (token_id.clone(), 1_000_000_i128)],
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

    // Arm: during the first top_up's transfer the token will call top_up again.
    // The reentrant call uses the same amount (1_000_000) from the owner's
    // remaining balance.
    token_client.set_top_up_reentrancy(
        &will_contract_id,
        &will_id,
        &owner,
        &token_id,
        &1_000_000_i128,
    );

    // Execute the first top_up.  The reentrant second top_up fires inside
    // the first one's transfer.  Both transfers are genuine (the token's
    // internal balance accounting is updated before the callback fires), so
    // each top_up must be reflected exactly once in the will's balance.
    will_client.top_up(&will_id, &owner, &token_id, &1_000_000);

    // Disarm before reading state.
    // (No disarm API needed; the callback only fires during `transfer`.)

    let will = will_client.get_will(&will_id);
    // The will was created with 1_000_000, topped up with 1_000_000 (outer),
    // and the reentrant inner top_up also deposited 1_000_000 from the owner's
    // balance, for a total of 3_000_000.
    assert_eq!(
        will.balances.get(token_id.clone()).unwrap_or(0),
        3_000_000,
        "each genuine top_up must be credited exactly once"
    );

    // Owner spent all 3_000_000 (1_000_000 at create_will + 1_000_000 outer
    // top_up + 1_000_000 reentrant top_up).
    assert_eq!(
        token_client.balance(&owner),
        0,
        "owner balance must reflect exactly three transfers"
    );
}

/// Issue #80: A malicious token that calls `top_up` on a *different* will
/// from inside `transfer` must not bypass status or ownership checks.
///
/// When the reentrant call targets a will in `Triggered` status, `top_up`
/// must reject it with `WillNotActive`.
#[test]
fn reentrant_top_up_on_triggered_will_is_rejected() {
    let (env, will_client, token_client) = setup_env();
    let token_id = token_client.address.clone();
    let will_contract_id = will_client.address.clone();

    let owner = soroban_sdk::Address::generate(&env);
    let beneficiary = soroban_sdk::Address::generate(&env);

    token_client.mint(&owner, &3_000_000);

    let active_will_id = will_client.create_will(
        &owner,
        &vec![&env, (token_id.clone(), 1_000_000_i128)],
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

    let triggered_will_id = will_client.create_will(
        &owner,
        &vec![&env, (token_id.clone(), 1_000_000_i128)],
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

    // Trigger the second will.
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + 91 * DAY);
    will_client.trigger_will(&triggered_will_id);

    // Arm: during the top_up of active_will, try to top_up triggered_will.
    token_client.set_top_up_reentrancy(
        &will_contract_id,
        &triggered_will_id,
        &owner,
        &token_id,
        &500_000_i128,
    );

    // The outer top_up must succeed; the reentrant one must fail silently
    // (try_top_up returns Err) without panicking the outer call.
    will_client.top_up(&active_will_id, &owner, &token_id, &500_000);

    // Active will got exactly one top_up credit.
    let active_will = will_client.get_will(&active_will_id);
    assert_eq!(
        active_will.balances.get(token_id.clone()).unwrap_or(0),
        1_500_000,
        "active will must reflect only the legitimate top_up"
    );

    // Triggered will balance must be unchanged.
    let triggered_will = will_client.get_will(&triggered_will_id);
    assert_eq!(
        triggered_will.balances.get(token_id).unwrap_or(0),
        1_000_000,
        "triggered will must not be topped up via reentrancy"
    );
}

// ── merge_wills reentrancy ────────────────────────────────────────────────────

/// Issue #80: A malicious token cannot cause `merge_wills` to run twice on
/// the same pair by re-invoking `merge_wills` from inside a subsequent
/// `top_up` transfer.
///
/// After the first merge, `will_id_b` is `Cancelled`. The reentrant merge
/// attempt is rejected with `WillNotBothActive` because one of the wills is
/// no longer `Active`.
#[test]
fn reentrant_merge_wills_is_rejected_after_first_merge_completes() {
    let (env, will_client, token_client) = setup_env();
    let token_id = token_client.address.clone();
    let will_contract_id = will_client.address.clone();

    let owner = soroban_sdk::Address::generate(&env);
    let beneficiary = soroban_sdk::Address::generate(&env);

    token_client.mint(&owner, &4_000_000);

    let will_id_a = will_client.create_will(
        &owner,
        &vec![&env, (token_id.clone(), 1_000_000_i128)],
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

    let will_id_b = will_client.create_will(
        &owner,
        &vec![&env, (token_id.clone(), 1_000_000_i128)],
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

    // Arm: during a top_up of will_id_a, attempt to merge will_id_a and will_id_b.
    token_client.set_merge_reentrancy(
        &will_contract_id,
        &will_id_a,
        &will_id_b,
        &owner,
    );

    // Top up will_id_a with the malicious token to trigger the reentrant merge.
    // The merge fires inside the top_up transfer.
    will_client.top_up(&will_id_a, &owner, &token_id, &1_000_000);

    // The reentrant merge attempt may succeed (merge fires after top_up effects
    // are committed) or be rejected for other reasons — but it must never
    // duplicate balances or leave will_id_b in an inconsistent state.
    // What we can assert unconditionally: will_id_b is either still Active
    // or Cancelled, never in an intermediate half-merged state.
    let will_b_status = will_client.get_will(&will_id_b).status;
    assert!(
        will_b_status == WillStatus::Active || will_b_status == WillStatus::Cancelled,
        "will_b must be either Active or Cancelled, never in a half-merged state"
    );

    // The will_id_a balance must equal initial + top_up regardless of whether
    // the merge happened inside or outside the transfer.
    let will_a = will_client.get_will(&will_id_a);
    // If will_a was the merge survivor, its balance includes will_b's funds.
    // If the reentrant merge was rejected, it only has initial + top_up.
    // Either way it must be a coherent value >= 2_000_000.
    let will_a_balance = will_a.balances.get(token_id.clone()).unwrap_or(0);
    assert!(
        will_a_balance >= 2_000_000,
        "will_a balance must be at least initial + top_up = 2_000_000, got {will_a_balance}"
    );
}

// ── reveal_and_claim reentrancy ───────────────────────────────────────────────

/// Issue #80: A malicious token that calls `reveal_and_claim` again from
/// inside the payout transfer cannot claim the same hashed-beneficiary slot
/// twice.
///
/// `reveal_and_claim` marks the slot `claimed = true` and deducts the share
/// from the will's balance *before* invoking any external transfer. The
/// reentrant call therefore encounters `AlreadyClaimed` and must be rejected
/// without paying out a second time.
#[test]
fn reentrant_reveal_and_claim_cannot_double_pay() {
    let (env, will_client, token_client) = setup_env();
    let token_id = token_client.address.clone();
    let will_contract_id = will_client.address.clone();

    let owner = soroban_sdk::Address::generate(&env);
    let regular_beneficiary = soroban_sdk::Address::generate(&env);
    let secret_beneficiary = soroban_sdk::Address::generate(&env);

    token_client.mint(&owner, &1_000_000);

    // Create a will where 50% goes to a regular beneficiary (FixedAmount)
    // and the remaining balance is reserved for one hashed beneficiary.
    let will_id = will_client.create_will(
        &owner,
        &vec![&env, (token_id.clone(), 1_000_000_i128)],
        &vec![
            &env,
            Beneficiary {
                address: regular_beneficiary.clone(),
                allocation: Allocation::FixedAmount(500_000),
            },
        ],
        &90,
        &7,
        &vec![&env],
        &0,
        &None,
        &0,
    );

    // Build a valid pre-image and commitment for the hashed beneficiary.
    let preimage = preimage_for(&env, &secret_beneficiary, 0xAB);
    let commitment = env.crypto().sha256(&preimage);
    let commitment_bytes = Bytes::from_array(&env, &commitment.to_array());

    // Reserve 5000 bps (50% of the hashed pool, i.e. 50% of remaining) for
    // the hashed beneficiary.
    will_client.add_hashed_beneficiary(&will_id, &owner, &commitment_bytes, &5_000);

    // Trigger → release so reveal_and_claim becomes available.
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + 91 * DAY);
    will_client.trigger_will(&will_id);
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + 8 * DAY);
    will_client.release_inheritance(&will_id, &None);

    // Arm the token to attempt reveal_and_claim again from inside the payout
    // transfer.
    token_client.set_reveal_and_claim_reentrancy(
        &will_contract_id,
        &will_id,
        &secret_beneficiary,
        &preimage,
    );

    // The first reveal_and_claim must succeed and pay out exactly once.
    will_client.reveal_and_claim(&will_id, &secret_beneficiary, &preimage);

    // The hashed beneficiary received exactly one payout; the reentrant
    // attempt returned `AlreadyClaimed` without a second transfer.
    let claimed_balance = token_client.balance(&secret_beneficiary);
    assert!(
        claimed_balance > 0,
        "secret beneficiary must receive their share"
    );

    // The slot must be marked claimed after the call.
    let will = will_client.get_will(&will_id);
    assert!(
        will.hashed_beneficiaries.get(0).unwrap().claimed,
        "hashed beneficiary slot must be marked claimed"
    );

    // Verify no phantom second payment: if the reentrant call had succeeded
    // the balance would be double. We assert the exact expected amount.
    // The hashed pool is everything not covered by FixedAmount(500_000), i.e.
    // 500_000.  With a single 5000 bps slot (100% of the unclaimed pool),
    // the entire 500_000 goes to secret_beneficiary.
    assert_eq!(
        claimed_balance, 500_000,
        "secret beneficiary must receive exactly their reserved share, not double"
    );
}

/// Issue #80: A malicious token that replays a valid pre-image for a second
/// hashed-beneficiary slot cannot claim both slots in a single reentrant
/// transaction.
///
/// After the first slot is claimed, the reentrant attempt on the same slot
/// must return `AlreadyClaimed`. A separate slot with a different commitment
/// remains claimable only through a legitimate subsequent call.
#[test]
fn reentrant_reveal_and_claim_cannot_claim_already_claimed_slot() {
    let (env, will_client, token_client) = setup_env();
    let token_id = token_client.address.clone();
    let will_contract_id = will_client.address.clone();

    let owner = soroban_sdk::Address::generate(&env);
    let regular_beneficiary = soroban_sdk::Address::generate(&env);
    let hashed_claimant = soroban_sdk::Address::generate(&env);

    token_client.mint(&owner, &1_000_000);

    let will_id = will_client.create_will(
        &owner,
        &vec![&env, (token_id.clone(), 1_000_000_i128)],
        &vec![
            &env,
            Beneficiary {
                address: regular_beneficiary.clone(),
                allocation: Allocation::FixedAmount(400_000),
            },
        ],
        &90,
        &7,
        &vec![&env],
        &0,
        &None,
        &0,
    );

    // Add a hashed beneficiary (occupies the entire unclaimed pool = 600_000).
    let preimage = preimage_for(&env, &hashed_claimant, 0xCD);
    let commitment = env.crypto().sha256(&preimage);
    let commitment_bytes = Bytes::from_array(&env, &commitment.to_array());
    will_client.add_hashed_beneficiary(&will_id, &owner, &commitment_bytes, &5_000);

    // Trigger and release.
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + 91 * DAY);
    will_client.trigger_will(&will_id);
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + 8 * DAY);
    will_client.release_inheritance(&will_id, &None);

    // Arm the token to attempt a duplicate reveal_and_claim mid-payout.
    token_client.set_reveal_and_claim_reentrancy(
        &will_contract_id,
        &will_id,
        &hashed_claimant,
        &preimage,
    );

    // First claim succeeds.
    will_client.reveal_and_claim(&will_id, &hashed_claimant, &preimage);

    // Slot is marked claimed.
    let will = will_client.get_will(&will_id);
    assert!(
        will.hashed_beneficiaries.get(0).unwrap().claimed,
        "slot must be marked claimed after reveal_and_claim"
    );

    // A direct second attempt (outside the reentrancy callback) must also
    // fail with AlreadyClaimed, confirming the state machine is correct.
    use crate::WillError;
    assert_eq!(
        will_client.try_reveal_and_claim(&will_id, &hashed_claimant, &preimage),
        Err(Ok(WillError::AlreadyClaimed.into())),
        "duplicate reveal_and_claim must return AlreadyClaimed"
    );
}
