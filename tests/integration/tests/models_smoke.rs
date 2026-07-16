//! Smoke test: the embedded models.dev catalog parses and exposes the
//! well-known providers we rely on elsewhere (`openai`, `anthropic`).
//!
//! This guards the 3.2 MB JSON blob at `crates/provider/data/models-dev.json`
//! against silent corruption — if a refresh produces a structurally-incompatible
//! file, [`ModelsCatalog::get`] panics on first access and this test fails at
//! the boundary instead of mid-session inside the TUI.

use mscode_provider::ModelsCatalog;

#[test]
fn models_catalog_embeds_known_provider() {
    let c = ModelsCatalog::get();
    let openai = c.get_provider("openai");
    let anthropic = c.get_provider("anthropic");
    let openai = openai.expect("openai must be present in embedded catalog");
    let anthropic = anthropic.expect("anthropic must be present in embedded catalog");
    assert!(
        !openai.models.is_empty(),
        "openai must ship at least one model"
    );
    assert!(
        !anthropic.models.is_empty(),
        "anthropic must ship at least one model"
    );
}

#[test]
fn models_catalog_all_models_returns_flattened_view() {
    let c = ModelsCatalog::get();
    let all = c.all_models();
    assert!(!all.is_empty(), "all_models() must surface entries");
    // Each ref should resolve back to a known provider entry.
    for r in &all {
        assert!(c.get_provider(r.provider_id).is_some());
    }
}
