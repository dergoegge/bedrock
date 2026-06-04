// SPDX-License-Identifier: GPL-2.0

//! ANSI stripping for serial lines.
//!
//! journald colorizes the `[source]` tag in each serial line. Solution
//! detection matches markers against the raw text, so it strips the color
//! first via [`strip_ansi`].

use std::borrow::Cow;

/// Strip ANSI CSI escape sequences from `s`. Returns `Cow::Borrowed` when the
/// input is escape-free (the common case), so the cost is one byte scan;
/// allocation happens only when a `\x1b[` is found.
pub fn strip_ansi(s: &str) -> Cow<'_, str> {
    if !s.contains('\x1b') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'[') {
            // CSI: skip `\x1b[`, parameter/intermediate bytes, one final byte.
            i += 2;
            while let Some(&b) = bytes.get(i) {
                i += 1;
                if (0x40..=0x7e).contains(&b) {
                    break;
                }
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_drops_csi() {
        let s = "[br 11 vt 59.026] \x1b[1;34m[podman]\x1b[0m | died";
        assert_eq!(strip_ansi(s), "[br 11 vt 59.026] [podman] | died");
        assert!(matches!(strip_ansi("plain line"), Cow::Borrowed(_)));
    }
}
