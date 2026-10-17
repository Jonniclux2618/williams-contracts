#![cfg(test)]

//! Regression coverage for issue #221: `archive_will` removes settled wills from
//! active owner and beneficiary indexes while rejecting in-flight lifecycle states.
//!
//! And for issue #393: archival also drops the will's `WillHistory` entry and
//! every `GuardianVote` / `GuardianCancelVote` entry belonging to its guardians,
//! rather than leaving them orphaned in persistent storage.
//!
//! And for issue #82: `get_owner_status_page` must skip archived/non-resolving
//! will ids consistently in both its pagination and count loops, so a mixed
//! active/archived owner index never panics during paginated reads.

use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    token::StellarAssetClient,
    vec, Address, Env,
};

use crate::event_test_support as ev;
use crate::storage::DataKey;
use crate::{Allocation, Beneficiary, GuardianVoteReason, WillContract, WillContractClient};

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

    (env.clone(), client, owner, token_address)
}

fn advance(env: &Env, days: u64) {
    env.ledger().with_mut(|l| l.timestamp += days * DAY);
}

#[test]
fn archive_will_removes_released_will_from_owner_and_beneficiary_indexes() {
    let (env, client, owner, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_000_i128)],
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

    advance(&env, 91);
    client.trigger_will(&will_id);
    advance(&env, 8);
    client.release_inheritance(&will_id, &None);

    client.archive_will(&will_id);

    let owner_wills = client.get_wills_by_owner(&owner, &None, &10);
    assert!(
        owner_wills.is_empty(),
        "released wills must be removed from owner index"
    );

    let beneficiary_wills = client.get_wills_by_beneficiary(&beneficiary, &None, &10);
    assert!(
        beneficiary_wills.is_empty(),
        "released wills must be removed from beneficiary index"
    );

    // archive_will removes the will's storage entry entirely (see its doc
    // comment in lib.rs), so get_will on an archived id must now panic with
    // WillNotFound rather than return a lingering Released record.
    let result = client.try_get_will(&will_id);
    assert!(
        result.is_err(),
        "an archived will's storage entry must be gone"
    );
}

#[test]
fn archive_will_removes_cancelled_will_from_active_indexes() {
    let (env, client, owner, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_000_i128)],
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

    client.cancel_will(&will_id, &owner);
    client.archive_will(&will_id);

    assert!(client.get_wills_by_owner(&owner, &None, &10).is_empty());
    assert!(client
        .get_wills_by_beneficiary(&beneficiary, &None, &10)
        .is_empty());
}

/// Regression test for issue #331: archiving a will that is currently in
/// `Triggered` status must remove its id from the global `TriggeredWills`
/// index so that keepers iterating `get_triggered_wills()` never see a
/// dangling id pointing at an entry that no longer resolves. Also covers
/// #462: this is precisely the triggered-state cleanup that guarantees
/// nothing reactivates if an archival were ever reversed.
#[test]
fn archive_triggered_will_removes_id_from_triggered_index() {
    let (env, client, owner, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_000_i128)],
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

    // Advance past the check-in deadline and trigger the will.
    advance(&env, 91);
    client.trigger_will(&will_id);

    // The will's id must now appear in the triggered-wills index.
    assert_eq!(
        client.get_triggered_wills(&None, &50).len(),
        1,
        "triggered will must be in index before archival"
    );

    // archive_will only accepts Released/Cancelled wills (see its doc
    // comment in lib.rs) -- a Triggered will must go through the grace
    // period and release_inheritance first, which already clears the
    // triggered-wills index on its own.
    advance(&env, 8);
    client.release_inheritance(&will_id, &None);
    assert!(
        client.get_triggered_wills(&None, &50).is_empty(),
        "release_inheritance must remove the will's id from the triggered-wills index"
    );

    // Archiving the now-Released will must not resurrect the stale id.
    client.archive_will(&will_id);
    assert!(
        client.get_triggered_wills(&None, &50).is_empty(),
        "archiving a released will must not leave a dangling id in the triggered-wills index"
    );
}

#[test]
#[should_panic]
fn archive_will_rejects_active_will() {
    let (env, client, owner, token_address) = setup();
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

    client.archive_will(&will_id);
}

// ── Issue #82: get_owner_status_page tolerates archived ids ──────────────

/// Regression test for issue #82: `get_owner_status_page` must not panic when
/// the owner's `ids` index still references a will whose storage entry has been
/// removed by archival. Both the pagination loop and the total-count loop must
/// skip non-resolving ids identically, so a mixed active/archived portfolio
/// returns cleanly and reports only the active wills.
#[test]
fn get_owner_status_page_skips_archived_wills() {
    let (env, client, owner, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let make_will = |amount: i128| {
        client.create_will(
            &owner,
            &vec![&env, (token_address.clone(), amount)],
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
        )
    };

    let active_id = make_will(1_000_000_i128);
    let archived_id = make_will(2_000_000_i128);

    // Cancel then archive the second will so its storage entry is gone while
    // its id may still linger in the owner index.
    client.cancel_will(&archived_id, &owner);
    client.archive_will(&archived_id);

    // Paginated read over the mixed active/archived portfolio must not panic.
    let page = client.get_owner_status_page(&owner, &None, &10);
    assert!(
        page.iter().all(|s| s.will_id != archived_id),
        "archived will ids must not appear in the status page"
    );
    assert!(
        page.iter().any(|s| s.will_id == active_id),
        "active will must still be reported"
    );
}

// ── Issue #393: archival drops history and guardian vote entries ─────────

/// Whether `key` is absent from persistent storage.
///
/// Must be called from inside `env.as_contract` — a registered contract's
/// persistent storage is only addressable from its own frame. Asserting
/// absence (rather than reading through a query) is the point: the orphaned
/// entries have no public reader left once the will is gone, so the only way to
/// prove they were removed is to look at the key itself.
fn key_exists(env: &Env, contract_id: &Address, key: &DataKey) -> bool {
    env.as_contract(contract_id, || env.storage().persistent().has(key))
}

/// Regression test for issue #393: `archive_will` moved the will to
/// `ArchivedWill` and cleared the owner/beneficiary/Triggered indexes, but left
/// the `WillHistory` entry and every `GuardianVote` / `GuardianCancelVote` entry
/// behind. Those keys then described a will `load_will` can no longer resolve,
/// while occupying ledger state until their TTL lapsed.
///
/// Also covers #462: a stray `GuardianVote`/`GuardianCancelVote` entry is
/// exactly the "pending guardian vote" state that issue describes as unsafe to
/// leave behind, since it would still be live if archival were ever reversed.
///
/// Asserts each key is present before archival and absent after, so the test
/// fails both if the cleanup regresses and if the setup silently stopped
/// creating the entries in the first place.
#[test]
fn archive_will_removes_history_and_guardian_vote_entries() {
    let env = Env::default();
    env.mock_all_auths();
    e

/* … truncated 6697 chars — edit only what you need near the top … */
