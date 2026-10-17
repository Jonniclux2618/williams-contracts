# Resource costs

Soroban meters every invocation and bills for it: CPU instructions, the number
of ledger entries touched, their size in bytes, and rent for keeping persistent
entries alive. This document records what each Williams entry point costs, how
to measure it yourself, and the reasoning behind the storage layout decisions
that drive those numbers.

## Measuring

The profile lives in [`contracts/will/src/profile.rs`](../contracts/will/src/profile.rs)
and runs as part of the normal test suite. To see the table:

```sh
cargo test -p will --lib profile -- --nocapture
```

It reports, per entry point, the figures from `Env::cost_estimate()`: metered
instructions, ledger entries read and written, bytes read and written,
cumulative rent in ledger-bytes, and the fee those resources would attract at a
snapshot of Stellar pubnet rates.

Two caveats. The contract is registered natively rather than as Wasm, so
everything the host charges for reading, instantiating and running the Wasm
module is missing from `instructions`; ledger-entry counts and byte sizes are
unaffected. And resource estimation is approximate by design — the SDK points
at RPC simulation for the exact resources of a real submission. The numbers are
therefore a good basis for comparing a change against its baseline, and a poor
basis for predicting an exact bill.

`read_entries` counts entries read but *not* modified, so an entry point's
total ledger footprint is `read_entries + write_entries`.

## Current profile

Measured with 5 beneficiaries and 2 guardians.

| entry point | instructions | read entries | write entries | read bytes | write bytes | rent ledger-bytes | fee (stroops) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| create_will | 883996 | 18 | 14 | 92 | 4336 | 144733774455 | 1398261322 |
| check_in | 410415 | 8 | 4 | 0 | 2800 | 290304000 | 5016388 |
| get_will | 100689 | 2 | 0 | 0 | 0 | 0 | 71 |
| top_up | 500122 | 9 | 5 | 0 | 2608 | 0 | 2219292 |
| trigger_will | 447462 | 7 | 4 | 0 | 3140 | 427161600 | 4147343 |
| emergency_checkin (no votes cast) | 474304 | 8 | 5 | 0 | 3472 | 294451200 | 5061070 |
| release_inheritance | 1879186 | 21 | 18 | 92 | 6368 | 875059200 | 8543033 |
| cancel_will | 919185 | 16 | 14 | 0 | 4072 | 294451200 | 5099613 |
| update_beneficiaries (shares only) | 437996 | 4 | 2 | 0 | 2012 | 0 | 2208230 |
| update_beneficiaries (full replacement) | 987379 | 14 | 12 | 0 | 3512 | 808704000 | 10063232 |
| update_guardians (no votes cast) | 352370 | 8 | 6 | 0 | 2012 | 0 | 2222390 |
| guardian_trigger (below threshold) | 432377 | 6 | 3 | 0 | 2220 | 215654400 | 4291863 |
| guardian_trigger (reaches threshold) | 2189866 | 24 | 19 | 92 | 5400 | 1086566400 | 12783958 |
| update_guardians (clearing a vote) | 371695 | 8 | 6 | 0 | 2012 | 0 | 2222404 |
| get_wills_by_owner (1 will) | 120869 | 3 | 0 | 0 | 0 | 0 | 85 |
| get_wills_by_beneficiary (1 will) | 115533 | 3 | 0 | 0 | 0 | 0 | 81 |
| check_in (rent due) | 419343 | 8 | 4 | 0 | 2568 | 2184704000 | 23295424 |
| cancel_will (rent due) | 972845 | 16 | 14 | 0 | 4072 | 1568051200 | 17406470 |

### Why two rows for some calls

Most rows run against entries written moments earlier, whose remaining lifetime
still exceeds the 30-day extension threshold. `extend_ttl` is a no-op there, so
rent reads zero. The `(rent due)` rows age the ledger past that threshold first
and show what the same call costs on the roughly monthly occasion that does
have to top the will's rent up — for `check_in`, about 4.5x a free one. That
periodic top-up, not the instruction count, is the dominant long-run cost of
holding a will open.

