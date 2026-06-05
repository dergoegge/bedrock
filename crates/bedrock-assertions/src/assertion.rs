// SPDX-License-Identifier: GPL-2.0

//! The [`Assertion`] type and its evaluation logic.

use serde::{Deserialize, Serialize};

use crate::Condition;

/// A property checked about guest execution, carrying the [`Condition`] it was
/// evaluated against.
///
/// The variant determines how the stored condition is interpreted across the
/// (many) times an assertion of this kind is recorded:
///
/// - [`Assertion::Always`] — the condition must hold on *every* evaluation. A
///   single failure is a violation.
/// - [`Assertion::Sometimes`] — the condition must hold on *at least one*
///   evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Assertion {
    /// The condition must hold every time the assertion is evaluated.
    Always {
        /// The condition that was evaluated.
        condition: Condition,
    },
    /// The condition must hold at least once across all evaluations.
    Sometimes {
        /// The condition that was evaluated.
        condition: Condition,
    },
}

impl Assertion {
    /// Create an [`Assertion::Always`] recording the given `condition`.
    pub fn always(condition: Condition) -> Self {
        Assertion::Always { condition }
    }

    /// Create an [`Assertion::Sometimes`] recording the given `condition`.
    pub fn sometimes(condition: Condition) -> Self {
        Assertion::Sometimes { condition }
    }

    /// The [`Condition`] recorded on this assertion.
    pub fn condition(&self) -> Condition {
        match self {
            Assertion::Always { condition } | Assertion::Sometimes { condition } => *condition,
        }
    }

    /// Whether the recorded [`Condition`] was satisfied for this evaluation.
    ///
    /// This is a per-record check and is the same for both variants — it simply
    /// evaluates the stored condition. The distinction between `Always` (every
    /// evaluation must hold) and `Sometimes` (at least one must hold) is a
    /// property of the *set* of records and is resolved by a collector that
    /// aggregates them, not by a single assertion.
    pub fn holds(&self) -> bool {
        self.condition().evaluate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_holds_when_condition_true() {
        assert!(Assertion::always(Condition::Bool(true)).holds());
        assert!(Assertion::always(Condition::Lt { x: 1, y: 2 }).holds());
    }

    #[test]
    fn always_violated_when_condition_false() {
        assert!(!Assertion::always(Condition::Bool(false)).holds());
        assert!(!Assertion::always(Condition::Gt { x: 1, y: 2 }).holds());
    }

    #[test]
    fn condition_is_saved() {
        let c = Condition::Lt { x: 3, y: 4 };
        assert_eq!(Assertion::always(c).condition(), c);
    }

    #[test]
    fn sometimes_holds_evaluates_condition() {
        assert!(Assertion::sometimes(Condition::Bool(true)).holds());
        assert!(!Assertion::sometimes(Condition::Bool(false)).holds());
        assert!(Assertion::sometimes(Condition::Lt { x: 1, y: 2 }).holds());
        assert!(!Assertion::sometimes(Condition::Gt { x: 1, y: 2 }).holds());
    }

    #[test]
    fn round_trips_through_serde() {
        let a = Assertion::always(Condition::Gt { x: 9, y: 2 });
        let json = serde_json::to_string(&a).unwrap();
        let back: Assertion = serde_json::from_str(&json).unwrap();
        assert_eq!(a, back);
    }
}
