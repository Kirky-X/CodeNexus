// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Pipeline DAG runner (T9 H2, design.md D2).
//!
//! Replaces the manual 9-step sequence in [`super::pipeline`] with a typed
//! [`Phase`] trait and a [`Pipeline`] runner that uses Kahn's topological sort
//! to determine execution order and detects cycles (fail-loud, Rule 12).
//!
//! # Design
//!
//! Each [`Phase`] declares:
//! - `type Input` / `type Output` — typed values flowing between phases.
//! - `const NAME` — unique phase identifier (used as the context key).
//! - `fn deps()` — names of phases whose outputs this phase consumes.
//! - `fn run(input, ctx)` — executes the phase, reading dep outputs from
//!   [`PipelineCtx`] as needed.
//!
//! The [`Pipeline`] runner:
//! 1. Registers phases by name.
//! 2. Topologically sorts phases via Kahn's algorithm (deterministic
//!    alphabetical order among ready phases).
//! 3. Detects cycles and returns [`PhaseError::Cycle`].
//! 4. Executes each phase in order: extracts `Input` from the context (keyed
//!    by the phase's own `NAME`), calls `run`, stores `Output` back under
//!    `NAME`.
//!
//! # Input wiring
//!
//! - **Root phases** (no deps): the caller inserts the typed `Input` into
//!   [`PipelineCtx`] under the phase's `NAME` before calling [`Pipeline::run`].
//! - **Derived phases**: set `Input = ()` (caller inserts `()`) and read dep
//!   outputs from `ctx` inside `run` via [`PipelineCtx::get`]. This keeps the
//!   `Phase` trait signature faithful to design.md D2 (5 items, no
//!   `build_input` method) while supporting multi-dep wiring.
//!
//! Task 2.5 refactors the existing pipeline steps into typed `Phase`
//! implementations using this runner.

use std::any::Any;
use std::collections::{BTreeSet, HashMap};

use thiserror::Error;

/// Errors raised by the pipeline DAG runner (T9 H2).
#[derive(Debug, Error)]
pub enum PhaseError {
    /// A cycle was detected in the phase dependency graph.
    ///
    /// The message lists the phases involved in the cycle (fail-loud,
    /// Rule 12 — never silently skip a cyclic phase).
    #[error("cycle detected in pipeline DAG involving phases: [{0}]")]
    Cycle(String),

    /// A phase declared a dependency on a name that is not registered.
    #[error("phase `{phase}` declares missing dependency `{dep}`")]
    MissingDependency {
        /// The dependent phase name.
        phase: &'static str,
        /// The unregistered dependency name.
        dep: &'static str,
    },

