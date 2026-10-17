//! Resource-cost profile for every public entry point.
//!
//! This is a measurement harness, not a correctness test — [`test`] covers
//! behaviour. Its job is to make the ledger cost of each
//! entry point visible, so that a change which quietly doubles the storage
//! traffic of a hot path shows up in review instead of on a fee invoice.
//!
//! Run it with the output visible:
//!
//! ```text
//! cargo test -p will --lib profile -- --nocapture
//! ```
//!
//! # Reading the numbers
//!
//! Figures come from `Env::cost_estimate()`, which reports the resources the
//! host metered during the last top-level invocation, plus that invocation's
//! fee under a snapshot of Stellar pubnet rates.
//!
//! Two caveats apply, and both are why these numbers are useful for comparing
//! a change against its baseline rather than for predicting an exact bill:
//!
//! - The contract is registered natively, not as Wasm. Everything the host
//!   charges for reading, instantiating and running the Wasm module is
//!   therefore missing, which understates `instructions` in particular.
//!   Ledger-entry counts and byte sizes — the quantities this profile exists
//!   to police — are unaffected.
//! - Resource estimation is approximate by design; the SDK points at RPC
//!   simulation for exact submission resources.
//!
//! `read_entries` counts entries read but *not* modified, so an entry point's
//! total ledger footprint is `read_entries + write_entries`.
//!
//! # Platform parameters
//!
//! The thresholds below track the Soroban SDK v28 budget model and the
//! Stellar Protocol 21/22 network limits (CPU instructions, memory, and
//! ledger IO pricing). They are deliberately generous — the profile exists to
//! catch regressions, not to pin exact costs — but they are current, so a
//! scenario that exceeds them indicates a real change in resource usage
//! rather than a stale baseline from an older SDK.

#![cfg(test)]

// The contract crate is `no_std`. Collecting rows and printing a table needs
// an allocator and stdout, which only this test-only module wants, so `std` is
// pulled in here rather than by relaxing the crate-level attribute.
extern crate std;

use std::{println, vec::Vec as StdVec};

use soroban_sdk::{
    testutils::{storage::Persistent as _, Address as _, Ledger},
    token::StellarAssetClient,
    vec, Address, Env, Vec,
};

use crate::{Allocation, Beneficiary, GuardianVoteReason, WillContract, WillContractClient};

const DAY: u64 = 86_400;

/// Beneficiary count used for the profiled will.
///
/// Half of `MAX_BENEFICIARIES`, so the recorded entry size reflects a
/// realistic will rather than the cheapest or the most expensive one possible.
const PROFILE_BENEFICIARIES: u32 = 5;

// ---------------------------------------------------------------------------
// Protocol 21/22 resource limits (Soroban SDK v28 budget model).
//
// These mirror the network-level caps enforced by the host. A profiled
// scenario that exceeds any of them would be rejected on-chain, so the
// profiling tests assert against them to guarantee the suite runs cleanly
// without budget panics.
// ---------------------------------------------------------------------------

/// Maximum CPU instructions per transaction (Protocol 21/22).
const MAX_INSTRUCTIONS: i64 = 100_000_000;

/// Maximum memory bytes per transaction (Protocol 21/22).
const MAX_MEMORY_BYTES: u64 = 40 * 1024 * 1024;

/// Maximum ledger entries read per transaction (Protocol 21/22).
const MAX_READ_ENTRIES: u32 = 40;

/// Maximum ledger entries written per transaction (Protocol 21/22).
const MAX_WRITE_ENTRIES: u32 = 25;

/// Maximum ledger bytes read per transaction (Protocol 21/22).
const MAX_READ_BYTES: u32 = 200 * 1024;

/// Maximum ledger bytes written per transaction (Protocol 21/22).
const MAX_WRITE_BYTES: u32 = 132 * 1024;

/// One measured invocation.
struct Row {
    name: &'static str,
    instructions: i64,
    read_entries: u32,
    write_entries: u32,
    read_bytes: u32,
    write_bytes: u32,
    rent_ledger_bytes: i64,
    fee_stroops: i64,
}

/// Collects one [`Row`] per profiled entry point and renders them as a table.
struct Report {
    rows: StdVec<Row>,
}

