// SPDX-License-Identifier: GPL-2.0

//! Convenience macros for constructing [`Assertion`](crate::Assertion)s.
//!
//! Two families are provided, one per [`Assertion`](crate::Assertion) variant:
//! `always_*` builds [`Assertion::Always`](crate::Assertion::Always) and
//! `sometimes_*` builds [`Assertion::Sometimes`](crate::Assertion::Sometimes).
//!
//! Comparison operands accept any integer value up to `u64`: they are converted
//! with [`i128::from`], which accepts every signed/unsigned integer type
//! through `u64`/`i64` and rejects anything wider (e.g. `u128`) at compile
//! time.

/// Internal: generate one comparison macro that builds an assertion of the
/// given variant for a given [`Condition`](crate::Condition).
///
/// `$d` threads a literal `$` into the generated macro so its metavariables
/// don't clash with this generator's own parser (same trick as `bedrock-log`).
macro_rules! define_cmp_macro {
    ($d:tt $(#[$doc:meta])* $name:ident => $ctor:ident, $variant:ident) => {
        $(#[$doc])*
        #[macro_export]
        macro_rules! $name {
            ($d x:expr, $d y:expr $d(,)?) => {
                $crate::Assertion::$ctor($crate::Condition::$variant {
                    x: i128::from($d x),
                    y: i128::from($d y),
                })
            };
        }
    };
}

define_cmp_macro!($
    /// Build an `Always` assertion over `x < y`.
    ///
    /// ```
    /// use bedrock_assertions::always_lt;
    /// assert!(always_lt!(1u64, 2u64).holds());
    /// ```
    always_lt => always, Lt);

define_cmp_macro!($
    /// Build a `Sometimes` assertion over `x < y`.
    ///
    /// ```
    /// use bedrock_assertions::{sometimes_lt, Assertion, Condition};
    /// assert_eq!(sometimes_lt!(1u64, 2u64).condition(), Condition::Lt { x: 1, y: 2 });
    /// ```
    sometimes_lt => sometimes, Lt);

define_cmp_macro!($
    /// Build an `Always` assertion over `x > y`.
    ///
    /// ```
    /// use bedrock_assertions::always_gt;
    /// assert!(always_gt!(2u64, 1u64).holds());
    /// ```
    always_gt => always, Gt);

define_cmp_macro!($
    /// Build a `Sometimes` assertion over `x > y`.
    ///
    /// ```
    /// use bedrock_assertions::{sometimes_gt, Condition};
    /// assert_eq!(sometimes_gt!(2u64, 1u64).condition(), Condition::Gt { x: 2, y: 1 });
    /// ```
    sometimes_gt => sometimes, Gt);

define_cmp_macro!($
    /// Build an `Always` assertion over `x <= y`.
    ///
    /// ```
    /// use bedrock_assertions::always_lte;
    /// assert!(always_lte!(2u64, 2u64).holds());
    /// ```
    always_lte => always, Lte);

define_cmp_macro!($
    /// Build a `Sometimes` assertion over `x <= y`.
    ///
    /// ```
    /// use bedrock_assertions::{sometimes_lte, Condition};
    /// assert_eq!(sometimes_lte!(2u64, 2u64).condition(), Condition::Lte { x: 2, y: 2 });
    /// ```
    sometimes_lte => sometimes, Lte);

define_cmp_macro!($
    /// Build an `Always` assertion over `x >= y`.
    ///
    /// ```
    /// use bedrock_assertions::always_gte;
    /// assert!(always_gte!(2u64, 2u64).holds());
    /// ```
    always_gte => always, Gte);

define_cmp_macro!($
    /// Build a `Sometimes` assertion over `x >= y`.
    ///
    /// ```
    /// use bedrock_assertions::{sometimes_gte, Condition};
    /// assert_eq!(sometimes_gte!(2u64, 2u64).condition(), Condition::Gte { x: 2, y: 2 });
    /// ```
    sometimes_gte => sometimes, Gte);

/// Build an `Always` assertion over a boolean condition.
///
/// ```
/// use bedrock_assertions::always_bool;
/// assert!(always_bool!(2 + 2 == 4).holds());
/// ```
#[macro_export]
macro_rules! always_bool {
    ($cond:expr) => {
        $crate::Assertion::always($crate::Condition::Bool($cond))
    };
}

/// Build a `Sometimes` assertion over a boolean condition.
///
/// ```
/// use bedrock_assertions::{sometimes_bool, Condition};
/// assert_eq!(sometimes_bool!(true).condition(), Condition::Bool(true));
/// ```
#[macro_export]
macro_rules! sometimes_bool {
    ($cond:expr) => {
        $crate::Assertion::sometimes($crate::Condition::Bool($cond))
    };
}
