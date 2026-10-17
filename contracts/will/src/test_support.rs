//! Test-only malicious/reentrant SEP-41 token harness.
//!
//! This module exists specifically for reentrancy regression testing: it
//! implements just enough of the token interface (`mint`, `balance`,
//! `transfer`) to fund and pay out a will, and can be configured to call
//! back into an arbitrary `WillContract` entrypoint from inside `transfer`.
//! It is used to prove that the checks-effects-interactions ordering fixed
//! in issue #54 prevents a reentrant token from triggering a double payout
//! during `release_inheritance`.
//!
//! Extended in issue #80 to support configurable reentrancy callbacks for
//! `split_will`, `top_up`, `merge_wills`, and `reveal_and_claim`.
#![cfg(test)]

use soroban_sdk::{contract, contractimpl, contracttype, Address, Bytes, Env};

use crate::WillContractClient;

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Balance(Address),
    ReentrancyTarget,
}

/// Which will-contract entrypoint to call back into during `transfer`.
#[contracttype]
#[derive(Clone)]
pub enum ReentrancyCallback {
    /// Attempt `release_inheritance(will_id)` — the original single-entrypoint
    /// test from issue #54.
    ReleaseInheritance,
    /// Attempt a second `top_up(will_id, owner, token, amount)`.
    TopUp,
    /// Attempt `merge_wills(owner, will_id_a, will_id_b)`.
    MergeWills,
    /// Attempt a second `reveal_and_claim(will_id, claimant, preimage)`.
    RevealAndClaim,
}

#[contracttype]
#[derive(Clone)]
struct ReentrancyTarget {
    will_contract: Address,
    will_id: u64,
    /// Secondary will id used by MergeWills callback (will_id_b).
    will_id_b: u64,
    owner: Address,
    token: Address,
    amount: i128,
    claimant: Address,
    preimage: Bytes,
    callback: ReentrancyCallback,
}

#[contract]
pub struct MaliciousToken;

#[contractimpl]
impl MaliciousToken {
    pub fn mint(env: Env, to: Address, amount: i128) {
        let key = DataKey::Balance(to);
        let balance: i128 = env.storage().instance().get(&key).unwrap_or(0);
        env.storage().instance().set(&key, &(balance + amount));
    }

