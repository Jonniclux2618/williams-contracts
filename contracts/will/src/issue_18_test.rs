#![cfg(test)]

//! Regression tests for issue #18: `reveal_and_claim` executed token
//! transfers directly, so a single failing transfer panicked the whole
//! claim — no failed-payout record was created and a multi-token hashed
//! beneficiary could not receive *any* of their assets. Failures now flow
//! through the same `pay_or_record_failure` ledger `distribute` uses (#459),
//! so the claim commits and each token's share is retried individually.

use soroban_sdk::{
    contract, contractimpl, contracttype,
    testutils::{Address as _, Ledger},
    vec,
    xdr::ToXdr,
    Address, Bytes, Env, Vec as SorobanVec,
};

use crate::{storage, Allocation, Beneficiary, WillContract, WillContractClient, PREIMAGE_LENGTH};

const DAY: u64 = 86_400;

#[contracttype]
#[derive(Clone)]
enum TokenDataKey {
    Balance(Address),
    FailAll,
}

/// A minimal SEP-41-shaped token whose transfers can be switched to fail
/// wholesale, simulating a paused token contract or a destination the token
/// refuses to pay.
#[contract]
pub struct FailableToken;

#[contractimpl]
impl FailableToken {
    pub fn mint(env: Env, to: Address, amount: i128) {
        let key = TokenDataKey::Balance(to.clone());
        let balance: i128 = env.storage().instance().get(&key).unwrap_or(0);
        env.storage().instance().set(&key, &(balance + amount));
    }

    pub fn balance(env: Env, id: Address) -> i128 {
        env.storage()
            .instance()
            .get(&TokenDataKey::Balance(id))
            .unwrap_or(0)
    }

    /// `create_will` and `batch_create_wills` probe every token with a
    /// read-only `decimals()` call, so this mock must answer it like a real
    /// SEP-41 token to be usable as a will's locked asset.
    pub fn decimals(_env: Env) -> u32 {
        7
    }

    /// Makes every subsequent `transfer` panic, like a paused token or a
    /// frozen recipient.
    pub fn set_fail_all(env: Env, fail: bool) {
        env.storage().instance().set(&TokenDataKey::FailAll, &fail);
    }

    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        let fail: bool = env
            .storage()
            .instance()
            .get(&TokenDataKey::FailAll)
            .unwrap_or(false);
        if fail {
            panic!("token transfers are disabled");
        }
        let from_key = TokenDataKey::Balance(from.clone());
        let from_balance: i128 = env.storage().instance().get(&from_key).unwrap_or(0);
        env.storage()
            .instance()
            .set(&from_key, &(from_balance - amount));

        let to_key = TokenDataKey::Balance(to.clone());
        let to_balance: i128 = env.storage().instance().get(&to_key).unwrap_or(0);
        env.storage()
            .instance()
            .set(&to_key, &(to_balance + amount));
    }
}

fn setup<'a>() -> (
    Env,
    WillContractClient<'a>,
    FailableTokenClient<'a>,
    Address,
    Address,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);

    let owner = Address::generate(&env);
    let token_id = env.register(FailableToken, ());
    let token_client = FailableTokenClient::new(&env, &token_id);
    token_client.mint(&owner, &1_000_000);

    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    (
        env.clone(),
        client,
        token_client,
        owner,
        token_id.clone(),
        contract_id,
    )
}

fn advance(env: &Env, days: u64) {
    env.ledger().with_mut(|l| l.timestamp += days * DAY);
}

/// Builds the 64-byte pre-image (`sha256(xdr(claimant)) || salt`) and its
/// SHA-256 commitment for a hashed beneficiary (#369's binding rule).
fn preimage_and_commitment(env: &Env, claimant: &Address) -> (Bytes, Bytes) {
    let mut preimage_bytes = [0xA5u8; PREIMAGE_LENGTH as usize];
    let digest = env.crypto().sha256(&claimant.clone().to_xdr(env));
    preimage_bytes[..32].copy_from_slice(&digest.to_array());

    let preimage = Bytes::from_array(env, &preimage_bytes);
    let commitment = env.crypto().sha256(&preimage.clone());
    let commitment_bytes = Bytes::from_array(env, &commitment.to_array());
    (preimage, commitment_bytes)
}

/// Creates a will whose visible beneficiary claims a fixed 500_000 and
/// whose hashed beneficiary is committed to the remaining 5000 bps.
fn create_will_with_hashed(
    env: &Env,
    client: &WillContractClient,
    owner: &Address,
    tokens: &SorobanVec<(Address, i128)>,
    commitment: &Bytes,
) -> u64 {
    let visible = Address::generate(env);
    let will_id = client.create_will(
        owner,
        tokens,
        &vec![
            env,
            Beneficiary {
                address: visible,
                allocation: Allocation::FixedAmount(500_000),
            },
        ],
        &90,
        &7,
        &vec![env],
        &0,
        &None,
        &0,
    );
    client.add_hashed_beneficiary(&will_id, owner, commitment, &5_000);
    will_id
}

