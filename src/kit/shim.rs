// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! In-tree fallback for [`trait_kit`] (Task 2.2).
//!
//! When the `trait-kit` cargo feature is disabled, this module provides a
//! complete re-implementation of the `trait_kit` API surface that
//! `crate::kit::mod` re-exports. The implementation uses `std::sync::RwLock`
//! instead of `arc-swap` (so no extra dependency is required) and is therefore
//! NOT lock-free — it is a correctness-only fallback for the rare case where
//! the upstream crate is unavailable (design.md §2.4).
//!
//! ## API parity
//!
//! Every path consumed by `crate::kit::mod` is mirrored here:
//!
//! - [`core::capability::CapabilityKey`]
//! - [`core::config::{ConfigKey, ConfigHandle}`]
//! - [`core::marker::{NoConfig, NoRequirements}`]
//! - [`core::module::Module`]
//! - [`core::builder::{ModuleBuilder, WithConfig, WithRequirements}`]
//! - [`kit::{Kit, KitError}`]
//! - [`kit::builder::{IntoKitModuleBuilder, KitModuleBuilder}`]
//!
//! Signatures match `trait-kit 0.1.0` exactly (verified against docs.rs).

// ===========================================================================
// core::capability
// ===========================================================================

pub mod core {
    pub mod capability {
        /// Key trait for identifying capabilities in Kit (shim mirror of
        /// `trait_kit::core::capability::CapabilityKey`).
        pub trait CapabilityKey: 'static {
            type Capability: ?Sized + Send + Sync + 'static;
            const NAME: &'static str;
        }
    }

    // -----------------------------------------------------------------------
    // core::config
    // -----------------------------------------------------------------------

    pub mod config {
        use std::sync::{Arc, RwLock};

        /// Key trait for identifying configuration in Kit (shim mirror of
        /// `trait_kit::core::config::ConfigKey`).
        pub trait ConfigKey: 'static {
            type Config: Send + Sync + 'static;
            const NAME: &'static str;
        }

        /// Shared handle to a configuration value (shim mirror of
        /// `trait_kit::core::config::ConfigHandle`).
        ///
        /// Wraps `Arc<RwLock<Arc<T>>>` instead of `Arc<ArcSwap<T>>` — same
        /// API, but uses a lock instead of lock-free swap. Multiple clones
        /// share the same cell; `set` on one is visible to all.
        pub struct ConfigHandle<T: Send + Sync + 'static> {
            inner: Arc<RwLock<Arc<T>>>,
        }

        impl<T: Send + Sync + 'static> ConfigHandle<T> {
            /// Create a new handle holding `value`.
            pub fn new(value: T) -> Self {
                Self {
                    inner: Arc::new(RwLock::new(Arc::new(value))),
                }
            }

            /// Load the current configuration snapshot.
            pub fn load(&self) -> Arc<T> {
                Arc::clone(&self.inner.read().expect("config lock poisoned"))
            }

            /// Replace the configuration value. Visible to all clones.
            pub fn set(&self, value: T) {
                *self.inner.write().expect("config lock poisoned") = Arc::new(value);
            }
        }

        impl<T: Send + Sync + 'static> Clone for ConfigHandle<T> {
            fn clone(&self) -> Self {
                Self {
                    inner: Arc::clone(&self.inner),
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // core::marker
    // -----------------------------------------------------------------------

    pub mod marker {
        /// Marker for modules without configuration.
        #[derive(
            Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd,
        )]
        pub struct NoConfig;

        /// Marker for modules without dependencies.
        #[derive(
            Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd,
        )]
        pub struct NoRequirements;
    }

    // -----------------------------------------------------------------------
    // core::module
    // -----------------------------------------------------------------------

    pub mod module {
        use std::error::Error;

        use super::builder::ModuleBuilder;

        /// Standard interface for all modules (shim mirror of
        /// `trait_kit::core::module::Module`).
        pub trait Module: Sized {
            type Config;
            type Requirements;
            type Capability;
            type Error: Error + Send + Sync + 'static;
            type Builder: ModuleBuilder<Self>;
            const NAME: &'static str;
        }
    }

    // -----------------------------------------------------------------------
    // core::builder
    // -----------------------------------------------------------------------

    pub mod builder {
        use super::module::Module;

        /// Standard builder trait (shim mirror of
        /// `trait_kit::core::builder::ModuleBuilder`).
        pub trait ModuleBuilder<M: Module> {
            fn build(self) -> Result<M::Capability, M::Error>;
        }

        /// Opt-in builder trait for configuration injection (shim mirror of
        /// `trait_kit::core::builder::WithConfig`).
        pub trait WithConfig<M: Module> {
            fn config(self, config: M::Config) -> Self;
        }

        /// Opt-in builder trait for dependency injection (shim mirror of
        /// `trait_kit::core::builder::WithRequirements`).
        pub trait WithRequirements<M: Module> {
            fn requirements(self, requirements: M::Requirements) -> Self;
        }
    }
}

