//! Which parts of a request are served from banks (docs/design.md, "The bank": the limit of two, and the placement).
use membrama::turn::{bank_plan, Part, PartKind};

fn text(t: &str) -> Part { Part { kind: PartKind::Text, text: t.into() } }
fn ent(e: &str) -> Part { Part { kind: PartKind::Entity { eid: e.into(), version: "v1".into() }, text: format!("{e} lives somewhere.") } }

#[test]
fn text_parts_are_never_banked() {
    assert_eq!(bank_plan(&[text("a"), text("b")], 2), Vec::<usize>::new());
}

#[test]
fn up_to_the_limit_every_entity_part_is_banked() {
    assert_eq!(bank_plan(&[text("pre"), ent("A"), text("mid"), ent("B"), text("post")], 2), vec![1, 3]);
}

#[test]
fn beyond_the_limit_the_last_entity_parts_are_banked_and_the_earlier_ones_are_read_as_text() {
    assert_eq!(bank_plan(&[ent("A"), ent("B"), ent("C"), text("q")], 2), vec![1, 2]);
}

#[test]
fn a_limit_of_zero_banks_nothing() {
    assert_eq!(bank_plan(&[ent("A")], 0), Vec::<usize>::new());
}
