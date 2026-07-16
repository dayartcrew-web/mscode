//! Static catalog of well-known AI providers.
//!
//! Mirrors a curated subset of OpenCode's `auth login` provider list
//! (https://github.com/sst/opencode). OpenCode fetches the full ~166-entry
//! catalog at runtime from `https://models.dev/api.json`; we cannot do that
//! under this crate's local-first / no-network-on-startup constraints.
//! Instead we ship a static catalog covering the providers most users will
//! actually configure. Niche providers can still be added via the
//! `--endpoint` flag with any provider id.
//!
//! # Auth methods
//!
//! - [`AuthMethod::ApiKey`] — Bearer token in `Authorization` header. The
//!   only flow the CLI supports at v1.
//! - [`AuthMethod::OAuth`] — provider requires an OAuth flow (not yet
//!   implemented). Listed for visibility; `add` will refuse with a clear
//!   error pointing to the tracker.
//! - [`AuthMethod::Both`] — accepts either; the API-key path works today.
//! - [`AuthMethod::Local`] — no auth (e.g. local Ollama server).

/// How a provider authenticates requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    /// Bearer token in `Authorization: Bearer <key>`.
    ApiKey,
    /// OAuth flow (device code or redirect). Not yet wired.
    OAuth,
    /// Both API key and OAuth work.
    Both,
    /// No auth required (local server).
    Local,
}

impl AuthMethod {
    /// Returns `true` when the v1 API-key flow can complete `login add`.
    pub fn supports_api_key(self) -> bool {
        matches!(
            self,
            AuthMethod::ApiKey | AuthMethod::Both | AuthMethod::Local
        )
    }

    /// Stable string for SQLite / TUI display.
    pub fn as_str(self) -> &'static str {
        match self {
            AuthMethod::ApiKey => "api_key",
            AuthMethod::OAuth => "oauth",
            AuthMethod::Both => "both",
            AuthMethod::Local => "local",
        }
    }
}

/// A static catalog entry describing a known provider.
#[derive(Debug, Clone, Copy)]
pub struct ProviderCatalogEntry {
    /// Lowercase identifier used in `--provider`. Matches OpenCode's id.
    pub id: &'static str,
    /// Human-readable name shown in TUI / `login list`.
    pub display_name: &'static str,
    /// Default chat-completions endpoint. `None` means the provider needs an
    /// explicit `--endpoint` (Azure, Bedrock, etc. require account-specific
    /// URLs that cannot be templated without user input).
    pub endpoint: Option<&'static str>,
    /// How the provider authenticates.
    pub auth: AuthMethod,
}

/// Recommended-flag shortcut. Kept as a function to avoid exposing the
/// sentinel stringly-typed field on the public struct.
fn is_recommended(id: &str) -> bool {
    matches!(id, "opencode" | "openai" | "anthropic" | "openrouter")
}