    /// A phase was registered with a name that is already in use.
    #[error("duplicate phase registration: `{0}`")]
    DuplicatePhase(&'static str),

    /// The phase's `Input` was not found in the context under its `NAME`.
    ///
    /// The caller must insert the input before running the pipeline
    /// (see [Input wiring](#input-wiring)).
    #[error("missing input for phase `{0}` — insert it into PipelineCtx before running")]
    MissingInput(&'static str),

    /// A value stored under a name did not match the requested type.
    ///
    /// This indicates a wiring bug: a phase expected `T` but the context
    /// held a different type under the same key.
    #[error("type mismatch for value keyed under `{0}`")]
    TypeMismatch(&'static str),

    /// A phase's `run` returned an error. The phase name and the underlying
    /// error are captured for diagnostics.
    ///
    /// The error is stored as `Box<dyn Error + Send + Sync>` so callers can
    /// downcast it back to the original error type (e.g. `IndexError`) to
    /// preserve specific error variants and exit codes (Rule 12: fail loud).
    #[error("phase `{phase}` failed: {inner}")]
    ExecutionFailed {
        /// The phase that raised the error.
        phase: &'static str,
        /// The underlying error.
        inner: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
}

/// Type-erased storage for pipeline intermediate values (design.md D2 risk
/// mitigation: "Pipeline 内部用 `Box<dyn Any>` 存储 intermediate results").
///
/// Each value is keyed by the producing phase's `NAME`. Phases read dep
/// outputs via [`PipelineCtx::get`] and consume their own `Input` via
/// [`PipelineCtx::remove`].
pub struct PipelineCtx {
    values: HashMap<&'static str, Box<dyn Any + Send + Sync>>,
}

impl PipelineCtx {
    /// Creates an empty context.
    #[must_use]
    pub fn new() -> Self {
        Self {
            values: HashMap::new(),
        }
    }

    /// Inserts `value` under `name`, replacing any existing value.
    ///
    /// Used by the caller to provide root-phase inputs before running, and
    /// by the runner to store each phase's `Output`.
    pub fn insert<T>(&mut self, name: &'static str, value: T)
    where
        T: Any + Send + Sync,
    {
        self.values.insert(name, Box::new(value));
    }

    /// Returns a shared reference to the value of type `T` stored under
    /// `name`, if present and type-compatible.
    ///
    /// Used by derived phases to read dep outputs inside `run` without
    /// consuming them (so sibling phases can also read the same dep).
    pub fn get<T>(&self, name: &str) -> Option<&T>
    where
        T: Any + Send + Sync,
    {
        self.values
            .get(name)
            .and_then(|v| v.downcast_ref::<T>())
    }

    /// Removes and returns the value of type `T` stored under `name`, if
    /// present and type-compatible.
    ///
    /// Used by the runner to extract a phase's `Input` before calling `run`.
    /// If the stored value's type does not match `T`, the entry is left
    /// intact (Rule 12: a wiring bug must not silently destroy data).
    pub fn remove<T>(&mut self, name: &str) -> Option<T>
    where
        T: Any + Send + Sync,
    {
        // Type-check before removing so a type mismatch leaves the entry
        // intact rather than dropping the boxed value.
        if !self.values.get(name).is_some_and(|v| v.is::<T>()) {
            return None;
        }
        self.values
            .remove(name)
            .and_then(|v| v.downcast::<T>().ok().map(|b| *b))
    }

    /// Returns `true` if a value is stored under `name`.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }

    /// Returns the number of stored values.
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Returns `true` if no values are stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

impl Default for PipelineCtx {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for PipelineCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PipelineCtx")
            .field("keys", &self.values.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// A phase in the indexing pipeline DAG (T9 H2, design.md D2).
///
/// Each phase declares its typed `Input`/`Output`, a unique `NAME`, and the
/// names of phases it depends on. The [`Pipeline`] runner executes phases in
/// dependency order, passing typed values via [`PipelineCtx`].
///
/// See the [module docs](crate::index::pipeline_dag) for input wiring rules.
pub trait Phase: Send + Sync + 'static {
    /// The typed input consumed by this phase.
    ///
    /// For root phases, this is the externally-provided input. For derived
    /// phases, use `()` and read dep outputs from the context in [`run`](Self::run).
    type Input: Any + Send + Sync + 'static;

    /// The typed output produced by this phase.
    type Output: Any + Send + Sync + 'static;

    /// Unique phase name. Used as the context key for both `Input` and `Output`.
    const NAME: &'static str;

    /// Names of phases whose outputs this phase consumes.
    ///
    /// Returns a `&'static [&'static str]` so the runner can reference the
    /// slice without allocation (design.md D2: "name slice 而非类型集合").
    fn deps() -> &'static [&'static str];

    /// Executes the phase.
    ///
    /// # Arguments
    ///
    /// * `input` - The typed input (extracted from the context under
    ///   [`NAME`](Self::NAME)).
    /// * `ctx` - Shared context for reading dep outputs via [`PipelineCtx::get`].
    ///
    /// # Errors
    ///
    /// Returns [`PhaseError`] on failure. The runner wraps non-`PhaseError`
    /// failures into [`PhaseError::ExecutionFailed`].
    fn run(&self, input: Self::Input, ctx: &PipelineCtx) -> Result<Self::Output, PhaseError>;
}

/// Type-erased phase wrapper for the registry.
///
/// Stored as `Box<dyn ErasedPhase>` so the runner can hold heterogeneous
/// `Phase` implementations in a single collection.
trait ErasedPhase: Send + Sync + 'static {
    fn deps(&self) -> &'static [&'static str];
    fn execute(&self, ctx: &mut PipelineCtx) -> Result<(), PhaseError>;
}

impl<P> ErasedPhase for P
where
    P: Phase,
{
    fn deps(&self) -> &'static [&'static str] {
        P::deps()
    }

    fn execute(&self, ctx: &mut PipelineCtx) -> Result<(), PhaseError> {
        let input = ctx
            .remove::<P::Input>(P::NAME)
            .ok_or(PhaseError::MissingInput(P::NAME))?;
        let output = self.run(input, ctx)?;
        ctx.insert(P::NAME, output);
        Ok(())
    }
}

/// Pipeline runner with Kahn topological sort and cycle detection (T9 H2).
///
/// Phases are registered by name; [`Pipeline::run`] topologically sorts them
/// (deterministic alphabetical order among ready phases) and executes each in
/// order, passing typed values through a shared [`PipelineCtx`].
pub struct Pipeline {
    phases: HashMap<&'static str, Box<dyn ErasedPhase>>,
}

impl Pipeline {
    /// Creates an empty pipeline.
    #[must_use]
    pub fn new() -> Self {
        Self {
            phases: HashMap::new(),
        }
    }

    /// Registers `phase` under its [`Phase::NAME`].
    ///
    /// # Errors
    ///
    /// Returns [`PhaseError::DuplicatePhase`] if a phase with the same name is
    /// already registered (fail-loud, Rule 12).
    pub fn register<P>(&mut self, phase: P) -> Result<(), PhaseError>
    where
        P: Phase,
    {
        if self.phases.contains_key(P::NAME) {
            return Err(PhaseError::DuplicatePhase(P::NAME));
        }
        self.phases.insert(P::NAME, Box::new(phase));
        Ok(())
    }

    /// Returns `true` if a phase named `name` is registered.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.phases.contains_key(name)
    }

    /// Returns the number of registered phases.
    #[must_use]
    pub fn len(&self) -> usize {
        self.phases.len()
    }

    /// Returns `true` if no phases are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.phases.is_empty()
    }

    /// Computes the topological order of registered phases using Kahn's
    /// algorithm.
    ///
    /// Among phases that are simultaneously ready (all deps satisfied), the
    /// algorithm picks them in alphabetical order for deterministic execution.
    ///
    /// # Errors
    ///
    /// - [`PhaseError::MissingDependency`] — a phase declares a dep that is
    ///   not registered.
    /// - [`PhaseError::Cycle`] — the dependency graph contains a cycle.
    fn topo_sort(&self) -> Result<Vec<&'static str>, PhaseError> {
        // Validate all deps are registered.
        for (&name, phase) in &self.phases {
            for &dep in phase.deps() {
                if !self.phases.contains_key(dep) {
                    return Err(PhaseError::MissingDependency {
                        phase: name,
                        dep,
                    });
                }
            }
        }

        // in_degree[name] = number of unsatisfied deps for that phase.
        let mut in_degree: HashMap<&'static str, usize> = HashMap::new();
        for &name in self.phases.keys() {
            in_degree.insert(name, self.phases[&name].deps().len());
        }

        // Ready set: phases with no unsatisfied deps. BTreeSet gives
        // deterministic alphabetical ordering.
        let mut ready: BTreeSet<&'static str> = self
            .phases
            .keys()
            .filter(|&&n| in_degree[&n] == 0)
            .copied()
            .collect();

        let mut order: Vec<&'static str> = Vec::with_capacity(self.phases.len());

        while let Some(&name) = ready.iter().next() {
            ready.remove(&name);
            order.push(name);

            // For each phase that depends on `name`, decrement its in-degree.
            // If it reaches zero, add it to the ready set.
            for (&other, phase) in &self.phases {
                if phase.deps().contains(&name) {
                    let entry = in_degree.get_mut(&other).expect("in_degree populated for all");
                    *entry -= 1;
                    if *entry == 0 {
                        ready.insert(other);
                    }
                }
            }
        }

        if order.len() != self.phases.len() {
            // Any phase not in `order` is part of (or downstream of) a cycle.
            let cyclic: Vec<&'static str> = self
                .phases
                .keys()
                .filter(|n| !order.contains(n))
                .copied()
                .collect();
            return Err(PhaseError::Cycle(cyclic.join(", ")));
        }

        Ok(order)
    }

