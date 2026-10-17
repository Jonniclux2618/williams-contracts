#![cfg(test)]

//! Regression coverage for issue #220 and issue #392: `get_will_history`
//! records the full lifecycle, including the transition status, actor, and
//! action labels — and, since #392, the retained trail is bounded at
//! `MAX_HISTORY_ENTRIES` (oldest dropped first) and can be read in pages via
//! `get_will_history_page`.
//!
//! Before #392 every recorded status transition was appended to a single
//! persistent vector that was never trimmed, so a will cycling
//! Active → Triggered → Active through repeated emergency check-ins
//! accumulated entries forever. That risked the per-entry ledger size limit on
//! the write path and blew past resource limits on the read path, where
//! `get_will_history` returned the whole trail in one call with no paging.

use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    token::StellarAssetClient,
    vec, Address, Env, Symbol, Vec,
};

use crate::storage::MAX_HISTORY_ENTRIES;
use crate::{
    types::WillStatusTransition, Allocation, Beneficiary, WillContract, WillContractClient,
    WillStatus,
};

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

fn create_will(
    env: &Env,
    client: &WillContractClient<'_>,
    owner: &Address,
    token_address: &Address,
) -> u64 {
    client.create_will(
        owner,
        &vec![env, (token_address.clone(), 10_000_000_i128)],
        &vec![
            env,
            Beneficiary {
                address: Address::generate(env),
                allocation: Allocation::Percentage(10_000),
            },
        ],
        &90,
        &7,
        &vec![env],
        &2,
        &None,
        &0,
    )
}

/// Cycles the will Active → Triggered → Active `rounds` times. Each round
/// records a `checkin`, `trigger` and an `emerg` transition, so a round adds
/// three entries on top of the single `create` transition (#500).
fn cycle(env: &Env, client: &WillContractClient<'_>, owner: &Address, will_id: u64, rounds: u32) {
    for _ in 0..rounds {
        // check_in resets the deadline so the next trigger is due.
        client.check_in(&will_id, owner);
        advance(env, 91);
        client.trigger_will(&will_id);
        advance(env, 1);
        client.emergency_checkin(&will_id, owner);
    }
}

/// The action symbols of a trail, for cheap comparisons against the expected
/// sequence.
fn actions(history: &Vec<WillStatusTransition>) -> Vec<Symbol> {
    let mut out = Vec::new(history.env());
    for entry in history.iter() {
        out.push_back(entry.action);
    }
    out
}

