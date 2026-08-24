//! One shape for a failure, wherever it is reported.
//!
//! Errors in this crate are layered: a deploy fails because Docker failed,
//! because a compose command failed, because the daemon is not running. Each
//! layer states only its own failure and delegates the rest to `source()`, so
//! the reader can tell the Platform's framing apart from what Docker said.
//!
//! `ErrorReport` is that chain flattened once, at the edge — the HTTP body,
//! the stored `last_error`, the log line — and never re-parsed back into
//! layers afterwards.

use serde::{Deserialize, Serialize};

/// A failure and the causes underneath it, outermost first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorReport {
    /// What failed, from the layer the caller asked to do something.
    pub error: String,
    /// Why, one entry per layer below, nearest cause first.
    #[serde(default)]
    pub caused_by: Vec<String>,
}

impl ErrorReport {
    /// Walks the `source()` chain of an error into a report.
    pub fn new(err: &dyn std::error::Error) -> Self {
        let mut caused_by = Vec::new();
        let mut cause = err.source();
        while let Some(e) = cause {
            caused_by.push(e.to_string());
            cause = e.source();
        }
        ErrorReport {
            error: err.to_string(),
            caused_by,
        }
    }

    /// A failure with nothing underneath it — the Platform itself gave up,
    /// with no lower layer to blame.
    pub fn plain(message: impl Into<String>) -> Self {
        ErrorReport {
            error: message.into(),
            caused_by: Vec::new(),
        }
    }
}

/// Renders the report the way `anyhow` renders a chain, so a log line and a
/// terminal say the same thing.
impl std::fmt::Display for ErrorReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.error)?;
        if self.caused_by.is_empty() {
            return Ok(());
        }
        write!(f, "\n\nCaused by:")?;
        for (i, cause) in self.caused_by.iter().enumerate() {
            write!(f, "\n{i:>5}: {cause}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Layer(&'static str, Option<Box<Layer>>);

    impl std::fmt::Display for Layer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    impl std::error::Error for Layer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.1.as_deref().map(|e| e as &dyn std::error::Error)
        }
    }

    #[test]
    fn a_chain_keeps_every_layer_apart() {
        let err = Layer(
            "System initialization aborted",
            Some(Box::new(Layer(
                "Failed to load application configuration",
                Some(Box::new(Layer("No such file or directory", None))),
            ))),
        );

        let report = ErrorReport::new(&err);

        assert_eq!(report.error, "System initialization aborted");
        assert_eq!(
            report.caused_by,
            [
                "Failed to load application configuration",
                "No such file or directory"
            ]
        );
    }

    #[test]
    fn a_lone_error_has_no_causes() {
        let report = ErrorReport::new(&Layer("nothing underneath", None));

        assert!(report.caused_by.is_empty());
        assert_eq!(report.to_string(), "nothing underneath");
    }

    #[test]
    fn rendering_indents_the_causes_under_the_failure() {
        let report = ErrorReport {
            error: "System initialization aborted".into(),
            caused_by: vec![
                "Failed to load application configuration".into(),
                "No such file or directory (os error 2)".into(),
            ],
        };

        assert_eq!(
            report.to_string(),
            "System initialization aborted\n\n\
             Caused by:\n\
             \x20   0: Failed to load application configuration\n\
             \x20   1: No such file or directory (os error 2)"
        );
    }
}