    /// Executes all registered phases in topological order.
    ///
    /// Before calling this, the caller must insert each phase's `Input` into
    /// `ctx` under the phase's `NAME` (see [Input wiring](#input-wiring)).
    ///
    /// # Errors
    ///
    /// - [`PhaseError::MissingDependency`] / [`PhaseError::Cycle`] — from
    ///   [`topo_sort`](Self::topo_sort).
    /// - [`PhaseError::MissingInput`] — a phase's `Input` was not inserted.
    /// - [`PhaseError::ExecutionFailed`] — a phase's `run` returned an error.
    pub fn run(&self, ctx: &mut PipelineCtx) -> Result<(), PhaseError> {
        let order = self.topo_sort()?;
        for &name in &order {
            let phase = self
                .phases
                .get(name)
                .expect("topo_sort only returns registered phase names");
            phase
                .execute(ctx)
                .map_err(|e| match e {
                    // Pass through ExecutionFailed as-is (already carries the
                    // phase name and boxed inner error).
                    PhaseError::ExecutionFailed { .. } => e,
                    PhaseError::MissingInput(_) => PhaseError::MissingInput(name),
                    other => PhaseError::ExecutionFailed {
                        phase: name,
                        inner: Box::new(other),
                    },
                })?;
        }
        Ok(())
    }
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Pipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pipeline")
            .field("phases", &self.phases.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Test fixtures ---

    // We can't override const NAME per-instance in Rust, so we define separate
    // phase types for each test name. To keep tests readable, we use a macro.
    macro_rules! named_phase {
        ($type_name:ident, $name:expr, $deps:expr) => {
            #[derive(Default)]
            struct $type_name {
                log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
            }
            impl Phase for $type_name {
                type Input = String;
                type Output = String;
                const NAME: &'static str = $name;
                fn deps() -> &'static [&'static str] {
                    $deps
                }
                fn run(
                    &self,
                    input: Self::Input,
                    _ctx: &PipelineCtx,
                ) -> Result<Self::Output, PhaseError> {
                    self.log
                        .lock()
                        .expect("log")
                        .push(format!("{}({input})", $name));
                    Ok(format!("{}({input})", $name))
                }
            }
        };
    }

