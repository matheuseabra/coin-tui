//! Pure refresh transition rules.

pub(crate) fn next_generation(current: u64) -> u64 {
    current.wrapping_add(1)
}
