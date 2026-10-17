#![cfg(test)]

//! Regression coverage for issue #498: `get_protocol_stats` sums locked
//! balances incrementally but had no verified way to catch bookkeeping
//! drift. `audit_protocol_stats`/`repair_protocol_stats` (added for #441)
//! already provide that mechanism -- compare the incrementally-maintained
//! `ProtocolStats` against a fresh recomputation from every will's stored
//! balance -- but had zero test coverage, and the `ProtocolStatsAudit` type
//! `audit_protocol_stats` returns was referenced in `lib.rs` without ever
//! being defined in `types.rs`, a compile error fixed alongside these tests.

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::StellarAssetClient,
    vec, Address, Env,
};

use crate::storage;
use crate::types::{ProtocolStats, TokenLockedBalance};
use crate::{Allocation, Beneficiary, WillContract, WillContractClient};

fn setup<'a>() -> (Env, WillContractClient<'a>, Address, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);

    let owner = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(owner.clone());
    let token_address = sac.address();
    StellarAssetClient::new(&env, &token_address).mint(&owner, &1_000_000_000);

    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    (env.clone(), client, owner, token_address, contract_id)
}

#[test]
fn audit_reports_consistent_after_normal_operations() {
    let (env, client, owner, token_address, _contract_id) = setup();
    let beneficiary = Address::generate(&env);

    client.create_will(
        &owner,
        &vec![&env, (token_address.clone(), 100_000_i128)],
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

    let audit = client.audit_protocol_stats();
    assert!(
        audit.consistent,
        "stored and recomputed stats must agree after a single create_will"
    );
    // When consistent, computed carries only discrepancies (none) to avoid
    // materialising a full second copy of the token-balance list (#76).
    assert!(
        audit.computed.total_locked_by_token.is_empty(),
        "computed.total_locked_by_token must be empty when stats are consistent (#76)"
    );
    assert_eq!(
        audit.computed.active_will_count,
        audit.stored.active_will_count,
        "computed.active_will_count must match stored when consistent"
    );
    assert_eq!(audit.stored.active_will_count, 1);
}

#[test]
fn audit_reports_consistent_across_multiple_wills_and_a_cancellation() {
    let (env, client, owner, token_address, _contract_id) = setup();
    let beneficiary = Address::generate(&env);

    let will_1 = client.create_will(
        &owner,
        &vec![&env, (token_address.clone(), 100_000_i128)],
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
    client.create_will(
        &owner,
        &vec![&env, (token_address.clone(), 250_000_i128)],
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

    // Cancelling one will must be reflected in both the stored and
    // recomputed totals identically.
    client.cancel_will(&will_1, &owner);

    let audit = client.audit_protocol_stats();
    assert!(
        audit.consistent,
        "stored and recomputed stats must agree after create_will x2 + cancel_will"
    );
    assert_eq!(audit.stored.active_will_count, 1);
}

#[test]
fn audit_detects_drift_and_repair_fixes_it() {
    let (env, client, owner, token_address, contract_id) = setup();
    let beneficiary = Address::generate(&env);

    client.create_will(
        &owner,
        &vec![&env, (token_address.clone(), 100_000_i128)],
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

    // Sanity check: consistent before we deliberately corrupt anything.
    assert!(client.audit_protocol_stats().consistent);

    // Directly corrupt the stored stats to simulate bookkeeping drift --
    // an incrementing helper skipped on some future code path, for example.
    let real_computed = env.as_contract(&contract_id, || storage::recompute_protocol_stats(&env));
    let corrupted = ProtocolStats {
        active_will_count: real_computed.active_will_count + 41,
        total_locked_by_token: vec![
            &env,
            TokenLockedBalance {
                token: token_address.clone(),
                total_locked: 999_999_999,
            },
        ],
    };
    env.as_contract(&contract_id, || {
        storage::save_protocol_stats(&env, &corrupted);
    });

    let audit = client.audit_protocol_stats();
    assert!(
        !audit.consistent,
        "audit must detect the deliberately-introduced drift"
    );
    assert_eq!(
        audit.stored.active_will_count,
        real_computed.active_will_count + 41
    );
    // With #76, `computed` carries only discrepant token entries (sparse),
    // not a full copy of recomputed stats.  Verify the discrepancy for our
    // token shows the correct recomputed total.
    assert_eq!(
        audit.computed.active_will_count,
        real_computed.active_will_count,
        "computed.active_will_count must reflect the true recomputed count"
    );
    let discrepant_token = audit
        .computed
        .total_locked_by_token
        .iter()
        .find(|e| e.token == token_address)
        .expect("token_address must appear in computed discrepancies");
    assert_eq!(
        discrepant_token.total_locked,
        real_computed
            .total_locked_by_token
            .iter()
            .find(|e| e.token == token_address)
            .map(|e| e.total_locked)
            .unwrap_or(0),
        "discrepant computed total must equal the true recomputed total"
    );

    // repair_protocol_stats() must overwrite the drifted stored value with
    // the recomputed truth, and return it.
    let repaired = client.repair_protocol_stats(&owner);
    assert_eq!(repaired, real_computed);

    let audit_after_repair = client.audit_protocol_stats();
    assert!(
        audit_after_repair.consistent,
        "audit must report consistent again immediately after repair"
    );
    assert_eq!(audit_after_repair.stored, real_computed);
}

#[test]
fn repair_drops_tokens_whose_recomputed_total_is_zero() {
    let (env, client, owner, token_address, contract_id) = setup();
    let beneficiary = Address::generate(&env);

    client.create_will(
        &owner,
        &vec![&env, (token_address.clone(), 100_000_i128)],
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

    // Inject a stale entry for a token no active will actually holds.
    let stale_token = Address::generate(&env);
    let mut stored = env.as_contract(&contract_id, || storage::get_protocol_stats(&env));
    stored.total_locked_by_token.push_back(TokenLockedBalance {
        token: stale_token.clone(),
        total_locked: 5_000,
    });
    env.as_contract(&contract_id, || {
        storage::save_protocol_stats(&env, &stored);
    });

    assert!(!client.audit_protocol_stats().consistent);

    let repaired = client.repair_protocol_stats(&owner);
    let stale_entry_present = repaired
        .total_locked_by_token
        .iter()
        .any(|entry| entry.token == stale_token);
    assert!(
        !stale_entry_present,
        "repair must drop a token entry whose recomputed total is zero, not keep it at zero"
    );
}

// --- Test for issue #55 ---

#[test]
fn repair_protocol_stats_requires_caller_auth() {
    // #55: repair_protocol_stats now takes an explicit `caller: Address` and
    // calls `caller.require_auth()`. This test verifies the function still
    // works correctly when called with a valid, authorised caller, and that
    // the caller address is not constrained to the will owner — any address
    // that signs the transaction is accepted.
    let (env, client, _owner, token_address, _contract_id) = setup();
    let caller = Address::generate(&env);
    let beneficiary = Address::generate(&env);

    client.create_will(
        &_owner,
        &vec![&env, (token_address.clone(), 100_000_i128)],
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

    // repair_protocol_stats must succeed when called with an authorised
    // caller (mock_all_auths satisfies the require_auth check).
    let repaired = client.repair_protocol_stats(&caller);
    assert_eq!(repaired.active_will_count, 1);
}
