//! File-watching daemon (Observer pattern).
//!
//! Uses [`notify_debouncer_full`] (ADR-013) to watch repositories and trigger
//! incremental indexing with configurable debounce (BR-DAEMON-001/004).
