# CodeNexus user-facing messages (en).
# Key parity with zh/messages.ftl is guarded by test_key_parity in
# src/i18n/catalog.rs; embedding is guarded by embedded_locales_cover_locales_dir.

# daemon: impact notifications (src/daemon/impact_observer.rs)
impact-db-open-failed = Impact alert: failed to open graph store, skipping this batch of notifications
impact-notice = Change impact alert
impact-webhook-client-failed = Impact alert webhook: failed to build HTTP client
impact-webhook-non-2xx = Impact alert webhook returned non-2xx status
impact-webhook-send-failed = Impact alert webhook failed to send

# daemon: incremental indexing (src/daemon/index_observer.rs)
index-incremental-triggered = Incremental indexing triggered
index-incremental-failed = Incremental indexing failed, continuing to watch

# graph-viewer server (axum) error responses
project-param-required = The project parameter is required
project-not-found = Project '{ $name }' not found
node-not-found = Node '{ $name }' not found
