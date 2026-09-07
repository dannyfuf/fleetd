//! Cursor motion shared by the list-like components: wrap-around next/previous that stays
//! in bounds after the collection shrinks.

pub(super) fn next(index: usize, len: usize) -> usize {
    if len == 0 || index >= len - 1 {
        0
    } else {
        index + 1
    }
}

pub(super) fn previous(index: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let index = index.min(len - 1);
    if index == 0 { len - 1 } else { index - 1 }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shrinkage_and_empty_lists_keep_navigation_in_bounds() {
        assert_eq!(next(usize::MAX, 3), 0);
        assert_eq!(previous(99, 3), 1);
        assert_eq!(next(0, 0), 0);
        assert_eq!(previous(99, 0), 0);
        assert_eq!(previous(0, 3), 2);
    }
}
