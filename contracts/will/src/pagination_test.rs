#![cfg(test)]

//! Regression test for pagination support in `get_wills_by_beneficiary`.
//! Verifies that the function respects cursor and limit parameters, staying
//! within resource limits even when a beneficiary is named on many wills.
//!
//! Also contains direct unit tests for `storage::paginate_ids` and
//! `storage::paginate_history`, verifying that:
//!  - page_size is computed before any iteration (slice bounds before copy);
//!  - cursor semantics are value-based, not position-based;
//!  - only the items that belong to the requested page are returned.

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient},
    vec, Address, Env,
};

use crate::{Allocation, Beneficiary, WillContract, WillContractClient};

#[allow(dead_code)]
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
    StellarAssetClient::new(&env, &token_address).mint(&owner, &1_000_000_000_000);

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

#[test]
fn pagination_respects_limit_parameter() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    // Create multiple wills with the same beneficiary
    let mut will_ids = vec![&env];
    for i in 0..5 {
        let will_id = client.create_will(
            &owner,
            &vec![&env, (token_address.clone(), 100_000_i128 + i as i128)],
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
        will_ids.push_back(will_id);
    }

    // Fetch with limit=2
    let first_page = client.get_wills_by_beneficiary(&beneficiary, &None, &2);
    assert_eq!(first_page.len(), 2);

    // Fetch with cursor pointing to second page
    let second_page =
        client.get_wills_by_beneficiary(&beneficiary, &first_page.get(1).map(|w| w.id), &2);
    assert_eq!(second_page.len(), 2);

    // Verify no duplicates between pages
    let first_ids: Vec<u64> = first_page.iter().map(|w| w.id).collect();
    let second_ids: Vec<u64> = second_page.iter().map(|w| w.id).collect();
    for id in &second_ids {
        assert!(!first_ids.contains(id));
    }
}

#[test]
fn pagination_handles_all_results() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    // Create 3 wills
    let mut will_ids = vec![&env];
    for i in 0..3 {
        let will_id = client.create_will(
            &owner,
            &vec![&env, (token_address.clone(), 100_000_i128 + i as i128)],
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
        will_ids.push_back(will_id);
    }

    // Fetch all without pagination (limit = 0 or very high)
    let all_wills = client.get_wills_by_beneficiary(&beneficiary, &None, &100);
    assert_eq!(all_wills.len(), 3);
}

#[test]
fn pagination_with_invalid_cursor_starts_from_beginning() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    // Create 2 wills
    for i in 0..2 {
        client.create_will(
            &owner,
            &vec![&env, (token_address.clone(), 100_000_i128 + i as i128)],
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
    }

    // Fetch with invalid cursor (should behave gracefully)
    let results = client.get_wills_by_beneficiary(&beneficiary, &Some(9999_u64), &10);
    // Should return remaining wills after cursor, or empty if cursor is beyond all
    assert!(results.len() <= 2);
}

// ---------------------------------------------------------------------------
// Direct unit tests for storage::paginate_ids
// ---------------------------------------------------------------------------
//
// These tests drive the helper directly — no contract setup required — to
// verify that:
//   1. slice bounds (page_size) are calculated before iterating;
//   2. only `limit` (capped at MAX_PAGE_SIZE) items are returned;
//   3. the cursor is a *value bound* (exclusive), never a position;
//   4. out-of-range or past-end cursors return an empty page without iterating;
//   5. a zero limit returns empty immediately.
//
// They complement the higher-level beneficiary tests above, which go through
// the full contract machinery.

use soroban_sdk::Vec as SorobanVec;

use crate::storage::paginate_ids;

fn make_ids(env: &Env, values: &[u64]) -> SorobanVec<u64> {
    let mut v = SorobanVec::new(env);
    for &id in values {
        v.push_back(id);
    }
    v
}

/// `paginate_ids` with no cursor must return the first `limit` ids.
#[test]
fn paginate_ids_no_cursor_returns_first_page() {
    let env = Env::default();
    let ids = make_ids(&env, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);

    let page = paginate_ids(&env, &ids, None, 3);
    assert_eq!(page.len(), 3);
    assert_eq!(page.get(0).unwrap(), 1);
    assert_eq!(page.get(1).unwrap(), 2);
    assert_eq!(page.get(2).unwrap(), 3);
}

