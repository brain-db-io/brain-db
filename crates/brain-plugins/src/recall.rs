//! Base lifecycle trait shared by every plugin kind.
//!
//! The `RecallPlugin` name reflects that the Brain primitives are
//! cognitive ops (encode / recall / plan / reason / forget) — every
//! plugin participates in the recall pathway, even when its concrete
//! hook is on the encode side. Concrete plugin kinds extend this trait
//! with their own per-hook entry points.

use crate::errors::PluginResult;

/// Lifecycle hooks every plugin must implement.
///
/// Plugins run on the writer's executor only. They MUST NOT spawn
/// threads, hold blocking IO, or block on tokio tasks — doing so will
/// stall the shard's Glommio executor. Use plugin-side caching and
/// pre-built indexes for any non-trivial work.
pub trait RecallPlugin: Send + Sync {
    /// Stable, globally unique id for this plugin instance. Used as the
    /// audit-trail key and as the registration de-duplication key.
    ///
    /// Convention: `vendor:plugin-name`, lowercase ASCII, kebab-case
    /// after the colon. Example: `"acme:slack-connector"`.
    fn plugin_id(&self) -> &'static str;

    /// Human-readable name for logs and admin output. May contain
    /// whitespace; it is never used as a registry key.
    fn plugin_name(&self) -> &'static str;

    /// Called once at registry build time. The plugin should validate
    /// its configuration and fail fast for malformed input. Errors here
    /// block registration so a misconfigured plugin never runs.
    fn initialize(&self, config: &serde_json::Value) -> PluginResult<()>;

    /// Optional lifecycle hook called on shard shutdown. Use for
    /// flushing plugin-side buffers. Errors are logged at warn but do
    /// not block shutdown.
    fn shutdown(&self) -> PluginResult<()> {
        Ok(())
    }
}
