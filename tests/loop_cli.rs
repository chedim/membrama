//! The loop's numbers on the command line: `--loop` turns the loop on and then every number is required (the owner's, no default).
use membrama::brain::{BrainConfig, Start};
use membrama::cli::parse;

fn base() -> Vec<String> {
    "--peer membrama --ns membrama --model /m/qwen3.gguf --n-ctx 2048 --slots 256 --key-cap 128 --val-cap 4096 --epoch 1 --spins 1000000 --depth 32 --bank-cap 2000000000"
        .split(' ').map(String::from).collect()
}
fn with_loop(skip: Option<&str>) -> Vec<String> {
    let mut a = base();
    for (o, v) in [("--loop-delta-cap", "40"), ("--loop-max-new", "512"), ("--loop-max-rounds", "6"), ("--loop-ask-ms", "250"), ("--loop-start", "head")] {
        if Some(o) != skip { a.push(o.into()); a.push(v.into()); }
    }
    a.push("--loop".into());
    a
}

#[test]
fn without_the_flag_there_is_no_loop() {
    assert_eq!(parse(&base()).unwrap().brain, None);
}

#[test]
fn the_flag_turns_the_loop_on_with_the_owners_numbers() {
    assert_eq!(parse(&with_loop(None)).unwrap().brain, Some(BrainConfig { delta_cap: 40, max_new: 512, max_rounds: 6, ask_every_ms: 250, start: Start::Head }));
}

#[test]
fn every_loop_number_is_required_and_the_error_names_it() {
    for o in ["--loop-delta-cap", "--loop-max-new", "--loop-max-rounds", "--loop-ask-ms", "--loop-start"] {
        let e = parse(&with_loop(Some(o))).unwrap_err();
        assert!(e.contains(o) && e.contains("no default"), "{o}: {e}");
    }
}

#[test]
fn the_start_is_head_or_beginning_and_nothing_else() {
    let mut a = with_loop(None);
    let i = a.iter().position(|x| x == "head").unwrap();
    a[i] = "beginning".into();
    assert_eq!(parse(&a).unwrap().brain.unwrap().start, Start::Beginning);
    a[i] = "yesterday".into();
    assert!(parse(&a).unwrap_err().contains("--loop-start"));
}

#[test]
fn loop_numbers_without_the_flag_are_refused_not_ignored() {
    let mut a = base();
    a.extend(["--loop-delta-cap".to_string(), "40".to_string()]);
    assert!(parse(&a).unwrap_err().contains("--loop"));
}
