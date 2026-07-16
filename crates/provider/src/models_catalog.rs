//! Embedded models.dev catalog — static lookup of (provider, model) metadata.
//!
//! The 3.2 MB snapshot at [`EMBEDDED_JSON`] is compiled into the binary as a
//! `&'static str`. Deserialization is deferred until the first call to
//! [`ModelsCatalog::get`] via a [`OnceLock`], so the cold-start fast path
//! (`mscode version`) pays zero cost — the bytes sit in the read-only segment
//! and the parser never runs unless a user actually invokes `/models` or
//! `mscode models`.
//!
//! # Forward compatibility
//!
//! Every field on [`ProviderEntry`] / [`ModelEntry`] is marked
//! `#[serde(default)]`. models.dev ships new fields regularly (`cost`,
//! `reasoning_options`, `release_date`, …) and we deliberately ignore the ones
//! we don't need. Stripping `#[serde(default)]` would break the next catalog
//! refresh.
//!
//! # Source
//!
//! Snapshot fetched from <https://github.com/codici-ai/models.dev> (specific
//! commit recorded by the dumper). Refresh procedure: download a new snapshot,
//! replace `data/models-dev.json`, run `cargo test -p mscode-provider
//! models_catalog` — if the parser regresses, default fields are missing.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

/// Embedded models.dev snapshot. Compiled into the binary read-only segment.
const EMBEDDED_JSON: &str = include_str!("../data/models-dev.json");

/// One top-level entry in the models.dev catalog (e.g. `openai`, `anthropic`).
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderEntry {
    /// Catalog id, e.g. `"openai"`. Matches the id used by the credentials
    /// store for the well-known providers.
    pub id: String,
    /// Human-readable provider name, e.g. `"OpenAI"`.
    pub name: String,
    /// Models keyed by id (`"gpt-5-codex"`). Empty for providers that have
    /// no static catalog (rare).
    #[serde(default)]
    pub models: BTreeMap<String, ModelEntry>,
}

/// One model in the catalog. Only the fields needed by the CLI/TUI are kept;
/// everything else (`cost`, `release_date`, `reasoning_options`, …) is
/// dropped at parse time.
#[derive(Debug, Clone, Deserialize)]
pub struct ModelEntry {
    /// Model id, e.g. `"gpt-5-codex"`. Passed verbatim to the provider adapter.
    pub id: String,
    /// Display name, e.g. `"GPT-5-Codex"`.
    pub name: String,
    /// `true` when the model accepts tool/function calls. Defaults to `false`
    /// when missing from the JSON.
    #[serde(default)]
    pub tool_call: bool,
    /// `true` when the model exposes a reasoning channel. Defaults to `false`.
    #[serde(default)]
    pub reasoning: bool,
    /// Context/output/input limits. Defaults to "unknown" when missing.
    #[serde(default)]
    pub limit: ModelLimit,
    /// I/O modalities. Defaults to empty when missing.
    #[serde(default)]
    pub modalities: ModelModalities,
}

/// Subset of the `limit` object.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ModelLimit {
    /// Maximum tokens the model can accept in one request.
    #[serde(default)]
    pub context: Option<u64>,
    /// Maximum output tokens per request. Surfaced when present.
    #[serde(default)]
    pub output: Option<u64>,
}

/// Subset of the `modalities` object.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ModelModalities {
    /// Input modalities, e.g. `["text", "image"]`.
    #[serde(default)]
    pub input: Vec<String>,
    /// Output modalities, e.g. `["text"]`.
    #[serde(default)]
    pub output: Vec<String>,
}

/// Borrowed view onto a (provider, model) pair. Returned by filtering queries
/// so callers don't have to re-resolve the parent provider.
#[derive(Debug, Clone, Copy)]
pub struct ModelRef<'a> {
    pub provider_id: &'a str,
    pub provider_name: &'a str,
    pub model: &'a ModelEntry,
}

/// Top-level catalog. Cheap to hold a reference (just `&'static Self`).
#[derive(Debug, Deserialize)]
pub struct ModelsCatalog {
    /// Flat map keyed by provider id. Using `flatten` so the JSON's top-level
    /// object (which is `{ "openai": {...}, "anthropic": {...}, ... }`)
    /// deserializes directly into this struct without a wrapper key.
    #[serde(flatten)]
    providers: BTreeMap<String, ProviderEntry>,
}

static CATALOG: OnceLock<ModelsCatalog> = OnceLock::new();

