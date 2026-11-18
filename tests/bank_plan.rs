//! Which parts of a request are served from banks (docs/design.md, "The bank": the limit, and all or nothing).
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
fn beyond_the_limit_nothing_is_banked_the_request_is_read_as_text() {
    // a bank among other entities' text, or beside another bank, answered about the wrong entity (docs/design.md): all banked or none
    assert_eq!(bank_plan(&[ent("A"), ent("B"), ent("C"), text("q")], 2), Vec::<usize>::new());
}

#[test]
fn a_limit_of_zero_banks_nothing() {
    assert_eq!(bank_plan(&[ent("A")], 0), Vec::<usize>::new());
}
