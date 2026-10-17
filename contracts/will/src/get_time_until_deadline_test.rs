#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::StellarAssetClient,
    vec, Address, Env, Vec as SorobanVec,
};

use crate::{Allocation, Beneficiary, WillContract, WillContractClient};

const DAY: u64 = 86_400;

fn setup<'a>() -> (Env, WillContractClient<'a>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);

    let owner = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(owner.clone());
    let token_address = sac.address();
    StellarAssetClient::new(&env, &token_address).mint(&owner, &1_000_000_000);

    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    (env, client, owner, token_address)
}

fn create_active_will(
    env: &Env,
    client: &WillContractClient,
    owner: &Address,
    token_address: &Address,
    checkin_period_days: u64,
    grace_period_days: u64,
) -> u64 {
    let beneficiary = Address::generate(env);

    let beneficiaries: SorobanVec<Beneficiary> = vec![
        env,
        Beneficiary {
            address: beneficiary,
            allocation: Allocation::Percentage(10_000),
        },
    ];
    let tokens: SorobanVec<(Address, i128)> = vec![env, (token_address.clone(), 1_000_000_i128)];

    // confirmation_delay_seconds is 0 above, so the will starts Active
    // immediately -- no confirm_will call is needed (or valid).
    client.create_will(
        owner,
        &tokens,
        &beneficiaries,
        &checkin_period_days,
        &grace_period_days,
        &vec![env],
        &1,
        &None,
        &0,
    )
}

#[test]
fn test_get_time_until_deadline_active_will_positive_time_remaining() {
    let (env, client, owner, token_address) = setup();

    let checkin_period_days = 30;
    let will_id = create_active_will(
        &env,
        &client,
        &owner,
        &token_address,
        checkin_period_days,
        7,
    );

    // Will was just created, so check-in deadline is ~30 days away
    let time_until_deadline = client.get_time_until_deadline(&will_id);
    assert!(time_until_deadline.is_some());
    let seconds_remaining = time_until_deadline.unwrap();

    // Should be approximately 30 days in seconds
    let expected_approx = (checkin_period_days * DAY) as i64;
    let diff = (seconds_remaining - expected_approx).abs();
    assert!(
        diff < 100,
        "Deadline should be ~30 days away, got {} seconds",
        seconds_remaining
    );
}

#[test]
fn test_get_time_until_deadline_active_will_clamped_after_missed_deadline() {
    let (env, client, owner, token_address) = setup();

    let checkin_period_days = 30;
    let will_id = create_active_will(
        &env,
        &client,
        &owner,
        &token_address,
        checkin_period_days,
        7,
    );

    // Advance time past the check-in deadline but don't trigger yet
    env.ledger().with_mut(|l| l.timestamp += 31 * DAY);

    let time_until_deadline = client.get_time_until_deadline(&will_id);
    assert!(time_until_deadline.is_some());
    let seconds_remaining = time_until_deadline.unwrap();

    // Overdue deadlines are clamped to zero (never negative)
    assert_eq!(
        seconds_remaining, 0,
        "Overdue deadline should be clamped to 0, got {} seconds",
        seconds_remaining
    );
}

#[test]
fn test_get_time_until_deadline_active_will_zero_at_exact_deadline() {
    let (env, client, owner, token_address) = setup();

    let checkin_period_days = 30;
    let will_id = create_active_will(
        &env,
        &client,
        &owner,
        &token_address,
        checkin_period_days,
        7,
    );

    // Advance exactly to the check-in deadline (now == deadline)
    env.ledger().with_mut(|l| l.timestamp += checkin_period_days * DAY);

    assert_eq!(
        client.get_time_until_deadline(&will_id),
        Some(0),
        "At the exact deadline the remaining time should be 0"
    );
}

#[test]
fn test_get_time_until_deadline_active_will_zero_ten_days_past_deadline() {
    let (env, client, owner, token_address) = setup();

    let checkin_period_days = 30;
    let will_id = create_active_will(
        &env,
        &client,
        &owner,
        &token_address,
        checkin_period_days,
        7,
    );

    // Advance 10 days past the check-in deadline (now > deadline)
    env.ledger()
        .with_mut(|l| l.timestamp += (checkin_period_days + 10) * DAY);

    assert_eq!(
        client.get_time_until_deadline(&will_id),
        Some(0),
        "Ten days past the deadline should be clamped to 0"
    );
}