    pub fn balance(env: Env, id: Address) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::Balance(id))
            .unwrap_or(0)
    }

    /// `create_will` probes every token with a read-only `decimals()` call
    /// before transferring, so this mock must answer it like a real SEP-41
    /// token to be usable as a will's locked asset.
    pub fn decimals(_env: Env) -> u32 {
        7
    }

    /// Configures this token to attempt `release_inheritance(will_id)` on
    /// `will_contract` every time `transfer` is invoked, simulating a
    /// malicious or reentrant token contract.
    ///
    /// This is the original single-entrypoint helper kept for backward
    /// compatibility with the existing `test_reentrant_release_inheritance_cannot_double_pay`
    /// test.
    pub fn set_reentrancy_target(env: Env, will_contract: Address, will_id: u64) {
        let dummy = Address::generate(&env);
        env.storage().instance().set(
            &DataKey::ReentrancyTarget,
            &ReentrancyTarget {
                will_contract,
                will_id,
                will_id_b: 0,
                owner: dummy.clone(),
                token: dummy.clone(),
                amount: 0,
                claimant: dummy,
                preimage: Bytes::new(&env),
                callback: ReentrancyCallback::ReleaseInheritance,
            },
        );
    }

    /// Configures the token to attempt a second `top_up` on `will_contract`
    /// from inside `transfer`, simulating a reentrant top-up attack.
    #[allow(clippy::too_many_arguments)]
    pub fn set_top_up_reentrancy(
        env: Env,
        will_contract: Address,
        will_id: u64,
        owner: Address,
        token: Address,
        amount: i128,
    ) {
        let dummy = Address::generate(&env);
        env.storage().instance().set(
            &DataKey::ReentrancyTarget,
            &ReentrancyTarget {
                will_contract,
                will_id,
                will_id_b: 0,
                owner,
                token,
                amount,
                claimant: dummy.clone(),
                preimage: Bytes::new(&env),
                callback: ReentrancyCallback::TopUp,
            },
        );
    }

    /// Configures the token to attempt `merge_wills(owner, will_id_a, will_id_b)`
    /// on `will_contract` from inside `transfer`.
    pub fn set_merge_reentrancy(
        env: Env,
        will_contract: Address,
        will_id_a: u64,
        will_id_b: u64,
        owner: Address,
    ) {
        let dummy = Address::generate(&env);
        env.storage().instance().set(
            &DataKey::ReentrancyTarget,
            &ReentrancyTarget {
                will_contract,
                will_id: will_id_a,
                will_id_b,
                owner,
                token: dummy.clone(),
                amount: 0,
                claimant: dummy,
                preimage: Bytes::new(&env),
                callback: ReentrancyCallback::MergeWills,
            },
        );
    }

    /// Configures the token to attempt a second `reveal_and_claim` on
    /// `will_contract` from inside `transfer`.
    pub fn set_reveal_and_claim_reentrancy(
        env: Env,
        will_contract: Address,
        will_id: u64,
        claimant: Address,
        preimage: Bytes,
    ) {
        let dummy = Address::generate(&env);
        env.storage().instance().set(
            &DataKey::ReentrancyTarget,
            &ReentrancyTarget {
                will_contract,
                will_id,
                will_id_b: 0,
                owner: dummy.clone(),
                token: dummy,
                amount: 0,
                claimant,
                preimage,
                callback: ReentrancyCallback::RevealAndClaim,
            },
        );
    }

    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        let from_key = DataKey::Balance(from);
        let from_balance: i128 = env.storage().instance().get(&from_key).unwrap_or(0);
        env.storage()
            .instance()
            .set(&from_key, &(from_balance - amount));

        let to_key = DataKey::Balance(to);
        let to_balance: i128 = env.storage().instance().get(&to_key).unwrap_or(0);
        env.storage()
            .instance()
            .set(&to_key, &(to_balance + amount));

        if let Some(target) = env
            .storage()
            .instance()
            .get::<_, ReentrancyTarget>(&DataKey::ReentrancyTarget)
        {
            let will_client = WillContractClient::new(&env, &target.will_contract);
            match target.callback {
                ReentrancyCallback::ReleaseInheritance => {
                    // Attempt to reenter mid-transfer. `distribute` commits
                    // `will.status = Released` before this transfer fires, so a
                    // second attempt is rejected with `WillNotTriggered` instead of
                    // paying out twice.
                    let _ = will_client.try_release_inheritance(&target.will_id, &None);
                }
                ReentrancyCallback::TopUp => {
                    // Attempt a second top_up inside the first top_up's transfer.
                    // The will's balance is already updated before the external
                    // transfer fires (checks-effects-interactions), so the reentrant
                    // call sees the post-first-top-up balance and must not corrupt it.
                    let _ = will_client.try_top_up(
                        &target.will_id,
                        &target.owner,
                        &target.token,
                        &target.amount,
                    );
                }
                ReentrancyCallback::MergeWills => {
                    // Attempt to merge mid-transfer. Effects are already committed
                    // before the external transfer fires, so the merge attempt should
                    // either succeed on a still-valid state or be rejected outright
                    // by status/ownership checks, never leaving state inconsistent.
                    let _ = will_client.try_merge_wills(
                        &target.owner,
                        &target.will_id,
                        &target.will_id_b,
                    );
                }
                ReentrancyCallback::RevealAndClaim => {
                    // Attempt a second reveal_and_claim mid-transfer. The slot is
                    // already marked claimed and the balance already deducted before
                    // the transfer fires, so the reentrant attempt must fail with
                    // `AlreadyClaimed` rather than paying out twice.
                    let _ = will_client.try_reveal_and_claim(
                        &target.will_id,
                        &target.claimant,
                        &target.preimage,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Allocation, Beneficiary, WillContract, WillContractClient, WillStatus};
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        vec,
    };

    const DAY: u64 = 86_400;

    #[test]
    fn test_reentrant_release_inheritance_cannot_double_pay() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(1_700_000_000);

        let owner = Address::generate(&env);
        let beneficiary = Address::generate(&env);

        let token_id = env.register(MaliciousToken, ());
        let token_client = MaliciousTokenClient::new(&env, &token_id);
        token_client.mint(&owner, &1_000_000);

        let will_contract_id = env.register(WillContract, ());
        let will_client = WillContractClient::new(&env, &will_contract_id);

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

        env.ledger()
            .set_timestamp(env.ledger().timestamp() + 91 * DAY);
        will_client.trigger_will(&will_id);

        env.ledger()
            .set_timestamp(env.ledger().timestamp() + 8 * DAY);

        // Arm the token to attempt a reentrant release during the payout.
        token_client.set_reentrancy_target(&will_contract_id, &will_id);

        will_client.release_inheritance(&will_id, &None);

        // The beneficiary must receive exactly one payout, not two.
        assert_eq!(token_client.balance(&beneficiary), 1_000_000);
        let will = will_client.get_will(&will_id);
        assert_eq!(will.status, WillStatus::Released);
    }
}
