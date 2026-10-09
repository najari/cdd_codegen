//! The little arithmetic generated code needs and `core` does not have.

/// Rounds to the nearest integer, halves away from zero (what `f64::round` does). `x` must be
/// finite and below 1e30 in magnitude; the generated code checks before it calls.
pub fn round_ties_away(x: f64) -> f64 {
    // Truncation toward zero, then the fraction left over: exact for every |x| below 2^52, and
    // above that `x` is already an integer.
    let t = x as i128 as f64;
    let d = x - t;
    if d >= 0.5 {
        t + 1.0
    } else if d <= -0.5 {
        t - 1.0
    } else {
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halves_go_away_from_zero_and_the_edge_case_is_right() {
        assert_eq!(round_ties_away(2.5), 3.0);
        assert_eq!(round_ties_away(-2.5), -3.0);
        assert_eq!(round_ties_away(2.4999), 2.0);
        assert_eq!(round_ties_away(-0.4), 0.0);
        // 0.49999999999999994 + 0.5 rounds to 1.0 in floating point; the right answer is 0.
        assert_eq!(round_ties_away(0.499_999_999_999_999_94), 0.0);
        assert_eq!(round_ties_away(125.0), 125.0);
        assert_eq!(round_ties_away(1e20), 1e20);
    }

    #[test]
    fn agrees_with_the_standard_library() {
        let mut x = -300.0;
        while x < 300.0 {
            assert_eq!(round_ties_away(x), x.round(), "x = {x}");
            x += 0.0625;
        }
    }
}
