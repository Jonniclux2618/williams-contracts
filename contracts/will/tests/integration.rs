//! Integration test that exercises the *compiled wasm artifact* through the
//! real `stellar` CLI against a local Soroban network, as opposed to the
//! in-process `soroban_sdk::Env::default()` unit tests in the modular test
//! suite files under `src/`.
//!
//! This is `#[ignore]`-gated because it requires `stellar`, `docker`, and
//! network access to spin up a local standalone network, none of which are
//! available (or desirable) in the fast unit-test CI job. It is run
//! separately via `scripts/integration_test.sh` and a dedicated CI job.
//! See CONTRIBUTING.md for how to run it locally.

use std::path::PathBuf;
use std::process::Command;

#[test]
#[ignore = "requires stellar CLI + docker; run via scripts/integration_test.sh or the integration CI job"]
fn lifecycle_through_real_wasm_artifact() {
    let script = repo_root().join("scripts/integration_test.sh");
    assert!(
        script.exists(),
        "expected integration script at {}",
        script.display()
    );

    let status = Command::new("bash")
        .arg(&script)
        .current_dir(repo_root())
        .status()
        .expect("failed to spawn scripts/integration_test.sh");

    assert!(status.success(), "integration_test.sh exited with {status}");
}

fn repo_root() -> PathBuf {
    // contracts/will/tests/integration.rs -> repo root is three levels up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("contracts/will should be nested two levels under the repo root")
        .to_path_buf()
}
