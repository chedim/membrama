//! Reusing the live sequence across requests (docs/design.md): the tokens of a new prompt that equal the start of what the cache holds are not read again.
use membrama::engine::common_prefix_len;

#[test]
fn the_shared_start_is_counted() {
    assert_eq!(common_prefix_len(&[1, 2, 3, 4], &[1, 2, 9, 4]), 2);
    assert_eq!(common_prefix_len(&[1, 2, 3], &[1, 2, 3, 4, 5]), 3);
    assert_eq!(common_prefix_len(&[1, 2, 3, 4, 5], &[1, 2, 3]), 3);
}

#[test]
fn nothing_shared_is_zero_and_empty_is_zero() {
    assert_eq!(common_prefix_len(&[1, 2], &[3, 4]), 0);
    assert_eq!(common_prefix_len::<i32>(&[], &[1]), 0);
    assert_eq!(common_prefix_len::<i32>(&[], &[]), 0);
}
