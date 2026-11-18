//! The command line of `membrama serve`: every number is required (the owner's, no default), an unknown option or a bad number is refused and named.
use membrama::cli::{parse, Config};
use membrama::peer::Numbers;

fn full() -> Vec<String> {
    "--peer membrama --ns membrama --model /m/qwen3.gguf --n-ctx 2048 --slots 256 --key-cap 128 --val-cap 4096 --epoch 1 --spins 1000000 --depth 32 --bank-cap 2000000000 --max-banks 1 --die-with-parent"
        .split(' ').map(String::from).collect()
}
fn without(opt: &str) -> Vec<String> {
    let mut a = full();
    if let Some(i) = a.iter().position(|x| x == opt) { a.remove(i); if opt != "--die-with-parent" { a.remove(i); } }
    a
}

#[test]
fn a_full_command_line_is_the_config() {
    assert_eq!(parse(&full()), Ok(Config {
        peer: "membrama".into(), ns: "membrama".into(), model: "/m/qwen3.gguf".into(), n_ctx: 2048,
        numbers: Numbers { slots: 256, key_cap: 128, val_cap: 4096, epoch: 1, spins: 1_000_000, depth: 32 },
        bank_cap: 2_000_000_000, max_banks: 1, die_with_parent: true, token_file: None,
    }));
}

#[test]
fn every_number_is_required_and_the_error_names_it() {
    for opt in ["--peer", "--ns", "--model", "--n-ctx", "--slots", "--key-cap", "--val-cap", "--epoch", "--spins", "--depth", "--bank-cap", "--max-banks"] {
        let e = parse(&without(opt)).unwrap_err();
        assert!(e.contains(opt), "{opt}: {e}");
    }
}

#[test]
fn die_with_parent_is_a_flag_and_may_be_left_out() {
    assert!(!parse(&without("--die-with-parent")).unwrap().die_with_parent);
}

#[test]
fn an_unknown_option_is_refused() {
    let mut a = full(); a.push("--turbo".into()); a.push("1".into());
    assert!(parse(&a).unwrap_err().contains("--turbo"));
}

#[test]
fn a_value_that_is_not_a_number_is_refused_and_the_option_named() {
    let mut a = full();
    let i = a.iter().position(|x| x == "--n-ctx").unwrap(); a[i + 1] = "lots".into();
    assert!(parse(&a).unwrap_err().contains("--n-ctx"));
}

#[test]
fn a_token_file_is_optional_and_named_when_given() {
    let mut a = full(); a.push("--token-file".into()); a.push("/d/peers/membrama.token".into());
    assert_eq!(parse(&a).unwrap().token_file.as_deref(), Some("/d/peers/membrama.token"));
    assert_eq!(parse(&full()).unwrap().token_file, None);
}
