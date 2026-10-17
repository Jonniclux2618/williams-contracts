# Williams Contract Error Codes

Every failure mode in the `will` contract is surfaced as a typed
`WillError` variant encoded as a `ContractError(u32)` in Soroban transaction
simulation and XDR results. This reference lists all discriminants 1–53 in
numerical order so SDK maintainers and Drips ecosystem contributors can decode
raw error codes without inspecting source.

All values are stable public ABI. New variants are always assigned the next
available discriminant; existing values are never changed.

| Code | Variant | Description |
|------|---------|-------------|
| 1 | `WillNotFound` | No will exists for the given identifier. |
| 2 | `NotOwner` | The caller is not the owner of the will. |
| 3 | `WillNotActive` | The requested action requires the will to be `Active`. |
| 4 | `WillNotTriggered` | The requested action requires the will to be `Triggered`. |
| 5 | `GracePeriodNotExpired` | `release_inheritance` was called before the grace period elapsed. |
| 6 | `GracePeriodExpired` | `emergency_checkin` (or `guardian_cancel_trigger`) was called after the grace period elapsed. |
| 7 | `InvalidPercentages` | Beneficiary percentages did not sum to exactly 10,000 basis points. |
| 8 | `AlreadyVoted` | The guardian has already voted to trigger this will. |
| 9 | `NotGuardian` | The caller is not a designated guardian of this will. |
| 10 | `CheckinNotDue` | `trigger_will` was called before the check-in deadline passed. |
| 11 | `ZeroAmount` | An amount of zero (or less) was supplied where a positive amount is required. |
| 12 | `TooManyBeneficiaries` | A list-length cap was exceeded (beneficiaries, guardians, or batch spec list). |
| 13 | `WillNotSettled` | The requested action requires the will to be `Released`, `Cancelled` or `Settled`. |
| 14 | `WillNotBothActive` | Cannot merge: one or both wills are not in `Active` status. |
| 15 | `SameWillId` | Cannot merge: the same will id was provided for both wills. |
| 16 | `MergeWouldExceedLimits` | Cannot merge: merging would result in too many beneficiaries or guardians. |
| 17 | `OwnerCannotBeGuardian` | The owner cannot designate themselves as a guardian of their own will. |
| 18 | `BeneficiaryNotFound` | A beneficiary is not found in the will's beneficiary list. |
| 19 | `KeeperBountyExceedsMax` | Keeper bounty basis points exceed the maximum allowed (100 bps / 1%). |
| 20 | `InvalidGuardianThreshold` | `guardian_threshold` lies outside the range the will's guardian list can reach. |
| 21 | `FixedAmountExceedsBalance` | The sum of `FixedAmount` entries exceeds the will's primary-token balance. |
| 23 | `WillNotReleased` | The requested action requires the will to be `Released`. |
| 24 | `NotSameOwner` | Cannot merge: both wills must be owned by the same address. |
| 25 | `InvalidPeriod` | A check-in or grace period was zero, or too large to represent as a ledger timestamp. |
| 26 | `DuplicateGuardian` | The same address was supplied more than once in a guardian list. |
| 27 | `GuardianCooldownActive` | The guardian-list cooldown has not yet elapsed; `guardian_trigger` is blocked. |
| 28 | `InvalidToken` | A supplied token address does not respond to `decimals()`, indicating it is not a valid SEP-41 token. |
| 29 | `DuplicateBeneficiary` | The same beneficiary address was supplied more than once. |
| 30 | `WillNotConfirmed` | `confirm_will` was called on a will that is not `PendingConfirmation`. |
| 31 | `ConfirmationWindowExpired` | `confirm_will` was called after the confirmation deadline elapsed. |
| 32 | `TooManyIds` | `get_wills` was called with more ids than `MAX_GET_WILLS_IDS`. |
| 33 | `InsufficientBalance` | `split_will` was asked to move more of a token than the will currently holds. |
| 34 | `InvalidSplit` | `split_will` was called with an empty beneficiary-to-split list, or a split that would leave an invalid state. |
| 35 | `InvalidPreimage` | `reveal_and_claim` was called with a pre-image that does not match any stored `HashedBeneficiary` commitment. |
| 36 | `AlreadyClaimed` | `reveal_and_claim` was called for a hashed beneficiary slot that has already been claimed. |
| 37 | `TooManyWills` | An owner or beneficiary index list is already at `MAX_WILLS_PER_INDEX` and cannot accept another will id. |
| 38 | `GuardianNotConsented` | A guardian has not accepted their role and cannot vote. |
| 39 | `DistributionMismatch` | `distribute` computed per-beneficiary shares that do not add up to the will's balance for some token. |
| 40 | `DuplicateToken` | The same token address was supplied more than once in a `tokens` list. |
| 41 | `BatchTooLarge` | A `batch_check_in` call supplied more will ids than `MAX_BATCH_CHECK_IN` (50). |
| 42 | `InvalidTokenCount` | The `tokens` list was empty or held more than `MAX_TOKENS` entries; also raised by `top_up` when the cap is reached for a new token. |
| 43 | `InvalidPreimageLength` | `reveal_and_claim` was called with a pre-image that is not exactly `SHA256_DIGEST_LEN` bytes. |
| 44 | `InvalidCommitmentLength` | A hashed-beneficiary commitment was not exactly `SHA256_DIGEST_LEN` bytes. |
| 45 | `DuplicateCommitment` | The same commitment hash is already registered on this will. |
| 46 | `PreimageAddressMismatch` | `reveal_and_claim`'s pre-image does not bind to the `claimant` address. |
| 47 | `MergeWithHashedBeneficiaries` | `merge_wills` was called while either will still carried unclaimed hashed beneficiaries. |
| 48 | `DuplicateWillId` | A `batch_check_in` `will_ids` list named the same will more than once. |
| 49 | `InvalidConsentTransition` | `accept_guardian_role` was called by a guardian who had already `Rejected` the role. |
| 50 | `NoFailedPayout` | `retry_failed_payout` was called for a tuple with no recorded failed payout. |
| 51 | `UnsupportedSchemaVersion` | A stored will could not be decoded as the current `Will` layout or any known legacy layout. |
| 52 | `PrimaryTokenMismatch` | Cannot merge: the two wills' primary tokens differ. |
| 53 | `GuardianCancelInProgress` | A guardian-list update was attempted while guardian-cancel votes were still recorded. |
| 54 | `ConfirmationWindowNotExpired` | `refund_unconfirmed_will` was called before the confirmation deadline plus grace margin had elapsed. |
| 55 | `ArithmeticOverflow` | A timestamp or deadline calculation overflowed `u64`; checked helpers panic with this error rather than wrapping silently. |
| 56 | `InvalidBountyToken` | `set_keeper_bounty_token` named a token the will does not currently hold. |
| 57 | `NotAuthorized` | `redirect_failed_payout` was called by an address that is neither the recorded failed-payout recipient nor the will's owner. |
| 58 | `UnclaimedHashedBeneficiary` | `archive_will` was called on a `Released` will that still has unclaimed hashed beneficiary shares. Archiving would destroy the will entry that `reveal_and_claim` needs to verify the commitment, permanently trapping those reserved funds. |

## Notes

- Discriminant `22` is unassigned (it was previously used by the removed
  `InvalidPercentage` variant and is reserved to avoid collision with any
  serialized on-chain data).
- Codes are stable across contract upgrades. If a variant is ever retired, its
  discriminant is kept reserved rather than reused.
- Raw Soroban error codes are reported as `ContractError(u32)` in transaction
  simulation results and XDR `ScError` values. Match against the numeric code
  above to identify the variant.
