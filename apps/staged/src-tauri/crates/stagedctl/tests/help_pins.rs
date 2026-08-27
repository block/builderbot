//! Byte-pins the rendered `--help` of every noun and verb.
//!
//! Help is the CLI's contract with agents (the agent skill teaches
//! discovery via `--help` instead of restating flags), so drift is a build
//! failure, not a docs bug. To refresh the fixture after an intentional
//! change:
//!
//! ```sh
//! STAGEDCTL_UPDATE_HELP_PINS=1 cargo test -p stagedctl --test help_pins
//! ```

const FIXTURE_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/help.txt");

#[test]
fn rendered_help_matches_the_pinned_fixture() {
    let rendered = stagedctl::cli::render_all_help();

    if std::env::var_os("STAGEDCTL_UPDATE_HELP_PINS").is_some() {
        std::fs::write(FIXTURE_PATH, &rendered).expect("write help fixture");
        return;
    }

    let pinned = std::fs::read_to_string(FIXTURE_PATH).expect(
        "missing tests/fixtures/help.txt — run with STAGEDCTL_UPDATE_HELP_PINS=1 to create it",
    );
    assert_eq!(
        rendered, pinned,
        "help output changed; if intentional, refresh with \
         STAGEDCTL_UPDATE_HELP_PINS=1 cargo test -p stagedctl --test help_pins"
    );
}