/// Issue #18: a failing token transfer during `reveal_and_claim` records a
/// failed payout instead of panicking the whole claim, and the claimant can
/// deliver it afterwards with `retry_failed_payout`.
#[test]
fn reveal_and_claim_records_a_failed_transfer_and_allows_a_retry() {
    let (env, client, token_client, owner, token_address, contract_id) = setup();
    let claimant = Address::generate(&env);

    let (preimage, commitment) = preimage_and_commitment(&env, &claimant);
    let tokens: SorobanVec<(Address, i128)> = vec![&env, (token_address.clone(), 1_000_000_i128)];
    let will_id = create_will_with_hashed(&env, &client, &owner, &tokens, &commitment);

    // Release the will; the visible beneficiary is paid and the hashed
    // share (500_000) is withheld in the will's balances.
    advance(&env, 91);
    client.trigger_will(&will_id);
    advance(&env, 8);
    client.release_inheritance(&will_id, &None);

    // The token starts refusing transfers before the hashed claimant reveals.
    token_client.set_fail_all(&true);

    let result = client.try_reveal_and_claim(&will_id, &claimant, &preimage);
    assert!(
        result.is_ok(),
        "a failing transfer must not abort the claim, got {result:?}"
    );

    // The claim itself committed: the slot is marked claimed and the share
    // is recorded for retry rather than lost.
    let will = client.get_will(&will_id);
    assert!(will.hashed_beneficiaries.get_unchecked(0).claimed);
    assert_eq!(token_client.balance(&claimant), 0);
    assert_eq!(
        env.as_contract(&contract_id, || storage::get_failed_payout(
            &env,
            will_id,
            &token_address,
            &claimant
        )),
        Some(500_000),
        "the failed share must be recorded for retry_failed_payout"
    );

    // Once transfers work again the claimant retrieves the payout.
    token_client.set_fail_all(&false);
    client.retry_failed_payout(&will_id, &token_address, &claimant);
    assert_eq!(token_client.balance(&claimant), 500_000);
    assert_eq!(
        env.as_contract(&contract_id, || storage::get_failed_payout(
            &env,
            will_id,
            &token_address,
            &claimant
        )),
        None,
        "a delivered retry must clear the record"
    );
}

/// Issue #18: on a multi-token will, one token failing must not block the
/// claimant's other token shares — each failing token is recorded
/// individually while the healthy token pays out in the same call.
#[test]
fn one_failing_token_does_not_block_the_other_tokens_share() {
    let (env, client, token_client, owner, token_address, contract_id) = setup();
    let claimant = Address::generate(&env);

    // A second failable token: the will's non-primary token.
    let token_b_id = env.register(FailableToken, ());
    let token_b_client = FailableTokenClient::new(&env, &token_b_id);
    token_b_client.mint(&owner, &2_000_000);

    let (preimage, commitment) = preimage_and_commitment(&env, &claimant);
    let tokens: SorobanVec<(Address, i128)> = vec![
        &env,
        (token_address.clone(), 1_000_000_i128),
        (token_b_id.clone(), 2_000_000_i128),
    ];
    let will_id = create_will_with_hashed(&env, &client, &owner, &tokens, &commitment);

    advance(&env, 91);
    client.trigger_will(&will_id);
    advance(&env, 8);
    client.release_inheritance(&will_id, &None);

    // Only the secondary token starts failing; the primary stays healthy.
    token_b_client.set_fail_all(&true);

    let result = client.try_reveal_and_claim(&will_id, &claimant, &preimage);
    assert!(
        result.is_ok(),
        "one failing token must not abort the multi-token claim, got {result:?}"
    );

    // The healthy token's share was delivered in the same call.
    assert_eq!(
        token_client.balance(&claimant),
        500_000,
        "the primary token's share must be paid despite the secondary failing"
    );
    // The failing token's share is recorded for retry instead.
    assert_eq!(token_b_client.balance(&claimant), 0);
    assert_eq!(
        env.as_contract(&contract_id, || storage::get_failed_payout(
            &env,
            will_id,
            &token_b_id,
            &claimant
        )),
        Some(1_000_000),
        "the secondary token's share must be recorded for retry_failed_payout"
    );

    token_b_client.set_fail_all(&false);
    client.retry_failed_payout(&will_id, &token_b_id, &claimant);
    assert_eq!(token_b_client.balance(&claimant), 1_000_000);
}
