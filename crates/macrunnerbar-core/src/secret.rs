//! `Secret<T>`: a value that never prints its content.

use std::fmt;

const REDACTED: &str = "[redacted]";

/// Holds a credential. `Debug` and `Display` print `[redacted]`; the content is reachable only
/// through [`Secret::expose`]. There is deliberately no `Serialize` and no `PartialEq` on content.
#[derive(Clone)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    /// Wrap a value.
    pub fn new(value: T) -> Self {
        Self(value)
    }

    /// The only way to read the content. Call it at the point of use, never to log or store.
    pub fn expose(&self) -> &T {
        &self.0
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

impl<T> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SENTINEL: &str = "ghp_SENTINEL_do_not_print_0123456789";

    #[test]
    fn debug_and_display_never_print_content() {
        let s = Secret::new(SENTINEL.to_string());
        assert_eq!(format!("{s:?}"), "[redacted]");
        assert_eq!(format!("{s}"), "[redacted]");
        assert_eq!(format!("{s:#?}"), "[redacted]");
        assert_eq!(s.expose(), SENTINEL);
    }

    #[test]
    fn nested_in_derived_debug_stays_redacted() {
        #[derive(Debug)]
        struct Holder {
            _token: Secret<String>,
        }
        let h = Holder {
            _token: Secret::new(SENTINEL.to_string()),
        };
        assert!(!format!("{h:?}").contains(SENTINEL));
        assert!(!format!("{h:#?}").contains(SENTINEL));
        let opt = Some(Secret::new(SENTINEL.to_string()));
        assert!(!format!("{opt:?}").contains(SENTINEL));
    }
}