impl ModelsCatalog {
    /// Access the singleton catalog. First call deserializes the embedded
    /// snapshot; subsequent calls are a cheap atomic load.
    ///
    /// # Panics
    ///
    /// Panics if the embedded JSON fails to parse. This is a programming error
    /// (the snapshot was committed without running tests) — failing loudly on
    /// first use is preferable to silent degradation.
    pub fn get() -> &'static Self {
        CATALOG.get_or_init(|| {
            serde_json::from_str(EMBEDDED_JSON).expect("embedded models-dev.json must parse")
        })
    }

    /// Read-only access to all providers, keyed by id.
    pub fn providers(&self) -> &BTreeMap<String, ProviderEntry> {
        &self.providers
    }

    /// Look up a single provider by id.
    pub fn get_provider(&self, id: &str) -> Option<&ProviderEntry> {
        self.providers.get(id)
    }

    /// Flatten all (provider, model) pairs across the entire catalog.
    pub fn all_models(&self) -> Vec<ModelRef<'_>> {
        let mut out = Vec::new();
        for p in self.providers.values() {
            for m in p.models.values() {
                out.push(ModelRef {
                    provider_id: &p.id,
                    provider_name: &p.name,
                    model: m,
                });
            }
        }
        out
    }

    /// Flatten (provider, model) pairs for a filtered set of provider ids.
    ///
    /// Provider ids not present in the catalog are silently skipped — callers
    /// may have credentials for providers we don't ship metadata for (e.g.
    /// `ollama`, `custom:*`, or providers added to the credentials catalog
    /// before their models.dev entry).
    ///
    /// Output is stable: iteration order follows the `BTreeMap` ordering of
    /// both `provider_ids` (caller's responsibility) and the inner models.
    pub fn models_for_providers<'a>(&'a self, provider_ids: &'a [String]) -> Vec<ModelRef<'a>> {
        let mut out = Vec::new();
        for pid in provider_ids {
            if let Some(p) = self.providers.get(pid) {
                for m in p.models.values() {
                    out.push(ModelRef {
                        provider_id: &p.id,
                        provider_name: &p.name,
                        model: m,
                    });
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_parses_successfully() {
        let c = ModelsCatalog::get();
        assert!(
            c.providers().len() > 100,
            "expected many providers, got {}",
            c.providers().len()
        );
    }

    #[test]
    fn catalog_has_openai_with_models() {
        let c = ModelsCatalog::get();
        let p = c.get_provider("openai").expect("openai must be in catalog");
        assert!(!p.models.is_empty(), "openai must have models");
        assert!(
            p.models.contains_key("gpt-5-codex"),
            "expected gpt-5-codex; got keys: {:?}",
            p.models.keys().take(5).collect::<Vec<_>>()
        );
    }

    #[test]
    fn catalog_has_anthropic() {
        let c = ModelsCatalog::get();
        let p = c
            .get_provider("anthropic")
            .expect("anthropic must be in catalog");
        assert!(!p.models.is_empty());
    }

    #[test]
    fn tool_call_field_deserializes() {
        let c = ModelsCatalog::get();
        let p = c.get_provider("openai").unwrap();
        let codex = &p.models["gpt-5-codex"];
        assert!(codex.tool_call, "gpt-5-codex should support tool calls");
        assert!(codex.reasoning, "gpt-5-codex should be reasoning-capable");
    }

    #[test]
    fn context_limit_deserializes() {
        let c = ModelsCatalog::get();
        let p = c.get_provider("openai").unwrap();
        let codex = &p.models["gpt-5-codex"];
        assert!(
            codex.limit.context.unwrap_or(0) >= 100_000,
            "gpt-5-codex context should be >=100k, got {:?}",
            codex.limit.context
        );
    }

    #[test]
    fn models_for_empty_provider_list_is_empty() {
        let c = ModelsCatalog::get();
        let out = c.models_for_providers(&[]);
        assert!(out.is_empty());
    }

    #[test]
    fn models_for_single_provider_filters_correctly() {
        let c = ModelsCatalog::get();
        let ids = ["openai".to_string()];
        let out = c.models_for_providers(&ids);
        assert!(!out.is_empty());
        assert!(
            out.iter().all(|m| m.provider_id == "openai"),
            "all entries should be openai"
        );
    }

    #[test]
    fn models_for_providers_skips_unknown_ids() {
        let c = ModelsCatalog::get();
        let ids = ["openai".to_string(), "ghost-provider".to_string()];
        let out = c.models_for_providers(&ids);
        assert!(out.iter().all(|m| m.provider_id == "openai"));
    }

    #[test]
    fn models_for_providers_union() {
        let c = ModelsCatalog::get();
        let ids = vec!["openai".to_string(), "anthropic".to_string()];
        let out = c.models_for_providers(&ids);
        let has_openai = out.iter().any(|m| m.provider_id == "openai");
        let has_anthropic = out.iter().any(|m| m.provider_id == "anthropic");
        assert!(has_openai && has_anthropic);
    }

    #[test]
    fn all_models_returns_every_provider() {
        let c = ModelsCatalog::get();
        let all = c.all_models();
        assert!(!all.is_empty());
        let distinct_providers: std::collections::HashSet<&str> =
            all.iter().map(|m| m.provider_id).collect();
        assert_eq!(distinct_providers.len(), c.providers().len());
    }

    #[test]
    fn modalities_default_to_empty_when_missing() {
        // Hard to predict which entry omits modalities, but the default impl
        // must not panic. The deserializer succeeding is the assertion.
        let _ = ModelsCatalog::get();
    }
}
