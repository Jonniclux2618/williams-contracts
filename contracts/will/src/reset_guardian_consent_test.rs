#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::StellarAssetClient,
    vec, Address, Env, Vec as SorobanVec,
};

use crate::{
    Allocation, Beneficiary, GuardianConsent, GuardianVoteReason, WillContract, WillContractClient,
    WillError, WillStatus,
};

const DAY: u64 = 86_400;

fn setup<'a>() -> (
    Env,
    WillContractClient<'a>,
    Address,
    Address,
    Address,
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

    let guardian_a = Address::generate(&env);
    let guardian_b = Address::generate(&env);

    (env, client, owner, token_address, guardian_a, guardian_b)
}

fn create_will_with_guardians(
    env: &Env,
    client: &WillContractClient,
    owner: &Address,
    token_address: &Address,
    guardians: &SorobanVec<Address>,
    threshold: u32,
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

    client.create_will(
        owner,
        &tokens,
        &beneficiaries,
        &30,
        &7,
        guardians,
        &threshold,
        &None,
        &0,
    )
}

#[test]
fn test_reacceptance_flow_after_rejection() {
    let (env, client, owner, token_address, guardian_a, guardian_b) = setup();

    let guardians: SorobanVec<Address> = vec![&env, guardian_a.clone(), guardian_b.clone()];
    let will_id = create_will_with_guardians(&env, &client, &owner, &token_address, &guardians, 2);

    let will = client.get_will(&will_id);
    let initial_updated_at = will.guardian_list_updated_at;

    // Guardian A accepts
    client.accept_guardian_role(&will_id, &guardian_a);
    let will = client.get_will(&will_id);
    let ga = will
        .guardians
        .iter()
        .find(|g| g.address == guardian_a)
        .unwrap();
    assert_eq!(ga.consent, GuardianConsent::Accepted);

    // Guardian B accidentally rejects
    client.reject_guardian_role(&will_id, &guardian_b);
    let will = client.get_will(&will_id);
    let gb = will
        .guardians
        .iter()
        .find(|g| g.address == guardian_b)
        .unwrap();
    assert_eq!(gb.consent, GuardianConsent::Rejected);

    // Guardian B cannot directly re-accept: rejected is normally terminal
    let err = client.try_accept_guardian_role(&will_id, &guardian_b);
    assert_eq!(err, Err(Ok(WillError::InvalidConsentTransition.into())));

    // Advance time by 3 days
    env.ledger().with_mut(|l| l.timestamp += 3 * DAY);

    // Owner resets guardian B's consent back to Pending (#36)
    client.reset_guardian_consent(&will_id, &guardian_b);

    // Verify guardian B is now Pending and guardian A is still Accepted
    let will = client.get_will(&will_id);
    let ga = will
        .guardians
        .iter()
        .find(|g| g.address == guardian_a)
        .unwrap();
    assert_eq!(
        ga.consent,
        GuardianConsent::Accepted,
        "Existing accepted guardians must retain consent"
    );
    let gb = will
        .guardians
        .iter()
        .find(|g| g.address == guardian_b)
        .unwrap();
    assert_eq!(
        gb.consent,
        GuardianConsent::Pending,
        "Rejected guardian must transition to Pending"
    );

    // Cooldown timestamp was not modified
    assert_eq!(
        will.guardian_list_updated_at, initial_updated_at,
        "Cooldown timestamp must not be restarted by reset_guardian_consent"
    );

    // Now Guardian B can successfully accept the role
    client.accept_guardian_role(&will_id, &guardian_b);
    let will = client.get_will(&will_id);
    let gb = will
        .guardians
        .iter()
        .find(|g| g.address == guardian_b)
        .unwrap();
    assert_eq!(gb.consent, GuardianConsent::Accepted);

    // Advance past cooldown
    env.ledger().with_mut(|l| l.timestamp += 35 * DAY);

    // Both guardians vote to trigger early release
    client.guardian_trigger(&will_id, &guardian_a, &GuardianVoteReason::Other);
    client.guardian_trigger(&will_id, &guardian_b, &GuardianVoteReason::Other);

    // Quorum reached -> Released
    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);
}

#[test]
fn test_reset_guardian_consent_not_guardian_panics() {
    let (env, client, owner, token_address, guardian_a, _guardian_b) = setup();

    let guardians: SorobanVec<Address> = vec![&env, guardian_a.clone()];
    let will_id = create_will_with_guardians(&env, &client, &owner, &token_address, &guardians, 1);

    let stranger = Address::generate(&env);
    let err = client.try_reset_guardian_consent(&will_id, &stranger);
    assert_eq!(err, Err(Ok(WillError::NotGuardian.into())));
}

#[test]
fn test_reset_guardian_consent_terminal_will_panics() {
    let (env, client, owner, token_address, guardian_a, _guardian_b) = setup();

    let guardians: SorobanVec<Address> = vec![&env, guardian_a.clone()];
    let will_id = create_will_with_guardians(&env, &client, &owner, &token_address, &guardians, 1);

    // Cancel the will
    client.cancel_will(&will_id, &owner);

    let err = client.try_reset_guardian_consent(&will_id, &guardian_a);
    assert_eq!(err, Err(Ok(WillError::WillNotActive.into())));
}

#[test]
fn test_reset_guardian_consent_noop_when_not_rejected() {
    let (env, client, owner, token_address, guardian_a, _guardian_b) = setup();

    let guardians: SorobanVec<Address> = vec![&env, guardian_a.clone()];
    let will_id = create_will_with_guardians(&env, &client, &owner, &token_address, &guardians, 1);

    // Guardian is Pending: reset_guardian_consent is a no-op
    client.reset_guardian_consent(&will_id, &guardian_a);
    let will = client.get_will(&will_id);
    assert_eq!(
        will.guardians.get(0).unwrap().consent,
        GuardianConsent::Pending
    );

    // Guardian Accepts: reset_guardian_consent is a no-op (leaves Accepted intact)
    client.accept_guardian_role(&will_id, &guardian_a);
    client.reset_guardian_consent(&will_id, &guardian_a);
    let will = client.get_will(&will_id);
    assert_eq!(
        will.guardians.get(0).unwrap().consent,
        GuardianConsent::Accepted
    );
}
