//! OSC 8 hyperlink emission.
//!
//! A single helper so call sites don't open-code the escape sequence and
//! so the on/off gate lives in one place. Renderers can route every
//! linkable span through here unconditionally — when hyperlinks are off
//! it's an owned-copy of the input.

use super::policy::TermPolicy;

/// Wrap `text` in an OSC 8 hyperlink pointing at `target`.
///
/// When `policy.hyperlinks` is false this returns `text.to_owned()` —
/// call sites get a `String` either way so they don't have to branch.
#[must_use]
pub fn link(policy: TermPolicy, text: &str, target: &str) -> String {
    if !policy.hyperlinks {
        return text.to_owned();
    }
    // OSC 8 ; ; <uri> ST <text> OSC 8 ; ; ST
    format!("\x1b]8;;{target}\x1b\\{text}\x1b]8;;\x1b\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_emits_plain_when_disabled() {
        let p = TermPolicy::plain(); // hyperlinks = false
        assert_eq!(link(p, "m17", "brain://recall/m17"), "m17");
    }

    #[test]
    fn link_emits_osc8_when_enabled() {
        let mut p = TermPolicy::plain();
        p.hyperlinks = true;
        let out = link(p, "m17", "brain://recall/m17");
        // The link wraps the text in OSC 8 ; ; <uri> ST … ST.
        assert!(out.starts_with("\x1b]8;;brain://recall/m17\x1b\\"));
        assert!(out.contains("m17"));
        assert!(out.ends_with("\x1b]8;;\x1b\\"));
    }
}
