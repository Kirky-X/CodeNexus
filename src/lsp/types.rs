// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Type-bridging helpers between `lsp_types` and CodeNexus [`model`].
//!
//! The LSP protocol carries symbol taxonomy as the opaque
//! [`lsp_types::SymbolKind`] struct (a wrapper around an `i32` discriminant
//! defined by the LSP specification, e.g. `SymbolKind::FUNCTION == 12`).
//! CodeNexus stores symbol taxonomy as the [`NodeLabel`] enum, which is the
//! label set materialized in the LadybugDB schema. [`map_lsp_symbol_kind`]
//! translates between the two taxonomies so that LSP-derived semantic
//! information can be persisted on graph nodes (R-lsp-004) without leaking
//! LSP-specific identifiers into the storage layer.
//!
//! [`model`]: crate::model
//! [`NodeLabel`]: crate::model::NodeLabel

use crate::model::NodeLabel;

/// Translate an LSP [`lsp_types::SymbolKind`] into the closest CodeNexus
/// [`NodeLabel`].
///
/// Returns `None` for kinds that have no faithful CodeNexus counterpart
/// (e.g. `SymbolKind::STRING`, `SymbolKind::NUMBER`, `SymbolKind::KEY`,
/// `SymbolKind::NULL`). Persisting `None` is meaningful — it tells the
/// indexer to leave `semantic_type` unset rather than guessing.
///
/// # Mapping table (specmark spec.md R-lsp-003)
///
/// | LSP SymbolKind       | CodeNexus NodeLabel |
/// |----------------------|---------------------|
/// | `FILE`               | — (out of scope)    |
/// | `MODULE`             | `Module`            |
/// | `NAMESPACE`          | `Namespace`         |
/// | `PACKAGE`            | — (out of scope)    |
/// | `CLASS`              | `Class`             |
/// | `METHOD`             | `Method`            |
/// | `PROPERTY`           | `Property`          |
/// | `FIELD`              | `Field`             |
/// | `CONSTRUCTOR`        | `Constructor`       |
/// | `ENUM`               | `Enum`              |
/// | `INTERFACE`          | `Interface`         |
/// | `FUNCTION`           | `Function`          |
/// | `VARIABLE`           | `Variable`          |
/// | `CONSTANT`           | `Const`             |
/// | `ENUM_MEMBER`        | `Variant`           |
/// | `STRUCT`             | `Struct`            |
/// | `EVENT`              | `Event`             |
/// | `OPERATOR`           | — (out of scope)    |
/// | `TYPE_PARAMETER`     | `Typedef`           |
/// | `STRING`/`NUMBER`/...| — (literals, not symbols) |
///
/// # Example
///
/// ```
/// # #[cfg(feature = "lsp")] {
/// use codenexus::lsp::map_lsp_symbol_kind;
/// use codenexus::model::NodeLabel;
/// use lsp_types::SymbolKind;
///
/// assert_eq!(map_lsp_symbol_kind(SymbolKind::FUNCTION), Some(NodeLabel::Function));
/// assert_eq!(map_lsp_symbol_kind(SymbolKind::CLASS),    Some(NodeLabel::Class));
/// assert_eq!(map_lsp_symbol_kind(SymbolKind::STRING),   None);
/// # }
/// ```
#[must_use]
pub fn map_lsp_symbol_kind(kind: lsp_types::SymbolKind) -> Option<NodeLabel> {
    use lsp_types::SymbolKind as K;
    // `SymbolKind` is `#[derive(PartialEq, Eq)]` — direct `==` comparison is
    // the canonical pattern used by rust-analyzer itself. We avoid `match`
    // because the field is a private `i32` (the discriminant), not an enum.
    if kind == K::FUNCTION {
        Some(NodeLabel::Function)
    } else if kind == K::METHOD {
        Some(NodeLabel::Method)
    } else if kind == K::CLASS {
        Some(NodeLabel::Class)
    } else if kind == K::INTERFACE {
        Some(NodeLabel::Interface)
    } else if kind == K::ENUM {
        Some(NodeLabel::Enum)
    } else if kind == K::STRUCT {
        Some(NodeLabel::Struct)
    } else if kind == K::MODULE {
        Some(NodeLabel::Module)
    } else if kind == K::NAMESPACE {
        Some(NodeLabel::Namespace)
    } else if kind == K::VARIABLE {
        Some(NodeLabel::Variable)
    } else if kind == K::CONSTANT {
        Some(NodeLabel::Const)
    } else if kind == K::PROPERTY {
        Some(NodeLabel::Property)
    } else if kind == K::FIELD {
        Some(NodeLabel::Field)
    } else if kind == K::CONSTRUCTOR {
        Some(NodeLabel::Constructor)
    } else if kind == K::ENUM_MEMBER {
        Some(NodeLabel::Variant)
    } else if kind == K::EVENT {
        Some(NodeLabel::Event)
    } else if kind == K::TYPE_PARAMETER {
        Some(NodeLabel::Typedef)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    //! R-lsp-003 acceptance tests.
    //!
    //! These tests do NOT touch a real LSP server — they only exercise the
    //! pure mapping function. CI runs them on every push; the
    //! `#[ignore]`-tagged integration tests in `client.rs` cover the
    //! rust-analyzer round-trip.

    use super::map_lsp_symbol_kind;
    use crate::model::NodeLabel;
    use lsp_types::SymbolKind;

    // --- R-lsp-003 mandatory mappings (specmark spec.md) ---

    #[test]
    fn map_function_kind() {
        assert_eq!(
            map_lsp_symbol_kind(SymbolKind::FUNCTION),
            Some(NodeLabel::Function)
        );
    }

    #[test]
    fn map_method_kind() {
        assert_eq!(
            map_lsp_symbol_kind(SymbolKind::METHOD),
            Some(NodeLabel::Method)
        );
    }

    #[test]
    fn map_class_kind() {
        assert_eq!(
            map_lsp_symbol_kind(SymbolKind::CLASS),
            Some(NodeLabel::Class)
        );
    }

    #[test]
    fn map_interface_kind() {
        assert_eq!(
            map_lsp_symbol_kind(SymbolKind::INTERFACE),
            Some(NodeLabel::Interface)
        );
    }

    #[test]
    fn map_enum_kind() {
        assert_eq!(map_lsp_symbol_kind(SymbolKind::ENUM), Some(NodeLabel::Enum));
    }

    #[test]
    fn map_struct_kind() {
        assert_eq!(
            map_lsp_symbol_kind(SymbolKind::STRUCT),
            Some(NodeLabel::Struct)
        );
    }

    #[test]
    fn map_module_kind() {
        assert_eq!(
            map_lsp_symbol_kind(SymbolKind::MODULE),
            Some(NodeLabel::Module)
        );
    }

    #[test]
    fn map_variable_kind() {
        assert_eq!(
            map_lsp_symbol_kind(SymbolKind::VARIABLE),
            Some(NodeLabel::Variable)
        );
    }

    #[test]
    fn map_constant_kind() {
        assert_eq!(
            map_lsp_symbol_kind(SymbolKind::CONSTANT),
            Some(NodeLabel::Const)
        );
    }

    // --- Extended mappings (R-lsp-003 table, optional rows) ---

    #[test]
    fn map_namespace_kind() {
        assert_eq!(
            map_lsp_symbol_kind(SymbolKind::NAMESPACE),
            Some(NodeLabel::Namespace)
        );
    }

    #[test]
    fn map_property_kind() {
        assert_eq!(
            map_lsp_symbol_kind(SymbolKind::PROPERTY),
            Some(NodeLabel::Property)
        );
    }

    #[test]
    fn map_field_kind() {
        assert_eq!(
            map_lsp_symbol_kind(SymbolKind::FIELD),
            Some(NodeLabel::Field)
        );
    }

    #[test]
    fn map_constructor_kind() {
        assert_eq!(
            map_lsp_symbol_kind(SymbolKind::CONSTRUCTOR),
            Some(NodeLabel::Constructor)
        );
    }

    #[test]
    fn map_enum_member_kind() {
        assert_eq!(
            map_lsp_symbol_kind(SymbolKind::ENUM_MEMBER),
            Some(NodeLabel::Variant)
        );
    }

    #[test]
    fn map_event_kind() {
        assert_eq!(
            map_lsp_symbol_kind(SymbolKind::EVENT),
            Some(NodeLabel::Event)
        );
    }

    #[test]
    fn map_type_parameter_kind() {
        assert_eq!(
            map_lsp_symbol_kind(SymbolKind::TYPE_PARAMETER),
            Some(NodeLabel::Typedef)
        );
    }

    // --- R-lsp-003: unknown/unsupported kinds must return None ---

    #[test]
    fn map_unknown_kind_returns_none() {
        // Literal kinds — not symbols.
        assert_eq!(map_lsp_symbol_kind(SymbolKind::STRING), None);
        assert_eq!(map_lsp_symbol_kind(SymbolKind::NUMBER), None);
        assert_eq!(map_lsp_symbol_kind(SymbolKind::BOOLEAN), None);
        assert_eq!(map_lsp_symbol_kind(SymbolKind::ARRAY), None);
        assert_eq!(map_lsp_symbol_kind(SymbolKind::OBJECT), None);
        assert_eq!(map_lsp_symbol_kind(SymbolKind::KEY), None);
        assert_eq!(map_lsp_symbol_kind(SymbolKind::NULL), None);
        // Out-of-scope kinds.
        assert_eq!(map_lsp_symbol_kind(SymbolKind::FILE), None);
        assert_eq!(map_lsp_symbol_kind(SymbolKind::PACKAGE), None);
        assert_eq!(map_lsp_symbol_kind(SymbolKind::OPERATOR), None);
    }

    // --- Edge case: raw/uninitialized SymbolKind value ---
    //
    // `SymbolKind` is a transparent struct over `i32`. A server might send a
    // kind value our mapping doesn't recognize (e.g. a future LSP revision
    // adding `RECORD = 27`). The function must coerce to `None` rather than
    // panic. We can't construct an arbitrary `SymbolKind` directly (private
    // field), but serde can — so we deserialize an unknown integer.

    #[test]
    fn map_unrecognized_int_returns_none() {
        let unknown: SymbolKind = serde_json::from_str("999").expect("deserialize unknown kind");
        assert_eq!(map_lsp_symbol_kind(unknown), None);
    }

    // --- Symmetry: mapping is deterministic ---

    #[test]
    fn map_is_deterministic() {
        for kind in [
            SymbolKind::FUNCTION,
            SymbolKind::CLASS,
            SymbolKind::STRUCT,
            SymbolKind::ENUM,
            SymbolKind::INTERFACE,
        ] {
            let first = map_lsp_symbol_kind(kind);
            let second = map_lsp_symbol_kind(kind);
            assert_eq!(first, second, "mapping must be stable for {kind:?}");
        }
    }
}
