//! The citation file (CITATION.cff): what Zenodo and GitHub read for the project's citation. It names the project and its repository, its license is the
//! crate's, and a `version` in it (written at the release commit, with `date-released`) is the crate's version.
use std::path::Path;

fn read(path: &Path) -> String { std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())) }

/// The value of a top-level `key: value` line, without quotes.
fn field(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| l.strip_prefix(&format!("{key}:")).map(|v| v.trim().trim_matches('"').to_string()))
}

const REPO: &str = "membrama";

fn root() -> &'static Path { Path::new(env!("CARGO_MANIFEST_DIR"))  }

#[test]
fn the_citation_file_names_the_project_its_repository_and_its_license() {
    let c = read(&root().join("CITATION.cff"));
    assert_eq!(field(&c, "cff-version").as_deref(), Some("1.2.0"));
    assert_eq!(field(&c, "type").as_deref(), Some("software"));
    assert_eq!(field(&c, "title").as_deref(), Some(REPO));
    assert_eq!(field(&c, "repository-code").as_deref(), Some(format!("https://github.com/chedim/{REPO}").as_str()));
    assert!(c.lines().any(|l| l == "authors:"), "authors are required");
    assert!(field(&c, "message").is_some(), "a message");
    assert!(c.lines().any(|l| l.starts_with("abstract:")), "an abstract");
    let cargo = read(&root().join("Cargo.toml"));
    assert!(cargo.lines().any(|l| l.trim() == format!("license = \"{}\"", field(&c, "license").unwrap())), "the license is the crate's");
}

#[test]
fn a_version_in_the_citation_file_is_the_crates_version() {
    let c = read(&root().join("CITATION.cff"));
    let Some(v) = field(&c, "version") else { return };   // written at the release commit
    let cargo = read(&root().join("Cargo.toml"));
    let crate_v = cargo.lines().find_map(|l| l.strip_prefix("version = ").map(|v| v.trim_matches('"').to_string())).expect("a version in Cargo.toml");
    assert_eq!(v, crate_v);
    assert!(field(&c, "date-released").is_some(), "a version comes with its date");
}
