// SPDX-License-Identifier: GPL-2.0

//! The [`Condition`] type: the thing an [`Assertion`](crate::Assertion) checks.

use serde::{Deserialize, Serialize};

/// A condition evaluated by an assertion, storing its operands.
///
/// Each comparison variant keeps the values it was evaluated against (`x` and
/// `y`) so the assertion record is self-describing — e.g. a failed `Lt` shows
/// exactly which `x` was not less than which `y`.
///
/// Operands are `i128` so the full `u64` and `i64` ranges are representable
/// without loss. The creation macros ([`always_lt!`](crate::always_lt) etc.)
/// accept any integer value up to `u64`.
///
/// More variants (`Eq`, `Ne`, …) will be added as needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Condition {
    /// A bare boolean, with no operands to compare.
    Bool(bool),
    /// `x < y`.
    Lt {
        /// Left-hand operand.
        x: i128,
        /// Right-hand operand.
        y: i128,
    },
    /// `x > y`.
    Gt {
        /// Left-hand operand.
        x: i128,
        /// Right-hand operand.
        y: i128,
    },
    /// `x <= y`.
    Lte {
        /// Left-hand operand.
        x: i128,
        /// Right-hand operand.
        y: i128,
    },
    /// `x >= y`.
    Gte {
        /// Left-hand operand.
        x: i128,
        /// Right-hand operand.
        y: i128,
    },
}

impl Condition {
    /// Evaluate the condition to its boolean result.
    pub fn evaluate(&self) -> bool {
        match self {
            Condition::Bool(b) => *b,
            Condition::Lt { x, y } => x < y,
            Condition::Gt { x, y } => x > y,
            Condition::Lte { x, y } => x <= y,
            Condition::Gte { x, y } => x >= y,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bool_evaluates_to_itself() {
        assert!(Condition::Bool(true).evaluate());
        assert!(!Condition::Bool(false).evaluate());
    }

    #[test]
    fn lt_compares_operands() {
        assert!(Condition::Lt { x: 1, y: 2 }.evaluate());
        assert!(!Condition::Lt { x: 2, y: 2 }.evaluate());
        assert!(!Condition::Lt { x: 3, y: 2 }.evaluate());
    }

    #[test]
    fn gt_compares_operands() {
        assert!(Condition::Gt { x: 3, y: 2 }.evaluate());
        assert!(!Condition::Gt { x: 2, y: 2 }.evaluate());
        assert!(!Condition::Gt { x: 1, y: 2 }.evaluate());
    }

    #[test]
    fn lte_compares_operands() {
        assert!(Condition::Lte { x: 1, y: 2 }.evaluate());
        assert!(Condition::Lte { x: 2, y: 2 }.evaluate());
        assert!(!Condition::Lte { x: 3, y: 2 }.evaluate());
    }

    #[test]
    fn gte_compares_operands() {
        assert!(Condition::Gte { x: 3, y: 2 }.evaluate());
        assert!(Condition::Gte { x: 2, y: 2 }.evaluate());
        assert!(!Condition::Gte { x: 1, y: 2 }.evaluate());
    }

    #[test]
    fn full_u64_range_is_representable() {
        let c = Condition::Lt {
            x: i128::from(u64::MAX),
            y: i128::from(u64::MAX),
        };
        assert!(!c.evaluate());
        assert_eq!(c.evaluate(), u64::MAX < u64::MAX);
    }

    #[test]
    fn operands_survive_serde_round_trip() {
        let c = Condition::Lt { x: -5, y: 7 };
        let json = serde_json::to_string(&c).unwrap();
        let back: Condition = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }
}