    // --- PipelineCtx tests ---

    #[test]
    fn ctx_insert_get_remove_round_trip() {
        let mut ctx = PipelineCtx::new();
        assert!(ctx.is_empty());
        ctx.insert("a", 42i32);
        assert!(ctx.contains("a"));
        assert_eq!(ctx.len(), 1);
        assert_eq!(ctx.get::<i32>("a"), Some(&42));
        let removed = ctx.remove::<i32>("a");
        assert_eq!(removed, Some(42));
        assert!(!ctx.contains("a"));
    }

    #[test]
    fn ctx_get_returns_none_for_missing_key() {
        let ctx = PipelineCtx::new();
        assert_eq!(ctx.get::<i32>("missing"), None);
    }

    #[test]
    fn ctx_get_returns_none_for_type_mismatch() {
        let mut ctx = PipelineCtx::new();
        ctx.insert("a", 42i32);
        // Wrong type requested → None (not a panic).
        assert_eq!(ctx.get::<String>("a"), None);
        assert_eq!(ctx.remove::<String>("a"), None);
        // Original value still present (remove with wrong type doesn't drop it).
        assert_eq!(ctx.get::<i32>("a"), Some(&42));
    }

    #[test]
    fn ctx_insert_replaces_existing() {
        let mut ctx = PipelineCtx::new();
        ctx.insert("a", 1i32);
        ctx.insert("a", 2i32);
        assert_eq!(ctx.get::<i32>("a"), Some(&2));
        assert_eq!(ctx.len(), 1);
    }

    #[test]
    fn ctx_debug_shows_keys_not_values() {
        let mut ctx = PipelineCtx::new();
        ctx.insert("alpha", 1i32);
        ctx.insert("beta", String::from("secret"));
        let s = format!("{ctx:?}");
        assert!(s.contains("alpha"), "got: {s}");
        assert!(s.contains("beta"), "got: {s}");
        // Values are type-erased; must not leak contents.
        assert!(!s.contains("secret"), "got: {s}");
    }

    // --- Pipeline registration tests ---

    named_phase!(PhaseA, "A", &[]);

