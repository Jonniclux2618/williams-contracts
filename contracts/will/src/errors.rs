use soroban_sdk::contracterror;

/// Errors returned by the Williams contract.
///
/// Every error variant is surfaced to callers as a `#[contracterror]` so that
/// SDK and client code can match on a stable numeric code instead of parsing
/// panic messages.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum WillError {
    /// No will exists for the given identifier.
    ///
    /// Raised when a will id passed to a lookup or mutation entry point does
    /// not correspond to any stored will (for example `get_will`, `update_*`,
    /// `trigger_will`, `release_inheritance`).
    WillNotFound = 1,
    /// The caller is not the owner of the will.
    ///
    /// Raised by owner-only entry points (`update_beneficiaries`,
    /// `update_guardians`, `cancel_will`, `close_will`, ...) when the
    /// authenticated caller address does not match the will's stored owner.
    NotOwner = 2,
    /// The requested action requires the will to be `Active`.
    ///
    /// Raised when an operation that only makes sense on a live, checked-in
    /// will (such as `trigger_will` or `emergency_checkin`) is invoked while
    /// the will is in any other state.
    WillNotActive = 3,
    /// The requested action requires the will to be `Triggered`.
    ///
    /// Raised when an operation that only makes sense after the will has been
    /// triggered (such as `release_inheritance` or `guardian_cancel_trigger`)
    /// is invoked while the will is in any other state.
    WillNotTriggered = 4,
    /// `release_inheritance` was called before the grace period elapsed.
    ///
    /// After a will is triggered, the owner still has a grace window to check
    /// in and cancel. Releasing before that window closes is rejected.
    GracePeriodNotExpired = 5,
    /// `emergency_checkin` (or `guardian_cancel_trigger`) was called after the
    /// grace period already elapsed.
    ///
    /// Once the grace window has closed the will is committed to release, so
    /// late cancellation attempts are rejected.
    GracePeriodExpired = 6,
    /// Beneficiary percentages did not sum to exactly 10,000.
    ///
    /// Percentage allocations are expressed in basis points and must account
    /// for the whole balance; any other total is rejected by
    /// `assert_valid_allocations`.
    InvalidPercentages = 7,
    /// The guardian has already voted to trigger this will.
    ///
    /// Each guardian may cast a single trigger vote per will; repeat votes are
    /// rejected so quorum cannot be reached by double-counting one guardian.
    AlreadyVoted = 8,
    /// The caller is not a designated guardian of this will.
    ///
    /// Raised by guardian-only entry points (`guardian_trigger`,
    /// `guardian_cancel_trigger`, `guardian_consent`) when the caller is not
    /// present in the will's guardian list.
    NotGuardian = 9,
    /// `trigger_will` was called before the check-in deadline passed.
    ///
    /// The owner is still within their check-in window, so the will cannot yet
    /// be triggered.
    CheckinNotDue = 10,
    /// An amount of zero (or less) was supplied where a positive amount is required.
    ///
    /// Raised when a deposit, allocation or transfer amount is not strictly
    /// positive.
    ZeroAmount = 11,
    /// Too many beneficiaries (or guardians/tokens) were supplied, or the list
    /// was empty. A will may name at most `MAX_BENEFICIARIES` (10)
    /// beneficiaries; see the README FAQ for why the cap exists.
    /// A list-length cap was exceeded. Raised by:
    /// - a `beneficiaries` list that is empty or longer than
    ///   `MAX_BENEFICIARIES` — `create_will`, `update_beneficiaries`,
    ///   `update_will_settings` and each `batch_create_wills` spec;
    /// - a `renounce_beneficiary` call that would leave the will with no
    ///   beneficiaries at all: the last remaining beneficiary cannot renounce,
    ///   because a will with an empty list would have nobody for
    ///   `release_inheritance` to pay (#361);
    /// - a `guardians` list longer than `MAX_GUARDIANS` — from
    ///   `assert_valid_guardians`, reached by every entry point that installs
    ///   or replaces a guardian list;
    /// - a `batch_create_wills` spec list that is empty or longer than
    ///   `BATCH_MAX`.
    ///
    /// Token-list bounds are deliberately *not* reported here: an empty or
    /// over-long `tokens` list raises [`WillError::InvalidTokenCount`] (#390).
    TooManyBeneficiaries = 12,
    /// The requested action requires the will to be `Released`, `Cancelled` or
    /// `Settled` (a will closed via `close_will`).
    ///
    /// Raised when an operation that only makes sense on a closed will (such
    /// as `close_will` or post-release cleanup) is invoked while the will is
    /// still open.
    WillNotSettled = 13,
    /// Cannot merge: both wills must be owned by the same address.
    ///
    /// Raised by `merge_wills` when the two source wills have different
    /// owners.
    WillNotBothActive = 14,
    /// Cannot merge: same will id provided for both wills.
    ///
    /// Raised by `merge_wills` when the caller passes the same id twice.
    SameWillId = 15,
    /// Cannot merge: merging would result in too many beneficiaries or guardians.
    ///
    /// Raised by `merge_wills` when the combined lists would exceed
    /// `MAX_BENEFICIARIES` or `MAX_GUARDIANS`.
    MergeWouldExceedLimits = 16,
    /// The owner cannot designate themselves as a guardian of their own will.
    ///
    /// Raised when the owner address appears in a guardian list supplied to
    /// `create_will` or `update_guardians`.
    OwnerCannotBeGuardian = 17,
    /// A beneficiary is not found in the will's beneficiary list.
    ///
    /// Raised by operations that address a specific beneficiary (such as
    /// `renounce_beneficiary` or `update_beneficiary`) when the address is not
    /// present in the will.
    BeneficiaryNotFound = 18,
    /// Keeper bounty basis points exceed the maximum allowed (100 bps/1%).
    ///
    /// Raised when a will is created or updated with a keeper bounty above the
    /// protocol cap.
    KeeperBountyExceedsMax = 19,
    /// `guardian_threshold` lies outside the range the will's guardian list can
    /// actually reach.
    ///
    /// Quorum in `guardian_trigger` / `guardian_cancel_trigger` is compared
    /// against the **accumulated vote weight** of the consenting guardians, not
    /// against a head count, so the reachable maximum depends on how the list
    /// was installed:
    /// - unweighted lists — `create_will`, `update_guardians` and
    ///   `update_will_settings` give every guardian weight 1, so the valid
    ///   range is `1..=guardians.len()`;
    /// - weighted lists — `update_guardians_weighted` validates against
    ///   `1..=sum(guardian weights)`.
    ///
    /// A threshold of 0, or one above the reachable maximum, is rejected there.
    /// `update_guardians_weighted` also raises it for any `GuardianSpec` whose
    /// `weight` is `0`: zero weights are rejected, not normalised to `1` (#489).
    /// The same error is also raised when *shrinking* a non-empty guardian list
    /// would leave the will's already-stored `guardian_threshold`
    /// permanently unreachable, and when `update_guardians_weighted` is called
    /// without a threshold on a list whose total weight is below the stored
    /// one. An empty guardian list disables the mechanism, so no threshold is
    /// checked in that case.
    InvalidGuardianThreshold = 20,
    /// The sum of every `Allocation::FixedAmount` entry on a will exceeds the
    /// will's **primary-token** balance (`Will::token`, the first entry of the
    /// `tokens` list). Secondary tokens are not drawn on to satisfy a fixed
    /// amount, so they never contribute to this check (#384).
    ///
    /// `assert_valid_allocations` rejects only that over-commitment. A will
    /// whose `FixedAmount` entries do *not* exactly account for the whole
    /// primary-token balance is deliberately allowed: the unallocated headroom
    /// is what a later `add_hashed_beneficiary` call reserves for a
    /// not-yet-disclosed beneficiary (#181/#186), and if none is ever added the
    /// leftover is refunded to the owner at release time
    /// (`events::leftover_refunded`, #383) rather than stranded in the
    /// contract. Every `Percentage` share, by contrast, must still sum to
    /// exactly 10,000 basis points, or `InvalidPercentages` is raised.
    FixedAmountExceedsBalance = 21,
    /// The requested action requires the will to be `Released`.
    ///
    /// Raised when an operation that only makes sense after inheritance has
    /// been released is invoked while the will is in any other state.
    WillNotReleased = 23,
    /// Cannot merge: both wills must be owned by the same address.
    ///
    /// Raised by `merge_wills` when the two source wills do not share the same
    /// owner address.
    NotSameOwner = 24,
    /// A check-in or grace period was zero, or long enough that the resulting
    /// deadline could not be represented as a ledger timestamp.
    ///
    /// Raised when a will is created or updated with an out-of-range period.
    InvalidPeriod = 25,
    /// The same address was supplied more than once in a guardian list.
    ///
    /// Raised by `assert_valid_guardians` when a guardian list contains
    /// duplicate addresses.
    DuplicateGuardian = 26,
    /// The guardian-list cooldown has not yet elapsed; guardian_trigger is
    /// blocked until the cooldown period passes after the last guardian-list
    /// change.
    ///
    /// The cooldown prevents a freshly installed guardian set from immediately
    /// triggering the will.
    GuardianCooldownActive = 27,
    /// A supplied token address does not respond to a read-only `decimals()`
    /// probe, indicating it is not a valid SEP-41 token.
    ///
    /// Raised when a will is created or updated with a token address that
    /// fails the SEP-41 conformance probe.
    InvalidToken = 28,
    /// The same beneficiary address was supplied more than once.
    ///
    /// Raised by `assert_valid_allocations` when a beneficiary list contains
    /// duplicate addresses.
    DuplicateBeneficiary = 29,
    /// `confirm_will` was called on a will that is not `PendingConfirmation`.
    ///
    /// Raised when a will has already been confirmed (or is otherwise not
    /// awaiting confirmation).
    WillNotConfirmed = 30,
    /// `confirm_will` was called after the confirmation deadline elapsed.
    ///
    /// A will that is not confirmed within its confirmation window cannot be
    /// activated.
    ConfirmationWindowExpired = 31,
    /// `get_wills` was called with more ids than `MAX_GET_WILLS_IDS`.
    ///
    /// Raised to bound the cost of a single batched lookup.
    TooManyIds = 32,
    /// `split_will` was asked to move more of a token than the will
    /// currently holds of it.
    ///
    /// Raised when a split would transfer an amount exceeding the source
    /// will's balance for that token.
    InsufficientBalance = 33,
    /// `split_will` was called with an empty beneficiary-to-split list, or a
    /// split that would leave the source or new will with an invalid state.
    ///
    /// Raised when the requested split cannot produce two valid wills.
    InvalidSplit = 34,
    /// `reveal_and_claim` was called with a pre-image that does not match any
    /// stored `HashedBeneficiary` commitment on the will.
    ///
    /// Raised when the supplied pre-image hashes to a value that is not among
    /// the will's stored commitments.
    InvalidPreimage = 35,
    /// `reveal_and_claim` was called for a hashed beneficiary slot that has
    /// already been claimed.
    ///
    /// Each hashed beneficiary commitment can be revealed and claimed only
    /// once.
    AlreadyClaimed = 36,
    /// An owner or beneficiary index list is already at
    /// `MAX_WILLS_PER_INDEX` and cannot accept another will id.
    ///
    /// Raised when creating a will would push an address's will index past
    /// the per-index cap.
    TooManyWills = 37,
    /// A guardian has not accepted their role and cannot vote.
    ///
    /// Raised by `guardian_trigger` / `guardian_cancel_trigger` when the
    /// calling guardian has not yet recorded consent via `guardian_consent`.
    GuardianNotConsented = 38,
    /// `distribute` computed per-beneficiary shares that do not add up to
    /// the will's balance for some token. Aborts the release rather than
    /// silently leaving (or over-paying) funds.
    ///
    /// Raised during `release_inheritance` when the computed distribution is
    /// inconsistent with the will's balance for a token.
    DistributionMismatch = 39,
}