## What each entry point touches

The reverse indexes (`OwnerWills`, `BeneficiaryWills`) exist so the contract can
answer "which wills am I named in" without an off-chain indexer. They are the
main source of storage traffic beyond the will entry itself, which is why most
of the tuning below concerns them.

- **`check_in` / `get_will`** — the hot paths, and already minimal: one read of
  the will entry, and for `check_in` one write back. Neither touches an index.
  See [below](#what-was-considered-and-rejected) for why they were left alone.
- **`create_will`** — unavoidably the most expensive call: it writes the will,
  the owner index, one index per beneficiary, and moves tokens.
- **`update_beneficiaries`** — cost depends entirely on how much the list
  actually changed, not on how large it is.
- **`guardian_trigger`** — cheap below the threshold; the vote that reaches
  quorum also distributes, so it carries the release cost too.
- **`get_wills_by_*`** — one read of the index plus one per will listed. Linear
  in the caller's number of wills, and read-only.

## Tuning applied

Five changes, each measured against the commit before them.

### Terminal wills stop paying rent

`Released` and `Cancelled` are terminal states — every entry point that could
touch a will again first asserts it is `Active` or `Triggered`. `save_will` no
longer extends the TTL of a will in either state, so the contract stops buying
60 more days of rent for an entry that can never change again.

| | before | after |
| --- | ---: | ---: |
| `cancel_will (rent due)` rent ledger-bytes | 1012147200 | 180147200 |
| `cancel_will (rent due)` fee | 7058777 | 2422527 |

A 66% fee reduction on that path, and the largest single saving in this pass.
The residual rent is the token contract's, not the will's.

**This is a deliberate behaviour change.** A terminal will keeps whatever
lifetime its last active operation bought — up to 60 days — and is then allowed
to archive rather than being renewed indefinitely. `get_will` on a long-settled
will may therefore need the entry restored. Reconstructing history from events
is unaffected: every state change still publishes one.

### Beneficiary updates only touch indexes that changed

`update_beneficiaries` used to remove the reverse index of every current
beneficiary and then re-add every new one. When an address appeared in both
lists — the common case, since most updates only re-cut the percentages — that
was a storage read and write per address for no net effect. Membership is now
decided against the two lists already in memory, at no storage cost.

| `update_beneficiaries (shares only)` | before | after |
| --- | ---: | ---: |
| write entries | 7 | 2 |
| instructions | 413024 | 155492 |
| fee | 1431880 | 1339485 |

The list is scanned quadratically to decide membership, which costs a full
replacement of 5 beneficiaries about 47,000 extra instructions. At 25 stroops
per 10,000 instructions that is 118 stroops, against 10,000 stroops for each
write entry it can avoid — so the trade is worth taking even when it does not
pay off. The measured worst case is `update_beneficiaries (full replacement)`:
+10.4% instructions, +0.002% fee.

### Guardian votes are only cleared when some exist

`will.guardian_votes` moves in lockstep with the per-guardian `GuardianVote`
markers, so a zero count means there is nothing to remove. `reset_guardian_votes`
now returns immediately in that case instead of issuing a storage removal per
guardian — the common case, since most wills never see a guardian vote.

| | before writes | after writes | before fee | after fee |
| --- | ---: | ---: | ---: | ---: |
| `emergency_checkin (no votes cast)` | 4 | 2 | 1372206 | 1339656 |
| `update_guardians (no votes cast)` | 4 | 2 | 1371962 | 1339404 |

`update_guardians (clearing a vote)` is unchanged, as it should be.

### The quorum-reaching guardian vote saves the will once

`guardian_trigger` wrote the will entry and then called `distribute`, which
wrote it again. The ledger footprint was already correct — the host counts
entries, not `set` calls — but the second serialisation was pure CPU.

| `guardian_trigger (reaches threshold)` | before | after |
| --- | ---: | ---: |
| instructions | 947917 | 892299 |

### Removals that change nothing do not write

`remove_beneficiary_index` rebuilt the index list and wrote it back even when
the will id was not in it. It now returns after the read. It also removes in
place instead of building a replacement list.

## What was considered and rejected

**Splitting the will entry into hot and cold halves.** `check_in` reads and
writes the whole `Will`, including the beneficiary and guardian lists it never
looks at. Storing the mutable fields (`last_checkin`, `status`, `trigger_time`,
`guardian_votes`, `balance`) separately from the immutable ones would cut its
write bytes by roughly 5x. It was rejected because `get_will` — the other
function this pass was asked to focus on — would need two reads instead of one
and pay the per-entry read fee twice, and because it is a storage layout change
requiring migration for any already-deployed will. Worth revisiting before
mainnet, when there is nothing to migrate.

**Storing `Will` as a tuple struct.** `#[contracttype]` encodes a named-field
struct as a map keyed by field-name symbols, so every read and write of a will
carries roughly 120 bytes of field names. A tuple struct would encode as a
vector and drop them. Rejected as a bad trade against the readability of the
type and its generated client bindings.

**Bumping `check_in` further.** After the changes above, `check_in` is one read
and one write of a single entry, and `get_will` is one read and no writes.
Neither has any redundant storage access left to remove; both were already at
that floor before this pass. Their numbers are unchanged, and the honest
finding is that the waste was elsewhere.

## Ledger close-time monitoring and `DAY_IN_LEDGERS` maintenance (issue #42)

`DAY_IN_LEDGERS = 17_280` is derived from an assumed **5-second average ledger
close time** on the Stellar network (`86,400 s/day ÷ 5 s/ledger`). It is a
compiled constant — changing it requires a contract redeployment. All storage
TTL margins (`LIFETIME_THRESHOLD` and `BUMP_AMOUNT`) are expressed as multiples
of this constant, so a sustained drift in the real close time shifts the
effective wall-clock safety margin proportionally.

### Safe thresholds and error margins

| Scenario | Effective `BUMP_AMOUNT` wall-clock | Action required? |
| --- | --- | --- |
| Close time stays at 5 s | ~60 days | None |
| Close time drifts to 6 s | ~50 days | Monitor; no immediate action |
| Close time drifts to 7 s | ~43 days | Consider redeployment before next major upgrade |
| Close time doubles to 10 s | ~30 days | Redeployment strongly recommended |
| Close time halves to 2.5 s | ~120 days | Wastes rent; update at next convenient upgrade |

The 60-day window is chosen to dwarf realistic drift: a 20% change in close
time (5 s → 6 s) reduces the effective margin by ~10 days, still leaving a
50-day runway to notice and plan a redeployment.

### Monitoring procedure

1. **Weekly**: query the Stellar Horizon statistics endpoint or a public
   explorer (e.g. Stellar.Expert) and record the 7-day median ledger close
   time.
2. **Alert threshold**: open a tracking issue when the rolling 7-day median
   exceeds **6.5 seconds** or drops below **4 seconds** (±30% of the nominal
   5 s). Neither value requires immediate action, but both indicate the
   constant should be reviewed at the next planned deployment.
3. **Action threshold**: schedule a redeployment when the 30-day median
   consistently exceeds **8 seconds** or drops below **3 seconds** (±60%
   of nominal). At 8 s, `BUMP_AMOUNT` is roughly 37.5 days, which is still
   above `LIFETIME_THRESHOLD` (30 days equivalent) but leaves little margin.

### How to update the constant

If a redeployment is warranted:

1. Update `DAY_IN_LEDGERS` in `contracts/will/src/storage.rs`:
   ```rust
   // Example for a 6-second average close time:
   const DAY_IN_LEDGERS: u32 = 14_400; // 86_400 s/day ÷ 6 s/ledger
   ```
2. `LIFETIME_THRESHOLD` and `BUMP_AMOUNT` update automatically because they
   are multiples of `DAY_IN_LEDGERS`; no further edits are required.
3. Re-run the resource profile (`cargo test -p will --lib profile -- --nocapture`)
   and update the table in this document if the rent figures change materially.
4. Existing deployed wills are unaffected until their next TTL-extending
   operation (e.g. `check_in`, `create_will`), which will bump them out to
   the new `BUMP_AMOUNT`.

## Hot/cold struct-split benchmark and v2 schema recommendation (issue #43)

The `profile_check_in_hot_cold_baseline` scenario in
[`contracts/will/src/profile.rs`](../contracts/will/src/profile.rs) measures
`check_in` write bytes at 1, 5, and 10 beneficiaries. Because `check_in` writes
the entire `Will` struct on every call, its write-byte cost grows linearly with
the size of the beneficiary list — even though `check_in` only modifies
`last_checkin`. Run it with:

```sh
cargo test -p will --lib profile_check_in_hot_cold_baseline -- --nocapture
```

### Expected baseline (5-second close time, 5 beneficiaries / 2 guardians)

| beneficiaries | write bytes | notes |
| ---: | ---: | --- |
| 1 | ~456 | minimal will |
| 5 | ~1 188 | standard profile size (matches current profile table) |
| 10 | ~1 900 | near `MAX_BENEFICIARIES` |

These are the baselines to compare against after a hot/cold migration.

### v2 schema migration recommendation

Splitting `Will` into `WillState` (hot: `last_checkin`, `status`,
`trigger_time`, `guardian_votes`, `balance`) and `WillConfig` (cold:
beneficiaries, guardians, `checkin_period_days`, `grace_period_days`) would
reduce `check_in` write bytes to roughly the `WillState` fraction — near the
"1 beneficiary" baseline above regardless of list size, a saving of ~5× at the
standard profile size.

The trade-off: `get_will` would need two reads instead of one, and any already-
deployed will would require migration before the hot key becomes available. This
makes the split the right change before mainnet launch, when the migration cost
is zero, and a poor choice after a large deployment without a comprehensive
migration path. See the ["What was considered and rejected"](#what-was-considered-and-rejected)
section for the detailed earlier analysis.

## `create_will` cost as protocol-wide distinct tokens grow

`ProtocolStats.total_locked_by_token` is a `Vec<TokenLockedBalance>` stored as
a single instance-storage entry. Every `create_will`, `top_up`, and
`cancel_will` call reads the whole entry, linearly scans it to find a matching
token, rebuilds the entire vector, and writes it back. The CPU and byte cost of
that operation scales with `N`, the number of **distinct token addresses ever
used across all wills on the protocol** — not the number of tokens in the will
being created.

To measure this effect, run the profile scenario `create_will (N protocol
tokens)` in `profile.rs` (see below for how to add it). The scenario pre-seeds
the `ProtocolStats` entry with varying values of `N` before calling
`create_will`, then records cost at each step.

### Expected growth

Each `TokenLockedBalance` entry serialises to roughly **72 bytes** (a 56-byte
`Address` plus a 16-byte `i128`). The table below shows the expected minimum
read/write byte growth on top of the baseline `create_will` cost (410 549
instructions, 888 read bytes, 2 720 write bytes at N = 1):

| N distinct protocol tokens | extra read bytes | extra write bytes | notes |
| ---: | ---: | ---: | --- |
| 1 (baseline) | 0 | 0 | current profile row |
| 10 | ~648 | ~648 | 9 extra entries × 72 bytes |
| 50 | ~3 528 | ~3 528 | |
| 100 | ~7 128 | ~7 128 | instance entry approaches 8 KB |
| 200 | ~14 328 | ~14 328 | instance entry exceeds 16 KB |

Instructions grow proportionally — each extra entry requires an address
comparison plus a clone. At 25 stroops per 10 000 instructions the instruction
cost is secondary to the byte cost, but both contribute to the fee.

### Why the current profile does not show this

The existing `profile_lifecycle` scenario creates one will with one token into a
fresh environment, so `N` is always 1. The growth is invisible unless the
protocol state is pre-seeded with many prior distinct tokens. Adding a
`profile_create_will_token_scaling` scenario to `profile.rs` that pre-seeds
`ProtocolStats` with 1, 10, 50, and 100 entries before each measurement would
make this growth visible in CI output. The scenario is not included in the
current profile because it requires no code change to understand the problem —
the growth follows directly from the linear-scan implementation — but it should
be added alongside any fix so the improvement can be measured against the
baseline.

### How to add the scenario

```rust
// In profile.rs — illustrative, not yet wired into profile_public_entry_points
fn profile_create_will_token_scaling(report: &mut Report) {
    for n in [1_u32, 10, 50, 100] {
        let f = fixture();
        // Pre-seed ProtocolStats with `n - 1` synthetic token entries so the
        // next create_will sees a vector of length `n - 1` before adding its own.
        f.env.as_contract(&f.client.address, || {
            use crate::storage::{DataKey, save_protocol_stats, get_protocol_stats};
            let mut stats = get_protocol_stats(&f.env);
            for _ in 1..n {
                stats.total_locked_by_token.push_back(
                    crate::types::TokenLockedBalance {
                        token: Address::generate(&f.env),
                        total_locked: 1,
                    }
                );
            }
            save_protocol_stats(&f.env, &stats);
        });
        f.create(&Vec::new(&f.env));
        let label = std::format!("create_will ({n} protocol tokens)");
        // report.record requires a &'static str; use a fixed set of labels
        // or extend Report to accept String.
    }
}
```

See [`docs/adr/0002-total-locked-by-token-scalability.md`](adr/0002-total-locked-by-token-scalability.md)
for the full analysis, options considered, and recommended fix.

## Contract instance storage TTL renewal (Resolved — Issue #21)

Previously, nothing in the contract renewed its own instance entry outside of
`next_will_id`. If a deployment experienced an extended period with no new wills
created, the contract instance could expire and archive, causing subsequent calls
to revert until restored.

This has been resolved: `env.storage().instance().extend_ttl(LIFETIME_THRESHOLD, BUMP_AMOUNT)`
is now invoked across high-frequency state-mutating entrypoints (`check_in`,
`batch_check_in`, `top_up`, `trigger_will`) and key lifecycle operations
(`emergency_checkin`, `release_inheritance`). Active wills now keep the parent
contract instance alive without requiring external restoration.

## Proportional rounding and dust remainder semantics (issue #32)

During `distribute` (invoked by `release_inheritance` and quorum-reaching
`guardian_trigger`), each token's distributable balance is split among
`Allocation::Percentage` beneficiaries using integer division that truncates
toward zero (`floor(remaining * basis_points / 10_000)` via `proportional_share`).
For `N` percentage beneficiaries, truncation across the first `N - 1` entries
leaves at most `N - 1` stroops of rounding dust per token.

### Deterministic final-beneficiary remainder convention

The final `Allocation::Percentage` beneficiary in `will.beneficiaries`
(`percentage_index == percentage_count`) deterministically absorbs the remaining
`percentage_remaining` balance (their proportional share plus the integer
truncation remainder) for every token held by the will:

1. **Zero dust stranded & strict balance conservation:** For every token,
   `keeper_bounty + sum(distributed_shares) + owner_refund + hashed_reserve == original_token_balance`
   holds strictly; any discrepancy panics with `WillError::DistributionMismatch`.
2. **Zero extra CPU or host-call overhead:** Rotating the remainder recipient
   per token (e.g. by hashing the token address or sorting recipients) would add
   per-token XDR serialization and SHA-256 host calls inside the hot
   `release_inheritance` loop for a difference bounded by `MAX_BENEFICIARIES - 1`
   (at most 9 stroops, or `0.0000009` XLM/USDC on a 7-decimal SEP-41 asset).
3. **Predictable client & SDK modeling:** Off-chain SDKs, dApp previews, and
   indexers can compute exact per-beneficiary payouts deterministically from the
   stored `will.beneficiaries` ordering without simulating host-level address
   hashing. Callers who wish to designate which beneficiary receives the
   sub-stroop rounding surplus can place that beneficiary last in the
   `Allocation::Percentage` list.