#[test]
fn test_get_time_until_deadline_triggered_will_counts_grace_period() {
    let (env, client, owner, token_address) = setup();

    let checkin_period_days = 30;
    let grace_period_days = 7;
    let will_id = create_active_will(
        &env,
        &client,
        &owner,
        &token_address,
        checkin_period_days,
        grace_period_days,
    );

    // Advance to check-in deadline
    env.ledger().with_mut(|l| l.timestamp += 30 * DAY);

    // Trigger the will
    client.trigger_will(&will_id);

    let time_until_deadline = client.get_time_until_deadline(&will_id);
    assert!(time_until_deadline.is_some());
    let seconds_remaining = time_until_deadline.unwrap();

    // Should be approximately 7 days in seconds
    let expected_approx = (grace_period_days * DAY) as i64;
    let diff = (seconds_remaining - expected_approx).abs();
    assert!(
        diff < 100,
        "Grace period deadline should be ~7 days away, got {} seconds",
        seconds_remaining
    );
}

#[test]
fn test_get_time_until_deadline_triggered_will_clamped_after_grace_expires() {
    let (env, client, owner, token_address) = setup();

    let checkin_period_days = 30;
    let grace_period_days = 7;
    let will_id = create_active_will(
        &env,
        &client,
        &owner,
        &token_address,
        checkin_period_days,
        grace_period_days,
    );

    // Advance to check-in deadline
    env.ledger().with_mut(|l| l.timestamp += 30 * DAY);

    // Trigger the will
    client.trigger_will(&will_id);

    // Advance past grace period
    env.ledger().with_mut(|l| l.timestamp += 8 * DAY);

    let time_until_deadline = client.get_time_until_deadline(&will_id);
    assert!(time_until_deadline.is_some());
    let seconds_remaining = time_until_deadline.unwrap();

    // Expired grace period is clamped to zero (never negative)
    assert_eq!(
        seconds_remaining, 0,
        "Expired grace period should be clamped to 0, got {} seconds",
        seconds_remaining
    );
}

#[test]
fn test_get_time_until_deadline_pending_confirmation() {
    let (env, client, owner, token_address) = setup();

    let beneficiary = Address::generate(&env);
    let beneficiaries: SorobanVec<Beneficiary> = vec![
        &env,
        Beneficiary {
            address: beneficiary,
            allocation: Allocation::Percentage(10_000),
        },
    ];
    let tokens: SorobanVec<(Address, i128)> = vec![&env, (token_address, 1_000_000_i128)];

    let will_id = client.create_will(
        &owner,
        &tokens,
        &beneficiaries,
        &30,
        &7,
        &vec![&env],
        &0,
        &None,
        &3600,
    );

    // Will is in PendingConfirmation status: deadline is 3600 seconds away
    let time_until_deadline = client.get_time_until_deadline(&will_id);
    assert_eq!(
        time_until_deadline,
        Some(3600),
        "PendingConfirmation status should return remaining seconds until confirmation deadline"
    );

    // Advance 1800 seconds
    env.ledger().with_mut(|l| l.timestamp += 1800);
    assert_eq!(
        client.get_time_until_deadline(&will_id),
        Some(1800)
    );

    // Advance past the confirmation deadline (another 2000 seconds -> 200 seconds past due)
    env.ledger().with_mut(|l| l.timestamp += 2000);
    assert_eq!(
        client.get_time_until_deadline(&will_id),
        Some(0),
        "Lapsed confirmation deadline should be clamped to 0"
    );
}

#[test]
fn test_get_time_until_deadline_released_returns_none() {
    let (env, client, owner, token_address) = setup();

    let checkin_period_days = 30;
    let grace_period_days = 7;
    let will_id = create_active_will(
        &env,
        &client,
        &owner,
        &token_address,
        checkin_period_days,
        grace_period_days,
    );

    // Advance past check-in deadline
    env.ledger().with_mut(|l| l.timestamp += 31 * DAY);

    // Trigger and release
    client.trigger_will(&will_id);
    env.ledger().with_mut(|l| l.timestamp += 8 * DAY);
    client.release_inheritance(&will_id, &None);

    let time_until_deadline = client.get_time_until_deadline(&will_id);
    assert!(
        time_until_deadline.is_none(),
        "Released status should return None"
    );
}

#[test]
fn test_get_time_until_deadline_cancelled_returns_none() {
    let (env, client, owner, token_address) = setup();

    let checkin_period_days = 30;
    let will_id = create_active_will(
        &env,
        &client,
        &owner,
        &token_address,
        checkin_period_days,
        7,
    );

    // Cancel the will
    client.cancel_will(&will_id);

    let time_until_deadline = client.get_time_until_deadline(&will_id);
    assert!(
        time_until_deadline.is_none(),
        "Cancelled status should return None"
    );
}