impl Report {
    fn new() -> Self {
        Self {
            rows: StdVec::new(),
        }
    }

    /// Records the resources metered for the most recent invocation on `env`.
    ///
    /// Must be called immediately after the call being profiled: the host only
    /// retains the resources of the last top-level invocation, so any
    /// intervening contract call — including a token transfer or a `get_will`
    /// used to set up an assertion — would be measured instead.
    fn record(&mut self, env: &Env, name: &'static str) {
        let resources = env.cost_estimate().resources();
        let fee = env.cost_estimate().fee();

        self.rows.push(Row {
            name,
            instructions: resources.instructions,
            // soroban-sdk v28 split the old single `read_entries` counter
            // into disk and memory reads. Everything a natively-registered
            // test contract touches is in-memory state, so the profile's
            // "entries read" column follows `memory_read_entries` (the
            // quantity the old counter measured in this harness).
            read_entries: resources.disk_read_entries + resources.memory_read_entries,
            write_entries: resources.write_entries,
            // There is no in-memory-read byte counter: byte sizes are only
            // metered for disk restores and ledger writes, so the "read
            // bytes" column follows `disk_read_bytes`. It reads 0 for calls
            // that touch only live in-memory state.
            read_bytes: resources.disk_read_bytes,
            write_bytes: resources.write_bytes,
            rent_ledger_bytes: resources.persistent_rent_ledger_bytes,
            fee_stroops: fee.total,
        });
    }

    /// Returns the row recorded under `name`, panicking if it is missing so a
    /// renamed scenario fails loudly instead of silently dropping its
    /// assertions.
    fn row(&self, name: &str) -> &Row {
        self.rows
            .iter()
            .find(|row| row.name == name)
            .unwrap_or_else(|| panic!("no profiled scenario named `{name}`"))
    }

    /// Asserts every recorded row fits within the Protocol 21/22 limits.
    ///
    /// This is what keeps the suite honest: a scenario that would be rejected
    /// on-chain fails here instead of silently reporting an over-budget cost.
    fn assert_within_limits(&self) {
        for row in &self.rows {
            assert!(
                row.instructions <= MAX_INSTRUCTIONS,
                "{}: {} instructions exceeds Protocol 21/22 limit of {}",
                row.name,
                row.instructions,
                MAX_INSTRUCTIONS,
            );
            assert!(
                row.read_entries <= MAX_READ_ENTRIES,
                "{}: {} read entries exceeds Protocol 21/22 limit of {}",
                row.name,
                row.read_entries,
                MAX_READ_ENTRIES,
            );
            assert!(
                row.write_entries <= MAX_WRITE_ENTRIES,
                "{}: {} write entries exceeds Protocol 21/22 limit of {}",
                row.name,
                row.write_entries,
                MAX_WRITE_ENTRIES,
            );
            assert!(
                row.read_bytes <= MAX_READ_BYTES,
                "{}: {} read bytes exceeds Protocol 21/22 limit of {}",
                row.name,
                row.read_bytes,
                MAX_READ_BYTES,
            );
            assert!(
                row.write_bytes <= MAX_WRITE_BYTES,
                "{}: {} write bytes exceeds Protocol 21/22 limit of {}",
                row.name,
                row.write_bytes,
                MAX_WRITE_BYTES,
            );
        }
    }

    /// Prints the profile as a Markdown table, ready to paste into a PR.
    fn print(&self) {
        println!(
            "\nWilliams resource profile ({PROFILE_BENEFICIARIES} beneficiaries, 2 guardians)\n"
        );
        println!(
            "| entry point | instructions | read entries | write entries | read bytes | write bytes | rent ledger-bytes | fee (stroops) |"
        );
        println!("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
        for row in &self.rows {
            println!(
                "| {} | {} | {} | {} | {} | {} | {} | {} |",
                row.name,
                row.instructions,
                row.read_entries,
                row.write_entries,
                row.read_bytes,
                row.write_bytes,
                row.rent_ledger_bytes,
                row.fee_stroops,
            );
        }
        println!();
    }
}

/// A profiling scenario: a fresh environment with a funded owner and a
/// registered contract.
struct Fixture<'a> {
    env: Env,
    client: WillContractClient<'a>,
    owner: Address,
    token: Address,
}

