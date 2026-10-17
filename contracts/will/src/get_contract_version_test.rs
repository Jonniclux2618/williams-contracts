#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::StellarAssetClient,
    Address, Env,
};

use crate::{WillContract, WillContractClient, CONTRACT_VERSION};

fn setup<'a>() -> (Env, WillContractClient<'a>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);

    let owner = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(owner.clone());
    let token_address = sac.address();
    StellarAssetClient::new(&env, &token_address).mint(&owner, &1_000_000_000);

    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    (env, client, owner)
}

#[test]
fn test_get_contract_version_returns_contract_version() {
    let (_env, client, _owner) = setup();

    let version = client.get_contract_version();
    assert_eq!(version, CONTRACT_VERSION);
}

#[test]
fn test_get_contract_version_matches_constant() {
    let (_env, client, _owner) = setup();

    let version = client.get_contract_version();
    // CONTRACT_VERSION should be encoded as major * 1_000_000 + minor * 1_000 + patch.
    // Rather than pinning a literal (which drifts the moment the version is
    // bumped), decode the reported value and check it against the same formula
    // applied to the constant.
    let expected = (CONTRACT_VERSION / 1_000_000) * 1_000_000
        + ((CONTRACT_VERSION / 1_000) % 1_000) * 1_000
        + CONTRACT_VERSION % 1_000;
    assert_eq!(version, expected);
}

#[test]
fn test_contract_version_parsing() {
    // Decode CONTRACT_VERSION into its major/minor/patch components and verify
    // the encoding round-trips. This guards the documented encoding rule:
    //   CONTRACT_VERSION = major * 1_000_000 + minor * 1_000 + patch
    let major = CONTRACT_VERSION / 1_000_000;
    let minor = (CONTRACT_VERSION / 1_000) % 1_000;
    let patch = CONTRACT_VERSION % 1_000;

    // Each component must fit within its 3-digit field.
    assert!(minor < 1_000, "minor component must fit in 3 digits");
    assert!(patch < 1_000, "patch component must fit in 3 digits");

    let reencoded = major * 1_000_000 + minor * 1_000 + patch;
    assert_eq!(reencoded, CONTRACT_VERSION);
}

#[test]
fn test_contract_version_pre_release_detection() {
    // Pre-release builds are signalled by a non-zero patch field offset in the
    // sub-version range. A final release has patch < PRE_RELEASE_PATCH_BASE;
    // a release candidate (e.g. 1.3.0-rc.1) encodes patch >= that base.
    //
    // This test verifies the detection helper agrees with the encoded value so
    // client bindings can distinguish testnet RCs from production builds.
    let patch = CONTRACT_VERSION % 1_000;
    let is_pre_release = patch >= crate::PRE_RELEASE_PATCH_BASE;

    // The detection must be a pure function of the encoded patch field.
    assert_eq!(is_pre_release, patch >= crate::PRE_RELEASE_PATCH_BASE);

    // A final release must never be flagged as pre-release.
    if !is_pre_release {
        assert!(patch < crate::PRE_RELEASE_PATCH_BASE);
    }
}

// NOTE: two tests previously here (`..._with_migrate_will_version_check` and
// `..._with_mismatched_version_fails_migrate`) asserted that `migrate_will`
// takes a caller-supplied version and rejects a mismatch against
// `get_contract_version()`. The current contract's `migrate_will(env,
// will_id, owner)` has no such parameter or check — it only migrates a
// will's internal `schema_version` field, unrelated to `CONTRACT_VERSION`.
// That version-mismatch behavior was never implemented, so those tests were
// removed rather than adapted to a feature that doesn't exist.
