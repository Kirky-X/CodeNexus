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

# daemon: event loop lifecycle (src/daemon/daemon.rs)
daemon-signals-registered = Signal handlers registered
daemon-started = Daemon mode started
daemon-stop-signal-received = Stop signal received, daemon exiting
daemon-debounce-batch = Processing debounced event batch
daemon-watch-error = File watcher error
daemon-channel-disconnected = Event channel disconnected, daemon exiting
daemon-shutdown-phase-1 = Shutdown phase 1/3: stop accepting new file events
daemon-shutdown-phase-2 = Shutdown phase 2/3: drain debounced event queue
daemon-shutdown-phase-3 = Shutdown phase 3/3: close graph database connections
daemon-shutdown-complete = Phased shutdown complete

# graph-viewer server: auth / startup logs (graph-viewer/server/src/main.rs)
graph-host-forbidden = This service only allows local loopback access
graph-token-missing = Missing or incorrect x-graph-token header
graph-server-started = Graph server started: http://127.0.0.1:9800 (requests require the x-graph-token header, see stdout)
graph-project-discovered = Discovered project: { $name } -> { $path }
