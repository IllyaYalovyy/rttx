//! Validates that the man page documents all CLI subcommands.
//! Prevents drift between the CLI and its documentation.

use std::fs;
use std::path::PathBuf;

fn man_page_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("man/rttx-server.1")
}

#[test]
fn man_page_exists_and_is_valid_roff() {
    let path = man_page_path();
    let content = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read man page at {}: {e}", path.display()));

    assert!(content.starts_with(".TH RTTX-SERVER"), "Man page must start with .TH header");
    assert!(content.contains(".SH NAME"), "Man page must have NAME section");
    assert!(content.contains(".SH SYNOPSIS"), "Man page must have SYNOPSIS section");
    assert!(content.contains(".SH COMMANDS"), "Man page must have COMMANDS section");
}

#[test]
fn man_page_documents_all_subcommands() {
    let content = fs::read_to_string(man_page_path()).unwrap();

    let expected_commands = [
        "start",
        "stop",
        "status",
        "clean",
        "kill",
        "attach-stdio",
        "logs",
        "diagnostics",
        "config",
        "profile",
    ];

    for cmd in expected_commands {
        assert!(content.contains(cmd), "Man page must document the '{cmd}' subcommand");
    }
}

#[test]
fn man_page_documents_environment_variables() {
    let content = fs::read_to_string(man_page_path()).unwrap();

    assert!(content.contains("RTTX_DEV_MODE"), "Man page must document RTTX_DEV_MODE");
    assert!(content.contains("RUST_LOG"), "Man page must document RUST_LOG");
}