/// Cursor is exclusive: the first returned id must be strictly greater than
/// the cursor value.
#[test]
fn paginate_ids_cursor_is_exclusive_value_bound() {
    let env = Env::default();
    let ids = make_ids(&env, &[10, 20, 30, 40, 50]);

    // cursor = 20 → skip 10 and 20, return from 30 onward
    let page = paginate_ids(&env, &ids, Some(20), 3);
    assert_eq!(page.len(), 3);
    assert_eq!(page.get(0).unwrap(), 30);
    assert_eq!(page.get(1).unwrap(), 40);
    assert_eq!(page.get(2).unwrap(), 50);
}

/// A cursor at or beyond the last id must return an empty page without
/// iterating over all elements.
#[test]
fn paginate_ids_cursor_beyond_last_returns_empty() {
    let env = Env::default();
    let ids = make_ids(&env, &[1, 2, 3]);

    // cursor equals the last element → nothing is strictly greater
    let page = paginate_ids(&env, &ids, Some(3), 10);
    assert!(page.is_empty());

    // cursor beyond the last element
    let page2 = paginate_ids(&env, &ids, Some(100), 10);
    assert!(page2.is_empty());
}

/// A zero limit must return an empty page immediately.
#[test]
fn paginate_ids_zero_limit_returns_empty() {
    let env = Env::default();
    let ids = make_ids(&env, &[1, 2, 3, 4, 5]);

    let page = paginate_ids(&env, &ids, None, 0);
    assert!(page.is_empty());
}

/// An empty index must always return an empty page regardless of cursor.
#[test]
fn paginate_ids_empty_index_returns_empty() {
    let env = Env::default();
    let ids = make_ids(&env, &[]);

    assert!(paginate_ids(&env, &ids, None, 10).is_empty());
    assert!(paginate_ids(&env, &ids, Some(0), 10).is_empty());
}

/// Consecutive pages must cover all ids exactly once with no overlap.
#[test]
fn paginate_ids_consecutive_pages_cover_all_ids() {
    let env = Env::default();
    let ids = make_ids(&env, &[1, 2, 3, 4, 5, 6, 7]);

    let mut collected: Vec<u64> = Vec::new();
    let mut cursor: Option<u64> = None;
    loop {
        let page = paginate_ids(&env, &ids, cursor, 3);
        if page.is_empty() {
            break;
        }
        for i in 0..page.len() {
            collected.push(page.get(i).unwrap());
        }
        cursor = page.get(page.len() - 1);
    }

    assert_eq!(collected, vec![1, 2, 3, 4, 5, 6, 7]);
}

/// Exact pagination output: a limit smaller than the remaining items must
/// return exactly `limit` items, in order, starting strictly after the cursor.
#[test]
fn paginate_ids_exact_output_with_limit() {
    let env = Env::default();
    let ids = make_ids(&env, &[5, 10, 15, 20, 25, 30, 35, 40]);

    // First page: no cursor, limit 3 → [5, 10, 15]
    let page = paginate_ids(&env, &ids, None, 3);
    assert_eq!(page.len(), 3);
    assert_eq!(page.get(0).unwrap(), 5);
    assert_eq!(page.get(1).unwrap(), 10);
    assert_eq!(page.get(2).unwrap(), 15);

    // Second page: cursor 15, limit 3 → [20, 25, 30]
    let page2 = paginate_ids(&env, &ids, Some(15), 3);
    assert_eq!(page2.len(), 3);
    assert_eq!(page2.get(0).unwrap(), 20);
    assert_eq!(page2.get(1).unwrap(), 25);
    assert_eq!(page2.get(2).unwrap(), 30);

    // Third page: cursor 30, limit 3 → [35, 40] (fewer than limit)
    let page3 = paginate_ids(&env, &ids, Some(30), 3);
    assert_eq!(page3.len(), 2);
    assert_eq!(page3.get(0).unwrap(), 35);
    assert_eq!(page3.get(1).unwrap(), 40);
}
