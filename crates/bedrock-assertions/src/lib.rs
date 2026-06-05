// SPDX-License-Identifier: GPL-2.0

//! Assertion primitives for bedrock.
//!
//! An [`Assertion`] checks a [`Condition`] about guest execution. The
//! assertion variant ([`Assertion::Always`] / [`Assertion::Sometimes`])
//! determines how the condition is interpreted across evaluations. The
//! [`Condition`] carries the operands it was evaluated against (e.g. the `x`
//! and `y` of a `<` comparison) so each record is self-describing.
//!
//! Both types are serializable via serde. This crate does not run inside the
//! kernel module.

#[macro_use]
mod macros;
mod assertion;
mod condition;

pub use assertion::Assertion;
pub use condition::Condition;
