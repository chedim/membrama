//! The bank store (docs/design.md, "The bank" and "Budget"): keys, the move of an entity's version, per-model separation,
//! least-recently-used eviction under a byte cap, and refusal of a bank that cannot fit. No model is needed.
use membrama::bank::{hash_text, Bank, BankKey, BankStore};

fn key(model: &str, eid: &str, version: &str, text: &str) -> BankKey {
    BankKey { model: model.into(), eid: eid.into(), version: version.into(), text_hash: hash_text(text) }
}
fn bank(tokens: u32, bytes: usize) -> Bank { Bank { tokens, state: vec![7u8; bytes] } }

#[test]
fn a_banked_entity_comes_back_by_its_key() {
    let mut s = BankStore::new(1000);
    let k = key("m1", "Zorvex", "v1", "Zorvex lives in Lima.");
    assert!(s.put(k.clone(), bank(6, 100)).is_ok());
    let got = s.get(&k).expect("banked");
    assert_eq!((got.tokens, got.state.len()), (6, 100));
}

#[test]
fn a_key_that_was_never_banked_is_a_miss() {
    let mut s = BankStore::new(1000);
    assert!(s.get(&key("m1", "Zorvex", "v1", "x")).is_none());
}

#[test]
fn a_new_version_of_the_entity_drops_the_old_one() {
    let mut s = BankStore::new(1000);
    let old = key("m1", "Zorvex", "v1", "Zorvex lives in Lima.");
    let new = key("m1", "Zorvex", "v2", "Zorvex lives in Cusco.");
    s.put(old.clone(), bank(6, 100)).unwrap();
    s.put(new.clone(), bank(6, 100)).unwrap();
    assert!(s.get(&old).is_none(), "the old version must be gone");
    assert!(s.get(&new).is_some());
    assert_eq!(s.bytes(), 100);
}

#[test]
fn the_same_version_rendered_two_ways_keeps_both() {
    let mut s = BankStore::new(1000);
    let a = key("m1", "Zorvex", "v1", "Zorvex lives in Lima.");
    let b = key("m1", "Zorvex", "v1", "Context: Zorvex lives in Lima.");
    s.put(a.clone(), bank(6, 100)).unwrap();
    s.put(b.clone(), bank(8, 100)).unwrap();
    assert!(s.get(&a).is_some() && s.get(&b).is_some());
}

#[test]
fn another_models_bank_of_the_same_entity_is_a_different_bank() {
    let mut s = BankStore::new(1000);
    let a = key("m1", "Zorvex", "v1", "t");
    let b = key("m2", "Zorvex", "v1", "t");
    s.put(a.clone(), bank(1, 100)).unwrap();
    s.put(b.clone(), bank(1, 100)).unwrap();
    assert!(s.get(&a).is_some() && s.get(&b).is_some());
}

#[test]
fn over_the_cap_the_least_recently_used_bank_goes() {
    let mut s = BankStore::new(100);
    let (a, b, c) = (key("m", "A", "1", "a"), key("m", "B", "1", "b"), key("m", "C", "1", "c"));
    s.put(a.clone(), bank(1, 40)).unwrap();
    s.put(b.clone(), bank(1, 40)).unwrap();
    assert!(s.get(&a).is_some());                 // a is now newer than b
    let evicted = s.put(c.clone(), bank(1, 40)).unwrap();
    assert_eq!(evicted, vec![b.clone()]);
    assert!(s.get(&b).is_none() && s.get(&a).is_some() && s.get(&c).is_some());
    assert_eq!(s.bytes(), 80);
}

#[test]
fn a_bank_bigger_than_the_cap_is_refused_and_nothing_is_evicted() {
    let mut s = BankStore::new(100);
    let a = key("m", "A", "1", "a");
    s.put(a.clone(), bank(1, 60)).unwrap();
    assert!(s.put(key("m", "Big", "1", "big"), bank(1, 101)).is_err());
    assert!(s.get(&a).is_some());
    assert_eq!(s.bytes(), 60);
}

#[test]
fn banking_the_same_key_again_replaces_it_and_counts_it_once() {
    let mut s = BankStore::new(1000);
    let k = key("m", "A", "1", "a");
    s.put(k.clone(), bank(1, 100)).unwrap();
    s.put(k.clone(), bank(2, 150)).unwrap();
    assert_eq!(s.bytes(), 150);
    assert_eq!(s.get(&k).unwrap().tokens, 2);
    assert_eq!(s.len(), 1);
}

#[test]
fn dropping_a_model_drops_all_of_its_banks_and_only_those() {
    let mut s = BankStore::new(1000);
    s.put(key("m1", "A", "1", "a"), bank(1, 10)).unwrap();
    s.put(key("m1", "B", "1", "b"), bank(1, 10)).unwrap();
    let keep = key("m2", "A", "1", "a");
    s.put(keep.clone(), bank(1, 10)).unwrap();
    assert_eq!(s.drop_model("m1"), 2);
    assert_eq!((s.len(), s.bytes()), (1, 10));
    assert!(s.get(&keep).is_some());
}

#[test]
fn the_text_hash_tells_texts_apart_and_is_stable() {
    assert_eq!(hash_text("Zorvex lives in Lima."), hash_text("Zorvex lives in Lima."));
    assert_ne!(hash_text("Zorvex lives in Lima."), hash_text("Zorvex lives in Cusco."));
}

#[test]
fn asking_whether_a_bank_is_kept_does_not_count_as_using_it() {
    let mut s = BankStore::new(100);
    let (a, b, c) = (key("m", "A", "1", "a"), key("m", "B", "1", "b"), key("m", "C", "1", "c"));
    s.put(a.clone(), bank(1, 40)).unwrap();
    s.put(b.clone(), bank(1, 40)).unwrap();
    assert!(s.contains(&a) && s.contains(&b) && !s.contains(&c));
    // a was asked about, not used: it is still the oldest, so it is the one that goes
    let evicted = s.put(c.clone(), bank(1, 40)).unwrap();
    assert_eq!(evicted, vec![a]);
}
