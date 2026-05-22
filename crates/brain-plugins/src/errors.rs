//! Plugin error taxonomy.
//!
//! All plugin entry points return [`PluginResult`]. The host translates
//! errors into structured tracing events and applies the fail-open
//! pipeline policy — a plugin error never aborts the host pipeline.

use thiserror::Error;

/// Result alias used across the plugin surface.
pub type PluginResult<T> = std::result::Result<T, PluginError>;

/// All errors a plugin or the host can produce.
#[derive(Debug, Error)]
pub enum PluginError {
    /// Configuration handed to `initialize` is missing required keys or
    /// has the wrong shape. Returned during registration; the host
    /// rejects the registration so a malformed plugin never executes.
    #[error("plugin {plugin_id}: invalid configuration: {message}")]
    InvalidConfig {
        plugin_id: &'static str,
        message: String,
    },

    /// Two plugins were registered with the same `plugin_id`. Plugin ids
    /// are the audit-trail key; collisions make audit ambiguous, so we
    /// reject the second registration.
    #[error("plugin id `{plugin_id}` is already registered")]
    DuplicateId { plugin_id: &'static str },

    /// Connector lookup miss — the caller asked to fetch from a
    /// connector id that wasn't registered.
    #[error("connector `{id}` is not registered")]
    ConnectorNotFound { id: String },

    /// Plugin panicked inside its body. The host catches the panic so
    /// the pipeline can continue with un-enriched items; the variant
    /// carries the panic message for the audit row.
    #[error("plugin {plugin_id} panicked: {message}")]
    Panicked {
        plugin_id: &'static str,
        message: String,
    },

    /// Plugin reported a logical failure — e.g. upstream API timeout,
    /// schema mismatch, configured budget exhausted. The host logs and
    /// (for enrichers) continues with un-enriched items.
    #[error("plugin {plugin_id} failed: {message}")]
    Failed {
        plugin_id: &'static str,
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_config_message_carries_plugin_id() {
        let e = PluginError::InvalidConfig {
            plugin_id: "test:foo",
            message: "missing api_key".into(),
        };
        let s = e.to_string();
        assert!(s.contains("test:foo"));
        assert!(s.contains("missing api_key"));
    }

    #[test]
    fn duplicate_id_renders_plugin_id() {
        let e = PluginError::DuplicateId {
            plugin_id: "test:dup",
        };
        assert!(e.to_string().contains("test:dup"));
    }

    #[test]
    fn connector_not_found_renders_id() {
        let e = PluginError::ConnectorNotFound {
            id: "gdrive".into(),
        };
        assert!(e.to_string().contains("gdrive"));
    }
}