    #[test]
    fn register_single_phase() {
        let mut p = Pipeline::new();
        assert!(p.is_empty());
        p.register(PhaseA::default()).unwrap();
        assert!(p.contains("A"));
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn register_duplicate_returns_error() {
        let mut p = Pipeline::new();
        p.register(PhaseA::default()).unwrap();
        let err = p.register(PhaseA::default()).unwrap_err();
        assert!(matches!(err, PhaseError::DuplicatePhase("A")));
        // Original still registered.
        assert_eq!(p.len(), 1);
    }

    // --- Topo sort tests ---

    #[test]
    fn topo_sort_empty_pipeline() {
        let p = Pipeline::new();
        let order = p.topo_sort().unwrap();
        assert!(order.is_empty());
    }

    #[test]
    fn topo_sort_single_phase() {
        let mut p = Pipeline::new();
        p.register(PhaseA::default()).unwrap();
        let order = p.topo_sort().unwrap();
        assert_eq!(order, vec!["A"]);
    }

    named_phase!(PhaseB, "B", &["A"]);
    named_phase!(PhaseC, "C", &["B"]);

    #[test]
    fn topo_sort_linear_chain() {
        let mut p = Pipeline::new();
        p.register(PhaseA::default()).unwrap();
        p.register(PhaseB::default()).unwrap();
        p.register(PhaseC::default()).unwrap();
        let order = p.topo_sort().unwrap();
        assert_eq!(order, vec!["A", "B", "C"]);
    }

    named_phase!(PhaseD, "D", &["B", "C"]);

    #[test]
    fn topo_sort_diamond_dag() {
        // A → B → D
        //   ↘ C ↗
        let mut p = Pipeline::new();
        p.register(PhaseA::default()).unwrap();
        p.register(PhaseB::default()).unwrap();
        p.register(PhaseC::default()).unwrap();
        p.register(PhaseD::default()).unwrap();
        let order = p.topo_sort().unwrap();
        // A must come first; D must come last. B and C are between, alphabetical.
        assert_eq!(order.first().copied(), Some("A"));
        assert_eq!(order.last().copied(), Some("D"));
        let mid = &order[1..3];
        assert_eq!(mid, &["B", "C"], "middle phases must be alphabetical: {order:?}");
    }

    named_phase!(PhaseIndependent, "Z", &[]);

    #[test]
    fn topo_sort_independent_phases_alphabetical() {
        let mut p = Pipeline::new();
        p.register(PhaseA::default()).unwrap();
        p.register(PhaseIndependent::default()).unwrap();
        let order = p.topo_sort().unwrap();
        // Both ready at start → alphabetical: A, Z.
        assert_eq!(order, vec!["A", "Z"]);
    }

    // --- Cycle detection ---

    named_phase!(CycleB, "CB", &["CA"]);
    named_phase!(CycleA, "CA", &["CB"]);

    #[test]
    fn topo_sort_detects_two_node_cycle() {
        let mut p = Pipeline::new();
        p.register(CycleA::default()).unwrap();
        p.register(CycleB::default()).unwrap();
        let err = p.topo_sort().unwrap_err();
        match err {
            PhaseError::Cycle(msg) => {
                // Both phases should be listed.
                assert!(msg.contains("CA"), "got: {msg}");
                assert!(msg.contains("CB"), "got: {msg}");
            }
            other => panic!("expected Cycle, got {other:?}"),
        }
    }

    named_phase!(CycleC, "CC", &["CB"]);

    #[test]
    fn topo_sort_blocks_downstream_of_cycle() {
        // CA ↔ CB form a 2-cycle; CC depends on CB (downstream of the cycle).
        // Kahn cannot make progress on any of them → all reported as blocked.
        let mut p = Pipeline::new();
        p.register(CycleA::default()).unwrap(); // deps: CB
        p.register(CycleB::default()).unwrap(); // deps: CA
        p.register(CycleC::default()).unwrap(); // deps: CB
        let err = p.topo_sort().unwrap_err();
        match err {
            PhaseError::Cycle(msg) => {
                // CA and CB are the cyclic pair; CC is blocked downstream.
                assert!(msg.contains("CA"), "got: {msg}");
                assert!(msg.contains("CB"), "got: {msg}");
                assert!(msg.contains("CC"), "got: {msg}");
            }
            other => panic!("expected Cycle, got {other:?}"),
        }
    }

    // --- Missing dependency ---

    named_phase!(PhaseWithMissingDep, "M", &["nonexistent"]);

    #[test]
    fn topo_sort_missing_dependency_returns_error() {
        let mut p = Pipeline::new();
        p.register(PhaseWithMissingDep::default()).unwrap();
        let err = p.topo_sort().unwrap_err();
        match err {
            PhaseError::MissingDependency { phase, dep } => {
                assert_eq!(phase, "M");
                assert_eq!(dep, "nonexistent");
            }
            other => panic!("expected MissingDependency, got {other:?}"),
        }
    }

    // --- Execution tests ---

    #[test]
    fn run_empty_pipeline_succeeds() {
        let p = Pipeline::new();
        let mut ctx = PipelineCtx::new();
        p.run(&mut ctx).unwrap();
    }

    #[test]
    fn run_single_phase_passes_input_and_stores_output() {
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let phase = PhaseA { log: log.clone() };
        let mut p = Pipeline::new();
        p.register(phase).unwrap();

        let mut ctx = PipelineCtx::new();
        ctx.insert("A", String::from("hello"));
        p.run(&mut ctx).unwrap();

        // Output stored under "A" (input "hello" → output "A(hello)").
        assert_eq!(ctx.get::<String>("A"), Some(&"A(hello)".to_string()));
        // Execution was logged exactly once.
        assert_eq!(log.lock().unwrap().len(), 1);
    }

    #[test]
    fn run_linear_chain_passes_outputs_in_order() {
        // A → B → C: each phase reads the previous phase's output from ctx.
        // We use Input = () for B and C (derived phases) and read dep outputs
        // from ctx inside run. But our named_phase! macro uses Input = String.
        // So for this test we define custom phases that read from ctx.
        struct A;
        impl Phase for A {
            type Input = ();
            type Output = u32;
            const NAME: &'static str = "A";
            fn deps() -> &'static [&'static str] {
                &[]
            }
            fn run(&self, _: Self::Input, _ctx: &PipelineCtx) -> Result<Self::Output, PhaseError> {
                Ok(1)
            }
        }
        struct B;
        impl Phase for B {
            type Input = ();
            type Output = u32;
            const NAME: &'static str = "B";
            fn deps() -> &'static [&'static str] {
                &["A"]
            }
            fn run(&self, _: Self::Input, ctx: &PipelineCtx) -> Result<Self::Output, PhaseError> {
                let a = ctx.get::<u32>("A").expect("A must run before B");
                Ok(a + 1)
            }
        }
        struct C;
        impl Phase for C {
            type Input = ();
            type Output = u32;
            const NAME: &'static str = "C";
            fn deps() -> &'static [&'static str] {
                &["B"]
            }
            fn run(&self, _: Self::Input, ctx: &PipelineCtx) -> Result<Self::Output, PhaseError> {
                let b = ctx.get::<u32>("B").expect("B must run before C");
                Ok(b + 1)
            }
        }

        let mut p = Pipeline::new();
        p.register(A).unwrap();
        p.register(B).unwrap();
        p.register(C).unwrap();

        let mut ctx = PipelineCtx::new();
        // Insert () inputs for all phases (derived phases use Input = ()).
        ctx.insert("A", ());
        ctx.insert("B", ());
        ctx.insert("C", ());
        p.run(&mut ctx).unwrap();

        assert_eq!(ctx.get::<u32>("A"), Some(&1));
        assert_eq!(ctx.get::<u32>("B"), Some(&2));
        assert_eq!(ctx.get::<u32>("C"), Some(&3));
    }

    #[test]
    fn run_diamond_dag_reads_shared_dep_output() {
        // A → B → D
        //   ↘ C ↗
        // B and C both read A's output; D reads B and C.
        struct A;
        impl Phase for A {
            type Input = ();
            type Output = u32;
            const NAME: &'static str = "A";
            fn deps() -> &'static [&'static str] {
                &[]
            }
            fn run(&self, _: Self::Input, _ctx: &PipelineCtx) -> Result<Self::Output, PhaseError> {
                Ok(10)
            }
        }
        struct B;
        impl Phase for B {
            type Input = ();
            type Output = u32;
            const NAME: &'static str = "B";
            fn deps() -> &'static [&'static str] {
                &["A"]
            }
            fn run(&self, _: Self::Input, ctx: &PipelineCtx) -> Result<Self::Output, PhaseError> {
                Ok(ctx.get::<u32>("A").copied().unwrap() + 1)
            }
        }
        struct C;
        impl Phase for C {
            type Input = ();
            type Output = u32;
            const NAME: &'static str = "C";
            fn deps() -> &'static [&'static str] {
                &["A"]
            }
            fn run(&self, _: Self::Input, ctx: &PipelineCtx) -> Result<Self::Output, PhaseError> {
                Ok(ctx.get::<u32>("A").copied().unwrap() + 2)
            }
        }
        struct D;
        impl Phase for D {
            type Input = ();
            type Output = u32;
            const NAME: &'static str = "D";
            fn deps() -> &'static [&'static str] {
                &["B", "C"]
            }
            fn run(&self, _: Self::Input, ctx: &PipelineCtx) -> Result<Self::Output, PhaseError> {
                let b = ctx.get::<u32>("B").copied().unwrap();
                let c = ctx.get::<u32>("C").copied().unwrap();
                Ok(b + c)
            }
        }

        let mut p = Pipeline::new();
        p.register(A).unwrap();
        p.register(B).unwrap();
        p.register(C).unwrap();
        p.register(D).unwrap();

        let mut ctx = PipelineCtx::new();
        for n in ["A", "B", "C", "D"] {
            ctx.insert(n, ());
        }
        p.run(&mut ctx).unwrap();

        // A=10, B=11, C=12, D=11+12=23.
        assert_eq!(ctx.get::<u32>("D"), Some(&23));
        // A's output must still be present (not consumed) since get() is shared.
        assert_eq!(ctx.get::<u32>("A"), Some(&10));
    }

    #[test]
    fn run_missing_input_returns_error() {
        let mut p = Pipeline::new();
        p.register(PhaseA::default()).unwrap();
        let mut ctx = PipelineCtx::new();
        // No input inserted for "A".
        let err = p.run(&mut ctx).unwrap_err();
        assert!(matches!(err, PhaseError::MissingInput("A")));
    }

    #[test]
    fn run_cycle_returns_error_before_execution() {
        let mut p = Pipeline::new();
        p.register(CycleA::default()).unwrap();
        p.register(CycleB::default()).unwrap();
        let mut ctx = PipelineCtx::new();
        let err = p.run(&mut ctx).unwrap_err();
        assert!(matches!(err, PhaseError::Cycle(_)));
        // Nothing executed → context untouched.
        assert!(ctx.is_empty());
    }

    #[test]
    fn run_missing_dependency_returns_error_before_execution() {
        let mut p = Pipeline::new();
        p.register(PhaseWithMissingDep::default()).unwrap();
        let mut ctx = PipelineCtx::new();
        let err = p.run(&mut ctx).unwrap_err();
        assert!(matches!(
            err,
            PhaseError::MissingDependency { phase: "M", dep: "nonexistent" }
        ));
    }

    #[test]
    fn run_phase_error_is_wrapped_with_phase_name() {
        struct FailPhase;
        impl Phase for FailPhase {
            type Input = ();
            type Output = ();
            const NAME: &'static str = "FAIL";
            fn deps() -> &'static [&'static str] {
                &[]
            }
            fn run(&self, _: Self::Input, _ctx: &PipelineCtx) -> Result<Self::Output, PhaseError> {
                Err(PhaseError::ExecutionFailed {
                    phase: "FAIL",
                    inner: Box::new(std::io::Error::other("boom")),
                })
            }
        }
        let mut p = Pipeline::new();
        p.register(FailPhase).unwrap();
        let mut ctx = PipelineCtx::new();
        ctx.insert("FAIL", ());
        let err = p.run(&mut ctx).unwrap_err();
        match err {
            PhaseError::ExecutionFailed { phase, inner } => {
                assert_eq!(phase, "FAIL");
                assert!(inner.to_string().contains("boom"), "got: {inner}");
            }
            other => panic!("expected ExecutionFailed, got {other:?}"),
        }
    }

    // --- Non-ExecutionFailed phase errors are rewrapped (other arm) ---
    //
    // A phase that returns a PhaseError variant OTHER than ExecutionFailed or
    // MissingInput must be wrapped into ExecutionFailed by the runner (lines 411-414).

    #[test]
    fn run_phase_returning_type_mismatch_is_wrapped_as_execution_failed() {
        struct OddFailPhase;
        impl Phase for OddFailPhase {
            type Input = ();
            type Output = ();
            const NAME: &'static str = "ODD";
            fn deps() -> &'static [&'static str] {
                &[]
            }
            fn run(&self, _: Self::Input, _ctx: &PipelineCtx) -> Result<Self::Output, PhaseError> {
                // A non-ExecutionFailed, non-MissingInput variant → hits the
                // `other` arm in Pipeline::run, which wraps it.
                Err(PhaseError::TypeMismatch("ODD"))
            }
        }
        let mut p = Pipeline::new();
        p.register(OddFailPhase).unwrap();
        let mut ctx = PipelineCtx::new();
        ctx.insert("ODD", ());
        let err = p.run(&mut ctx).unwrap_err();
        match err {
            PhaseError::ExecutionFailed { phase, inner } => {
                assert_eq!(phase, "ODD");
                // Inner carries the original TypeMismatch error message.
                assert!(inner.to_string().contains("ODD"), "got: {inner}");
            }
            other => panic!("expected ExecutionFailed wrapping TypeMismatch, got {other:?}"),
        }
    }

    // --- Default impls + Debug formatting (lines 181-182, 423-424, 429-431) ---

    #[test]
    fn pipeline_ctx_default_is_empty() {
        let ctx = PipelineCtx::default();
        assert!(ctx.is_empty());
        assert_eq!(ctx.len(), 0);
    }

    #[test]
    fn pipeline_default_is_empty() {
        let p = Pipeline::default();
        assert!(p.is_empty());
        assert_eq!(p.len(), 0);
    }

    #[test]
    fn pipeline_debug_format_lists_registered_phase_names() {
        let mut p = Pipeline::new();
        p.register(PhaseA::default()).unwrap();
        let s = format!("{p:?}");
        assert!(s.contains("Pipeline"), "got: {s}");
        assert!(s.contains("A"), "debug output must list phase names: {s}");
    }

    #[test]
    fn pipeline_debug_format_empty_pipeline() {
        let p = Pipeline::new();
        let s = format!("{p:?}");
        assert!(s.contains("Pipeline"), "got: {s}");
    }

    #[test]
    fn run_executes_phases_in_topological_order() {
        // Use a shared log to verify execution order matches topo sort.
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        struct A(std::sync::Arc<std::sync::Mutex<Vec<String>>>);
        impl Phase for A {
            type Input = ();
            type Output = ();
            const NAME: &'static str = "A";
            fn deps() -> &'static [&'static str] {
                &[]
            }
            fn run(&self, _: Self::Input, _ctx: &PipelineCtx) -> Result<Self::Output, PhaseError> {
                self.0.lock().unwrap().push("A".to_string());
                Ok(())
            }
        }
        struct B(std::sync::Arc<std::sync::Mutex<Vec<String>>>);
        impl Phase for B {
            type Input = ();
            type Output = ();
            const NAME: &'static str = "B";
            fn deps() -> &'static [&'static str] {
                &["A"]
            }
            fn run(&self, _: Self::Input, _ctx: &PipelineCtx) -> Result<Self::Output, PhaseError> {
                self.0.lock().unwrap().push("B".to_string());
                Ok(())
            }
        }
        struct C(std::sync::Arc<std::sync::Mutex<Vec<String>>>);
        impl Phase for C {
            type Input = ();
            type Output = ();
            const NAME: &'static str = "C";
            fn deps() -> &'static [&'static str] {
                &["A"]
            }
            fn run(&self, _: Self::Input, _ctx: &PipelineCtx) -> Result<Self::Output, PhaseError> {
                self.0.lock().unwrap().push("C".to_string());
                Ok(())
            }
        }
        struct D(std::sync::Arc<std::sync::Mutex<Vec<String>>>);
        impl Phase for D {
            type Input = ();
            type Output = ();
            const NAME: &'static str = "D";
            fn deps() -> &'static [&'static str] {
                &["B", "C"]
            }
            fn run(&self, _: Self::Input, _ctx: &PipelineCtx) -> Result<Self::Output, PhaseError> {
                self.0.lock().unwrap().push("D".to_string());
                Ok(())
            }
        }

        let mut p = Pipeline::new();
        p.register(A(log.clone())).unwrap();
        p.register(B(log.clone())).unwrap();
        p.register(C(log.clone())).unwrap();
        p.register(D(log.clone())).unwrap();

        let mut ctx = PipelineCtx::new();
        for n in ["A", "B", "C", "D"] {
            ctx.insert(n, ());
        }
        p.run(&mut ctx).unwrap();

        let recorded = log.lock().unwrap().clone();
        assert_eq!(
            recorded,
            vec!["A".to_string(), "B".to_string(), "C".to_string(), "D".to_string()],
            "execution order must match topological order"
        );
    }

    // --- Send + Sync assertions ---

    #[test]
    fn pipeline_ctx_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PipelineCtx>();
    }

    #[test]
    fn pipeline_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Pipeline>();
    }
}