// ===========================================================================
// kit
// ===========================================================================

pub mod kit {
    use std::any::{Any, TypeId};
    use std::collections::HashMap;
    use std::sync::{Arc, RwLock};

    use super::core::capability::CapabilityKey;
    use super::core::config::{ConfigHandle, ConfigKey};

    pub mod builder {
        use std::sync::Arc;

        use super::super::core::builder::ModuleBuilder;
        use super::super::core::capability::CapabilityKey;
        use super::super::core::module::Module;
        use super::{Kit, KitError};

        /// Wrapper that builds a module and registers its capability in Kit
        /// (shim mirror of `trait_kit::kit::builder::KitModuleBuilder`).
        pub struct KitModuleBuilder<M: Module, B> {
            builder: B,
            kit: Kit,
            _marker: std::marker::PhantomData<M>,
        }

        impl<M: Module, B: ModuleBuilder<M>> KitModuleBuilder<M, B> {
            /// Create a new KitModuleBuilder wrapping `builder` attached to
            /// `kit`.
            pub fn new(builder: B, kit: Kit) -> Self {
                Self {
                    builder,
                    kit,
                    _marker: std::marker::PhantomData,
                }
            }

            /// Build the module, register its capability under key `K`, and
            /// return the capability.
            ///
            /// # Type constraint
            ///
            /// `M::Capability` must equal `Arc<K::Capability>` (mirrors the
            /// real trait-kit constraint). Typically `M::Capability =
            /// Arc<dyn Trait>` and `K::Capability = dyn Trait`.
            pub fn provide<K>(self) -> Result<Arc<K::Capability>, KitError>
            where
                K: CapabilityKey,
                M: Module<Capability = Arc<K::Capability>>,
            {
                let capability: Arc<K::Capability> = self
                    .builder
                    .build()
                    .map_err(|source| KitError::BuildFailed {
                        module: M::NAME,
                        source: Box::new(source),
                    })?;
                self.kit.provide::<K>(Arc::clone(&capability))?;
                Ok(capability)
            }
        }

        /// Convert a standard builder into a KitModuleBuilder (shim mirror of
        /// `trait_kit::kit::builder::IntoKitModuleBuilder`).
        pub trait IntoKitModuleBuilder<M: Module> {
            type Builder: ModuleBuilder<M>;

            fn kit(self, kit: &Kit) -> KitModuleBuilder<M, Self::Builder>;
        }

        impl<M, B> IntoKitModuleBuilder<M> for B
        where
            M: Module<Builder = B>,
            B: ModuleBuilder<M>,
        {
            type Builder = B;

            fn kit(self, kit: &Kit) -> KitModuleBuilder<M, Self::Builder> {
                KitModuleBuilder::new(self, kit.clone())
            }
        }
    }

    pub mod error {
        use std::error::Error;
        use std::fmt;

        /// Error type for Kit operations (shim mirror of
        /// `trait_kit::kit::error::KitError`).
        #[derive(Debug)]
        pub enum KitError {
            BuildFailed {
                module: &'static str,
                source: Box<dyn Error + Send + Sync>,
            },
            MissingCapability {
                key: &'static str,
            },
            DuplicateCapability {
                key: &'static str,
            },
            MissingConfig {
                key: &'static str,
            },
        }

        impl fmt::Display for KitError {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self {
                    Self::BuildFailed { module, source } => {
                        write!(f, "build failed for module `{module}`: {source}")
                    }
                    Self::MissingCapability { key } => {
                        write!(f, "required capability `{key}` is missing from Kit")
                    }
                    Self::DuplicateCapability { key } => {
                        write!(f, "capability `{key}` already registered in Kit")
                    }
                    Self::MissingConfig { key } => {
                        write!(f, "required config `{key}` is missing from Kit")
                    }
                }
            }
        }

