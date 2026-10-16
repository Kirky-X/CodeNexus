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

# viewer: auth / startup logs (src/viewer/server.rs)
graph-host-forbidden = This service only allows local loopback access
graph-token-missing = Missing or incorrect x-graph-token header
graph-server-started = Graph server started: http://127.0.0.1:{ $port } (requests require the x-graph-token header, see stdout)
graph-server-auth-token = graph-server auth token: { $auth_token }
graph-project-discovered = Discovered project: { $name } -> { $path }
graph-asset-bad-path = Bad path
graph-asset-not-found = Asset not found

# viewer: graph query diagnostics (src/viewer/graph.rs)
graph-node-query-info = Queried { $types } node types, got { $nodes } nodes
graph-edge-scan-info = Edge scan: { $rows } rows, { $edges } edges matched, { $total } total nodes

# viewer: embedded graph-viewer (src/viewer/assets.rs)
graph-viewer-dist-missing = graph-viewer frontend assets are not built: run "npm run build" in graph-viewer/ and rebuild this binary.
graph-viewer-fallback-title = CodeNexus graph-viewer
graph-viewer-fallback-heading = CodeNexus graph-viewer

# logging: inklog level presets (src/logging.rs)
logging-invalid_target_level = Ignoring invalid CODENEXUS_LOG_TARGETS entry '{ $entry }' (expected target=level, e.g. hyper=warn)
logging-invalid_sample_entry = Ignoring invalid CODENEXUS_LOG_SAMPLE entry '{ $entry }' (expected level=N or prefix=N[@level], e.g. debug=10 or sqlx=100@warn)
logging-invalid_rate_limit_entry = Ignoring invalid CODENEXUS_LOG_RATE_LIMIT entry '{ $entry }' (expected prefix=rate, e.g. lbug=50)

# embed: model integrity digest validation (src/embed/config.rs)
embed-invalid_model_sha256 = Ignoring invalid CODENEXUS_EMBED_MODEL_SHA256 value '{ $value }' (expected a 64-character hex digest; integrity verification stays disabled)

# embed: local model bundle pre-checks (src/embed/client.rs)
embed-model-not-found = local embedding model not found at { $path }. Place the arctic-embed-xs model at this path (or set the { $env } env var to a custom location).
embed-model-config-missing = model config.json not found at { $path }.
embed-model-weights-missing = model weights not found at { $path } (expected model.safetensors; pytorch_model.bin is not supported by the vecboost Candle engine).
embed-runtime-build-failed = failed to build tokio runtime: { $reason }
embed-runtime-thread-panic = vecboost runtime thread panicked

# cli: usage errors (src/main.rs)
cli-no-subcommand = no subcommand supplied (run `codenexus --help` to list commands)