/// The curated static catalog. Ordered roughly by popularity so the TUI
/// prompt shows useful entries first.
pub const PROVIDER_CATALOG: &[ProviderCatalogEntry] = &[
    // --- Tier 1: OpenCode-branded + majors -----------------------------
    ProviderCatalogEntry {
        id: "opencode",
        display_name: "OpenCode Zen",
        endpoint: Some("https://opencode.ai/zen/v1/chat/completions"),
        auth: AuthMethod::Both,
    },
    ProviderCatalogEntry {
        id: "openai",
        display_name: "OpenAI",
        endpoint: Some("https://api.openai.com/v1/chat/completions"),
        auth: AuthMethod::Both,
    },
    ProviderCatalogEntry {
        id: "anthropic",
        display_name: "Anthropic",
        endpoint: Some("https://api.anthropic.com/v1/messages"),
        auth: AuthMethod::Both,
    },
    ProviderCatalogEntry {
        id: "google",
        display_name: "Google Gemini",
        endpoint: Some("https://generativelanguage.googleapis.com/v1beta/chat/completions"),
        auth: AuthMethod::Both,
    },
    ProviderCatalogEntry {
        id: "github-copilot",
        display_name: "GitHub Copilot",
        endpoint: Some("https://api.githubcopilot.com/chat/completions"),
        auth: AuthMethod::Both,
    },
    ProviderCatalogEntry {
        id: "github-models",
        display_name: "GitHub Models",
        endpoint: Some("https://models.github.ai/inference/chat/completions"),
        auth: AuthMethod::Both,
    },
    ProviderCatalogEntry {
        id: "azure",
        display_name: "Azure OpenAI",
        endpoint: None,
        auth: AuthMethod::Both,
    },
    ProviderCatalogEntry {
        id: "amazon-bedrock",
        display_name: "Amazon Bedrock",
        endpoint: None,
        auth: AuthMethod::Both,
    },
    ProviderCatalogEntry {
        id: "vercel",
        display_name: "Vercel AI Gateway",
        endpoint: Some("https://ai-gateway.vercel.sh/v1/chat/completions"),
        auth: AuthMethod::Both,
    },
    // --- Tier 2: Popular OpenAI-compatible -----------------------------
    ProviderCatalogEntry {
        id: "openrouter",
        display_name: "OpenRouter",
        endpoint: Some("https://openrouter.ai/api/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "mistral",
        display_name: "Mistral",
        endpoint: Some("https://api.mistral.ai/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "groq",
        display_name: "Groq",
        endpoint: Some("https://api.groq.com/openai/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "deepseek",
        display_name: "DeepSeek",
        endpoint: Some("https://api.deepseek.com/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "xai",
        display_name: "xAI (Grok)",
        endpoint: Some("https://api.x.ai/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "cohere",
        display_name: "Cohere",
        endpoint: None, // Uses /v1/chat, not /chat/completions
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "togetherai",
        display_name: "Together AI",
        endpoint: Some("https://api.together.xyz/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "fireworks-ai",
        display_name: "Fireworks AI",
        endpoint: Some("https://api.fireworks.ai/inference/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "perplexity",
        display_name: "Perplexity",
        endpoint: Some("https://api.perplexity.ai/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "huggingface",
        display_name: "Hugging Face",
        endpoint: Some("https://router.huggingface.co/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "nvidia",
        display_name: "Nvidia NIM",
        endpoint: Some("https://integrate.api.nvidia.com/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "cerebras",
        display_name: "Cerebras",
        endpoint: Some("https://api.cerebras.ai/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "perplexity-agent",
        display_name: "Perplexity (Sonar API)",
        endpoint: Some("https://api.perplexity.ai/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    // --- Tier 3: OpenAI-compatible routers ------------------------------
    ProviderCatalogEntry {
        id: "302ai",
        display_name: "302.AI",
        endpoint: Some("https://api.302.ai/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "ai-router",
        display_name: "AI-ROUTER",
        endpoint: Some("https://api.ai-router.dev/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "aihubmix",
        display_name: "AIHubMix",
        endpoint: Some("https://api.aihubmix.com/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "abacus",
        display_name: "Abacus",
        endpoint: Some("https://routellm.abacus.ai/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "requesty",
        display_name: "Requesty",
        endpoint: Some("https://router.requesty.ai/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "helicone",
        display_name: "Helicone",
        endpoint: Some("https://ai-gateway.helicone.ai/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "novita-ai",
        display_name: "NovitaAI",
        endpoint: Some("https://api.novita.ai/openai/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "anyapi",
        display_name: "AnyAPI",
        endpoint: Some("https://api.anyapi.ai/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    // --- Tier 4: Chinese providers --------------------------------------
    ProviderCatalogEntry {
        id: "alibaba",
        display_name: "Alibaba (intl)",
        endpoint: Some("https://dashscope-intl.aliyuncs.com/compatible-mode/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "alibaba-cn",
        display_name: "Alibaba (China)",
        endpoint: Some("https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "alibaba-coding-plan",
        display_name: "Alibaba Coding Plan (intl)",
        endpoint: Some("https://dashscope-intl.aliyuncs.com/compatible-mode/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "alibaba-coding-plan-cn",
        display_name: "Alibaba Coding Plan (China)",
        endpoint: Some("https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "alibaba-token-plan",
        display_name: "Alibaba Token Plan (intl)",
        endpoint: Some("https://dashscope-intl.aliyuncs.com/compatible-mode/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "alibaba-token-plan-cn",
        display_name: "Alibaba Token Plan (China)",
        endpoint: Some("https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "moonshotai",
        display_name: "Moonshot AI",
        endpoint: Some("https://api.moonshot.ai/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "moonshotai-cn",
        display_name: "Moonshot AI (China)",
        endpoint: Some("https://api.moonshot.cn/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "zhipuai",
        display_name: "Zhipu AI",
        endpoint: Some("https://open.bigmodel.cn/api/paas/v4/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "minimax",
        display_name: "MiniMax",
        endpoint: Some("https://api.minimax.io/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "stepfun",
        display_name: "StepFun (China)",
        endpoint: Some("https://api.stepfun.com/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "stepfun-ai",
        display_name: "StepFun (Global)",
        endpoint: Some("https://api.stepfun.ai/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "siliconflow",
        display_name: "SiliconFlow",
        endpoint: Some("https://api.siliconflow.com/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "siliconflow-cn",
        display_name: "SiliconFlow (China)",
        endpoint: Some("https://api.siliconflow.cn/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "modelscope",
        display_name: "ModelScope",
        endpoint: Some("https://api-inference.modelscope.cn/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "bailing",
        display_name: "Bailing",
        endpoint: Some("https://api.tbox.cn/api/llm/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "kimi-for-coding",
        display_name: "Kimi For Coding",
        endpoint: Some("https://api.kimi.com/coding/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    // --- Tier 5: Local / no-auth ---------------------------------------
    ProviderCatalogEntry {
        id: "ollama",
        display_name: "Ollama (local)",
        endpoint: Some("http://localhost:11434/api/chat"),
        auth: AuthMethod::Local,
    },
    ProviderCatalogEntry {
        id: "lmstudio",
        display_name: "LMStudio (local)",
        endpoint: Some("http://localhost:1234/v1/chat/completions"),
        auth: AuthMethod::Local,
    },
    ProviderCatalogEntry {
        id: "atomic-chat",
        display_name: "Atomic Chat (local)",
        endpoint: Some("http://127.0.0.1:1337/v1/chat/completions"),
        auth: AuthMethod::Local,
    },
    // --- Tier 6: Other visible in OpenCode TUI ------------------------
    ProviderCatalogEntry {
        id: "ambient",
        display_name: "Ambient",
        endpoint: Some("https://api.ambient.xyz/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "auriko",
        display_name: "Auriko",
        endpoint: Some("https://api.auriko.ai/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "azure-cognitive-services",
        display_name: "Azure Cognitive Services",
        endpoint: None,
        auth: AuthMethod::ApiKey,
    },
    ProviderCatalogEntry {
        id: "tinfoil",
        display_name: "Tinfoil",
        endpoint: Some("https://inference.tinfoil.sh/v1/chat/completions"),
        auth: AuthMethod::ApiKey,
    },
];

/// Lookup a catalog entry by provider id.
pub fn lookup(id: &str) -> Option<&'static ProviderCatalogEntry> {
    PROVIDER_CATALOG.iter().find(|e| e.id == id)
}

/// Default endpoint for a known provider. Returns `None` for unknown
/// providers and for known providers that require an account-specific URL
/// (Azure, Bedrock, Cohere, etc.).
pub fn default_endpoint(id: &str) -> Option<&'static str> {
    lookup(id).and_then(|e| e.endpoint)
}

/// Display name for a known provider, falling back to the raw id.
pub fn display_name(id: &str) -> &str {
    match lookup(id) {
        Some(e) => e.display_name,
        None => id,
    }
}

/// Returns `true` if `id` is in the catalog.
pub fn is_known(id: &str) -> bool {
    lookup(id).is_some()
}

/// Returns `true` if `id` should be marked "recommended" in the TUI.
pub fn is_recommended_provider(id: &str) -> bool {
    is_recommended(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_no_duplicate_ids() {
        let mut ids: Vec<&str> = PROVIDER_CATALOG.iter().map(|e| e.id).collect();
        ids.sort_unstable();
        let dups: Vec<&str> = ids
            .windows(2)
            .filter(|w| w[0] == w[1])
            .map(|w| w[0])
            .collect();
        assert!(dups.is_empty(), "duplicate provider ids: {dups:?}");
    }

    #[test]
    fn catalog_ids_are_lowercase_kebab() {
        // Provider ids must pass the model::validate_provider rules.
        for entry in PROVIDER_CATALOG {
            let valid = entry
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
            assert!(
                valid,
                "catalog id `{}` must be lowercase ascii with -/_",
                entry.id
            );
        }
    }

    #[test]
    fn catalog_endpoints_are_https_or_localhost() {
        for entry in PROVIDER_CATALOG {
            let Some(ep) = entry.endpoint else { continue };
            assert!(
                ep.starts_with("https://")
                    || ep.starts_with("http://localhost")
                    || ep.starts_with("http://127.0.0.1"),
                "endpoint for `{}` must be https or localhost, got: {ep}",
                entry.id
            );
        }
    }

    #[test]
    fn lookup_returns_known_provider() {
        assert_eq!(lookup("openai").map(|e| e.display_name), Some("OpenAI"));
        assert_eq!(
            lookup("anthropic").map(|e| e.display_name),
            Some("Anthropic")
        );
    }

    #[test]
    fn lookup_returns_none_for_unknown() {
        assert!(lookup("unknown-provider").is_none());
    }

    #[test]
    fn default_endpoint_for_openai() {
        assert!(
            default_endpoint("openai")
                .unwrap()
                .starts_with("https://api.openai.com")
        );
    }

    #[test]
    fn default_endpoint_none_for_azure() {
        // Azure requires account-specific URL.
        assert!(default_endpoint("azure").is_none());
    }

    #[test]
    fn display_name_falls_back_to_id() {
        assert_eq!(display_name("openai"), "OpenAI");
        assert_eq!(display_name("custom-unknown"), "custom-unknown");
    }

    #[test]
    fn is_known_distinguishes_known_from_unknown() {
        assert!(is_known("ollama"));
        assert!(!is_known("not-a-real-provider"));
    }

    #[test]
    fn auth_method_supports_api_key_correctly() {
        assert!(AuthMethod::ApiKey.supports_api_key());
        assert!(AuthMethod::Both.supports_api_key());
        assert!(AuthMethod::Local.supports_api_key());
        assert!(!AuthMethod::OAuth.supports_api_key());
    }

    #[test]
    fn auth_method_as_str_round_trips_known_values() {
        assert_eq!(AuthMethod::ApiKey.as_str(), "api_key");
        assert_eq!(AuthMethod::OAuth.as_str(), "oauth");
        assert_eq!(AuthMethod::Both.as_str(), "both");
        assert_eq!(AuthMethod::Local.as_str(), "local");
    }

    #[test]
    fn recommended_providers_are_marked() {
        // Recommended providers should be a small, stable set.
        assert!(is_recommended_provider("opencode"));
        assert!(is_recommended_provider("openai"));
        assert!(!is_recommended_provider("302ai"));
    }

    #[test]
    fn catalog_includes_screenshot_providers() {
        // Every provider visible in the OpenCode auth login screenshot
        // should be in our catalog.
        let screenshot_ids = [
            "opencode",
            "openai",
            "github-copilot",
            "google",
            "anthropic",
            "openrouter",
            "vercel",
            "302ai",
            "ai-router",
            "aihubmix",
            "abacus",
            "alibaba",
            "alibaba-cn",
            "alibaba-coding-plan",
            "alibaba-coding-plan-cn",
            "alibaba-token-plan",
            "alibaba-token-plan-cn",
            "amazon-bedrock",
            "ambient",
            "anyapi",
            "atomic-chat",
            "auriko",
            "azure",
            "azure-cognitive-services",
            "bailing",
        ];
        for id in screenshot_ids {
            assert!(
                is_known(id),
                "provider `{id}` from OpenCode screenshot missing from catalog"
            );
        }
    }

    #[test]
    fn catalog_size_is_reasonable() {
        // Sanity: at least the 25 screenshot providers + the popular additions.
        assert!(
            PROVIDER_CATALOG.len() >= 30,
            "catalog should have at least 30 entries, got {}",
            PROVIDER_CATALOG.len()
        );
    }
}
