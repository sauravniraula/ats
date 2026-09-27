# ATS discovery tools

ATS is a Rust Cargo workspace split into a reusable discovery library and a stdio MCP executable:

- `ats-llm/` — reusable web-search and multi-provider model-discovery APIs.
- `ats-mcp/` — MCP JSON-RPC stdio framing, schemas, and dispatch.

## Build and verify

From this workspace root: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and `cargo build --workspace --release`.

MCP clients launch `target/release/ats-mcp` over stdio. Stdout is reserved for newline-delimited JSON-RPC 2.0. Supported operations include `initialize`, `ping`, `tools/list`, and `tools/call`.

## Tools

`web_search` accepts required `query` (1–300 bytes), optional `limit` (1–20, default 10), and searches DuckDuckGo HTML or configured SearXNG JSON.

`list_free_llms` aggregates provider model catalogs, supports `provider`, `access_classification`, `query`, `tools_only`, `min_context`, `sort`, `offset`, and `limit` filters. Each response includes model provider/id, pricing/free evidence, source URL, fetch timestamp, benchmark fields, and a status/diagnostic for each selected provider. A provider failure does not suppress successful catalogs. `partial_results` indicates mixed success and failure. Select one source with `provider` or omit it to query every source.

Example arguments:

```json
{"provider":"gemini","access_classification":"model_price_zero","sort":"name","limit":20}
```

Access classifications are intentionally conservative:

- `model_price_zero`: an official public source explicitly reports zero input/output prices (OpenRouter `:free`, Gemini Free Tier, and Nous zero-priced models).
- `free_tier_eligible_unverified`: an official public source documents free/quota-limited access, but current model/account eligibility remains provider-controlled. This is not a guarantee of free use.
- `unknown`: pricing/free eligibility cannot be established from available catalog data. Unknown does not mean free.
- `open_weights_self_hosted`: a public, non-gated Hugging Face Hub repository is discoverable for self-hosting subject to its license; this does not imply hosted inference is free or even available from that provider. `model_license` is included when Hub metadata exposes a license tag; review the repository terms.

Provider matrix:

| Provider | Catalog | Credentials | Access interpretation |
|---|---|---|---|
| OpenRouter | Public `/api/v1/models` catalog | None for listing | Only explicit `:free` + zero pricing is labeled zero-price |
| Groq | Public Free Plan rate-limits page (Markdown representation) | None for listing | Models listed under Free Plan limits are quota-eligible, but signup, current quota, and account access remain unverified |
| Cloudflare Workers AI | Public Workers AI pricing page | None for listing | LLM pricing rows are quota-eligible under the documented daily free allocation unless the page explicitly marks a model as requiring paid billing; current account eligibility remains unverified |
| Google Gemini API | Public Gemini API pricing page | None for listing | Only models whose Free Tier explicitly says input and output are “Free of charge” are labeled zero-price |
| Nous Portal | Public Nous inference `/v1/models` catalog | None for listing | Only rows with explicit zero prompt and completion prices are included as zero-price |
| Hugging Face Hub | Public `/api/models` text-generation catalog (top 100 by downloads) | None for public, non-gated repositories | Public weights can be self-hosted subject to the repository license; hosted inference availability/pricing is separate and not claimed free |
| OpenAI-compatible (optional) | Configured `{base_url}/models` (`base_url` includes `/v1`) | No key required for public catalogs; optional `ATS_OPENAI_COMPATIBLE_API_KEY` bearer key | `unknown` unless the source schema has trustworthy explicit zero-price evidence (this adapter does not infer it) |

All built-in listing sources are keyless: OpenRouter and Nous use public JSON catalogs; Groq uses the official public Free Plan rate-limits Markdown representation; Cloudflare and Gemini use official public pricing pages; and Hugging Face uses the public Hub API. The listing client does not read or send provider inference credentials for these sources. Hugging Face public, non-gated text-generation repositories are labeled `open_weights_self_hosted`, not free hosted inference; this is not a license grant, so review `model_license` and repository terms. Listing is separate from inference: using any listed model can still require signup, an inference API key, quota, or a paid account. Never place keys in MCP arguments or source control. Optional trusted-source overrides are `ATS_MODELS_URL` (OpenRouter JSON), `ATS_GROQ_MODELS_URL` (Groq rate-limits Markdown), `ATS_CLOUDFLARE_MODELS_URL` (Cloudflare pricing HTML), `ATS_GEMINI_MODELS_URL` (Gemini pricing HTML), and `ATS_NOUS_MODELS_URL` (Nous JSON).

To add another provider without code changes, configure the generic OpenAI-compatible connector. Set `ATS_OPENAI_COMPATIBLE_BASE_URL` to a trusted catalog base ending in `/v1` (for example, `https://provider.example/v1`); the connector requests `/models` and expects the standard `{"data":[{"id":"..."}]}` response. Set `ATS_OPENAI_COMPATIBLE_NAME` to an optional lowercase slug (default `openai_compatible`) for provider identity/filtering, and optionally set `ATS_OPENAI_COMPATIBLE_API_KEY` for endpoints requiring bearer authentication. Public endpoints need no key. The connector never treats a provider's free quota or an unpriced catalog entry as proof of free model access; its access classification remains `unknown`. The endpoint uses bounded request/response limits and should be trusted because it receives the optional bearer key. To filter, pass the configured provider slug as `provider`.

Search configuration remains `ATS_SEARCH_URL` and `ATS_SEARCH_BACKEND` (`duckduckgo` by default or `searxng`). HTTP connect/overall timeouts are 5/15 seconds; response bodies over 8 MiB and protocol lines over 1 MiB are rejected. API keys are sent in headers, not returned in response data. Provider error diagnostics omit response bodies and secrets.

Benchmark scores are only emitted when a provider reports Artificial Analysis values (currently OpenRouter); other providers remain null. No cross-provider throughput comparison is inferred. OpenRouter throughput is an upstream ordering only, not numeric tokens/second.

## Official sources

- OpenRouter Models API: https://openrouter.ai/docs/overview/models
- Groq Free Plan rate limits (public Markdown source: `https://console.groq.com/docs/rate-limits.md`): https://console.groq.com/docs/rate-limits
- Cloudflare Workers AI model catalog: https://developers.cloudflare.com/workers-ai/models/
- Cloudflare Workers AI plans and usage pricing: https://developers.cloudflare.com/workers-ai/platform/pricing/
- Google Gemini pricing: https://ai.google.dev/gemini-api/docs/pricing
- Google Gemini billing tiers: https://ai.google.dev/gemini-api/docs/billing
- Nous public models catalog: https://inference-api.nousresearch.com/v1/models
- Nous Portal model browser: https://portal.nousresearch.com/models
- Nous Portal authentication and inference configuration: https://hermes-agent.nousresearch.com/docs/integrations/nous-portal
- Hugging Face Hub API endpoints: https://huggingface.co/docs/hub/api (public Hub catalog endpoint: `https://huggingface.co/api/models`)
- Artificial Analysis benchmark origin: https://artificialanalysis.ai/
- DuckDuckGo HTML: https://html.duckduckgo.com/html/
- SearXNG Search API: https://docs.searxng.org/dev/search_api.html