        impl Error for KitError {
            fn source(&self) -> Option<&(dyn Error + 'static)> {
                match self {
                    Self::BuildFailed { source, .. } => Some(source.as_ref()),
                    _ => None,
                }
            }
        }
    }

    pub use error::KitError;

    /// Capability & configuration management center (shim mirror of
    /// `trait_kit::kit::Kit`).
    ///
    /// Uses `RwLock<HashMap<TypeId, Box<dyn Any + Send + Sync>>>` for both
    /// capabilities and configs. `Clone` shares the same inner map (mirrors
    /// the real crate's "changes on one clone are visible on all" semantics).
    pub struct Kit {
        inner: Arc<RwLock<KitInner>>,
    }

    struct KitInner {
        capabilities: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
        configs: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    }

    impl Default for Kit {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Kit {
        /// Create a new empty Kit.
        pub fn new() -> Self {
            Self {
                inner: Arc::new(RwLock::new(KitInner {
                    capabilities: HashMap::new(),
                    configs: HashMap::new(),
                })),
            }
        }

        /// Register a capability under key `K`.
        ///
        /// Returns `Err(KitError::DuplicateCapability)` if the key already
        /// exists.
        pub fn provide<K>(&self, value: Arc<K::Capability>) -> Result<(), KitError>
        where
            K: CapabilityKey,
        {
            let mut inner = self.inner.write().expect("kit lock poisoned");
            let key = TypeId::of::<K>();
            if inner.capabilities.contains_key(&key) {
                return Err(KitError::DuplicateCapability { key: K::NAME });
            }
            inner.capabilities.insert(key, Box::new(value));
            Ok(())
        }

        /// Replace an existing capability (or insert if absent).
        pub fn replace<K>(&self, value: Arc<K::Capability>) -> Result<(), KitError>
        where
            K: CapabilityKey,
        {
            let mut inner = self.inner.write().expect("kit lock poisoned");
            inner.capabilities.insert(TypeId::of::<K>(), Box::new(value));
            Ok(())
        }

        /// Require a capability, returning `Err(MissingCapability)` if absent.
        pub fn require<K>(&self) -> Result<Arc<K::Capability>, KitError>
        where
            K: CapabilityKey,
        {
            let inner = self.inner.read().expect("kit lock poisoned");
            let boxed = inner
                .capabilities
                .get(&TypeId::of::<K>())
                .ok_or(KitError::MissingCapability { key: K::NAME })?;
            // The stored value is `Arc<K::Capability>` boxed as
            // `Box<dyn Any + Send + Sync>`. Downcast to the concrete Arc type.
            boxed
                .downcast_ref::<Arc<K::Capability>>()
                .map(Arc::clone)
                .ok_or(KitError::MissingCapability { key: K::NAME })
        }

        /// Check whether capability `K` is registered.
        pub fn contains<K>(&self) -> bool
        where
            K: CapabilityKey,
        {
            self.inner
                .read()
                .expect("kit lock poisoned")
                .capabilities
                .contains_key(&TypeId::of::<K>())
        }

        /// Register a config handle under key `K`.
        pub fn set_config<K>(
            &self,
            handle: ConfigHandle<K::Config>,
        ) -> Result<(), KitError>
        where
            K: ConfigKey,
        {
            let mut inner = self.inner.write().expect("kit lock poisoned");
            inner.configs.insert(TypeId::of::<K>(), Box::new(handle));
            Ok(())
        }

        /// Require a config handle, returning `Err(MissingConfig)` if absent.
        pub fn config<K>(&self) -> Result<ConfigHandle<K::Config>, KitError>
        where
            K: ConfigKey,
        {
            let inner = self.inner.read().expect("kit lock poisoned");
            let boxed = inner
                .configs
                .get(&TypeId::of::<K>())
                .ok_or(KitError::MissingConfig { key: K::NAME })?;
            boxed
                .downcast_ref::<ConfigHandle<K::Config>>()
                .cloned()
                .ok_or(KitError::MissingConfig { key: K::NAME })
        }

        /// Check whether config `K` is registered.
        pub fn contains_config<K>(&self) -> bool
        where
            K: ConfigKey,
        {
            self.inner
                .read()
                .expect("kit lock poisoned")
                .configs
                .contains_key(&TypeId::of::<K>())
        }
    }

    impl Clone for Kit {
        fn clone(&self) -> Self {
            Self {
                inner: Arc::clone(&self.inner),
            }
        }
    }
}