#[test]
fn get_will_history_records_lifecycle_transition_sequence() {
    let (env, client, owner, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
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

    let history = client.get_will_history(&will_id);
    assert_eq!(history.len(), 1, "creation should append one audit entry");
    let create = history.get(0).unwrap();
    assert_eq!(create.from_status, WillStatus::Active);
    assert_eq!(create.to_status, WillStatus::Active);
    assert_eq!(create.actor, owner);
    assert_eq!(create.action, symbol_short!("create"));

    advance(&env, 91);
    client.trigger_will(&will_id);

    let history = client.get_will_history(&will_id);
    assert_eq!(
        history.len(),
        2,
        "trigger should append one more transition"
    );
    let trigger = history.get(1).unwrap();
    assert_eq!(trigger.from_status, WillStatus::Active);
    assert_eq!(trigger.to_status, WillStatus::Triggered);
    assert_eq!(trigger.actor, client.address.clone());
    assert_eq!(trigger.action, symbol_short!("trigger"));

    advance(&env, 8);
    client.release_inheritance(&will_id, &None);

    let history = client.get_will_history(&will_id);
    assert_eq!(
        history.len(),
        3,
        "release should append the final transition"
    );
    let release = history.get(2).unwrap();
    assert_eq!(release.from_status, WillStatus::Triggered);
    assert_eq!(release.to_status, WillStatus::Released);
    assert_eq!(release.actor, client.address.clone());
    assert_eq!(release.action, symbol_short!("release"));
}

// ── Issue #392: the retained trail is capped and pageable ─────────────────

#[test]
fn history_is_capped_and_drops_the_oldest_entries() {
    let (env, client, owner, token_address) = setup();
    let will_id = create_will(&env, &client, &owner, &token_address);

    // Push well past the cap so trimming has to do real work.
    cycle(&env, &client, &owner, will_id, MAX_HISTORY_ENTRIES);

    let history = client.get_will_history(&will_id);
    assert_eq!(
        history.len(),
        MAX_HISTORY_ENTRIES,
        "the retained trail must never exceed MAX_HISTORY_ENTRIES",
    );

    // The retained window is the *newest* entries: `create` is long gone, and
    // the trail still ends on the final transition.
    let symbols = actions(&history);
    assert!(
        !symbols.contains(&symbol_short!("create")),
        "the oldest entries must be the ones dropped",
    );
    let last = symbols.get(symbols.len() - 1).unwrap();
    assert_eq!(
        last,
        symbol_short!("emerg"),
        "the trail must still end on the most recent transition",
    );
}

#[test]
fn history_shorter_than_the_cap_is_returned_whole() {
    let (env, client, owner, token_address) = setup();
    let will_id = create_will(&env, &client, &owner, &token_address);

    cycle(&env, &client, &owner, will_id, 3);

    // 1 create + 3 per round (checkin, trigger, emerg) = 10, comfortably under the cap.
    let history = client.get_will_history(&will_id);
    assert_eq!(history.len(), 10);
    assert_eq!(actions(&history).get(0).unwrap(), symbol_short!("create"));
}

#[test]
fn history_page_returns_a_bounded_slice() {
    let (env, client, owner, token_address) = setup();
    let will_id = create_will(&env, &client, &owner, &token_address);

    cycle(&env, &client, &owner, will_id, 10);

    let page = client.get_will_history_page(&will_id, &None, &3);
    assert_eq!(page.len(), 3, "limit must bound the page size");
}

#[test]
fn history_pages_walk_the_whole_trail_without_gaps_or_duplicates() {
    let (env, client, owner, token_address) = setup();
    let will_id = create_will(&env, &client, &owner, &token_address);

    cycle(&env, &client, &owner, will_id, 10);

    // Walk the trail in pages of 4 and reassemble it using seq-based cursors
    // (#59). After each page, the next cursor is the seq of the last entry
    // received; entries with seq strictly greater than that are returned next.
    const PAGE: u32 = 4;
    let mut walked: Vec<WillStatusTransition> = Vec::new(&env);
    let mut cursor: Option<u32> = None;
    loop {
        let page = client.get_will_history_page(&will_id, &cursor, &PAGE);
        if page.is_empty() {
            break;
        }
        assert!(page.len() <= PAGE, "a page must never exceed the limit");
        // Advance the cursor to the seq of the last entry on this page.
        let last = page.get(page.len() - 1).unwrap();
        cursor = Some(last.seq);
        for entry in page.iter() {
            walked.push_back(entry);
        }
    }

    let full = client.get_will_history(&will_id);
    assert_eq!(
        walked.len(),
        full.len(),
        "paging must cover the whole trail",
    );
    for i in 0..full.len() {
        assert_eq!(
            walked.get(i).unwrap().seq,
            full.get(i).unwrap().seq,
            "paged entries must match the full trail in order",
        );
    }
}

// ── Issue #111: amortized batch pruning keeps the trail bounded ───────────

#[test]
fn history_size_stays_within_bounds_across_many_transitions() {
    let (env, client, owner, token_address) = setup();
    let will_id = create_will(&env, &client, &owner, &token_address);

    // Drive many consecutive transitions and assert the retained trail never
    // exceeds the cap at any point, not just at the end.
    for _ in 0..(MAX_HISTORY_ENTRIES + 5) {
        client.check_in(&will_id, &owner);
        advance(&env, 91);
        client.trigger_will(&will_id);
        advance(&env, 1);
        client.emergency_checkin(&will_id, &owner);

        let history = client.get_will_history(&will_id);
        assert!(
            history.len() <= MAX_HISTORY_ENTRIES,
            "history must never exceed MAX_HISTORY_ENTRIES after append",
        );
    }

    // Ordering is preserved: the newest entry is the last transition recorded.
    let history = client.get_will_history(&will_id);
    let last = history.get(history.len() - 1).unwrap();
    assert_eq!(last.action, symbol_short!("emerg"));
}