fn fixture<'a>() -> Fixture<'a> {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);

    let owner = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(owner.clone());
    let token = sac.address();
    StellarAssetClient::new(&env, &token).mint(&owner, &1_000_000_000_000);

    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    Fixture {
        env,
        client,
        owner,
        token,
    }
}

impl Fixture<'_> {
    /// Creates a will with `PROFILE_BENEFICIARIES` beneficiaries and the given
    /// guardians, returning its id and beneficiary list.
    fn create(&self, guardians: &Vec<Address>) -> (u64, Vec<Beneficiary>) {
        let list = beneficiaries(&self.env, PROFILE_BENEFICIARIES);
        let tokens: Vec<(Address, i128)> =
            soroban_sdk::vec![&self.env, (self.token.clone(), 1_000_000_i128)];
        let will_id = self.client.create_will(
            &self.owner,
            &tokens,
            &list,
            &90,
            &7,
            guardians,
            &2,
            &None,
            &0,
        );
        (will_id, list)
    }

    fn advance(&self, seconds: u64) {
        self.env.ledger().with_mut(|ledger| {
            ledger.timestamp += seconds;
        });
    }

    /// Advances the ledger sequence without moving the clock, ageing every
    /// stored entry towards expiry.
    fn age(&self, ledgers: u32) {
        self.env.ledger().with_mut(|ledger| {
            ledger.sequence_number += ledgers;
        });
    }
}

/// Builds `count` beneficiaries with fresh addresses whose basis points sum
/// to 10,000, any remainder going to the last entry.
fn beneficiaries(env: &Env, count: u32) -> Vec<Beneficiary> {
    let mut list = Vec::new(env);
    let even = 10_000 / count;
    let mut allocated: u32 = 0;
    for index in 0..count {
        let basis_points = if index == count - 1 {
            10_000 - allocated
        } else {
            even
        };
        allocated += basis_points;
        list.push_back(Beneficiary {
            address: Address::generate(env),
            basis_points,
        });
    }
    list
}

#[test]
fn profile() {
    let mut report = Report::new();

    // create_will
    {
        let f = fixture();
        let guardians = vec![&f.env, Address::generate(&f.env), Address::generate(&f.env)];
        let _ = f.create(&guardians);
        report.record(&f.env, "create_will");
    }

    // get_will
    {
        let f = fixture();
        let guardians = vec![&f.env, Address::generate(&f.env), Address::generate(&f.env)];
        let (will_id, _) = f.create(&guardians);
        let _ = f.client.get_will(&will_id);
        report.record(&f.env, "get_will");
    }

    // check_in
    {
        let f = fixture();
        let guardians = vec![&f.env, Address::generate(&f.env), Address::generate(&f.env)];
        let (will_id, _) = f.create(&guardians);
        f.advance(DAY);
        f.client.check_in(&will_id, &f.owner);
        report.record(&f.env, "check_in");
    }

    // trigger_guardian_vote
    {
        let f = fixture();
        let guardians = vec![&f.env, Address::generate(&f.env), Address::generate(&f.env)];
        let (will_id, _) = f.create(&guardians);
        f.advance(91 * DAY);
        f.client.trigger_guardian_vote(
            &will_id,
            &guardians.get(0).unwrap(),
            &GuardianVoteReason::Incapacitated,
        );
        report.record(&f.env, "trigger_guardian_vote");
    }

    // claim
    {
        let f = fixture();
        let guardians = vec![&f.env, Address::generate(&f.env), Address::generate(&f.env)];
        let (will_id, list) = f.create(&guardians);
        f.advance(91 * DAY);
        let beneficiary = list.get(0).unwrap().address.clone();
        f.client.claim(&will_id, &beneficiary);
        report.record(&f.env, "claim");
    }

    // age_entries (rent extension path)
    {
        let f = fixture();
        let guardians = vec![&f.env, Address::generate(&f.env), Address::generate(&f.env)];
        let (will_id, _) = f.create(&guardians);
        f.age(1_000);
        let _ = f.client.get_will(&will_id);
        report.record(&f.env, "get_will_after_age");
    }

    report.print();
    report.assert_within_limits();
}
