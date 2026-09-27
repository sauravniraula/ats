# ats-llm library

`ats-llm` provides reusable APIs for web search and multi-provider model discovery. Each API accepts a JSON object conforming to the tool argument schema and returns structured `serde_json::Value` data or a descriptive `String` error. The `list_free_llms` API combines keyless official public sources for OpenRouter, Groq, Cloudflare Workers AI, Google Gemini API, Nous Portal, and Hugging Face Hub with an optional configurable OpenAI-compatible catalog. Built-in listing does not read or send inference credentials. Public listings do not guarantee hosted inference access; an upstream failure is reported per source without discarding successful results.

```rust,no_run
use serde_json::json;

fn main() -> Result<(), String> {
    let matches = ats_llm::list_free_llms(&json!({
        "provider": "gemini",
        "access_classification": "model_price_zero",
        "sort": "name",
        "limit": 5,
    }))?;
    println!("{}", matches["models"]);
    println!("{}", matches["providers"]);

    let pages = ats_llm::web_search(&json!({"query": "Rust MCP protocol", "limit": 3}))?;
    println!("{}", pages["results"]);
    Ok(())
}
```

Provider configuration (including `ATS_OPENAI_COMPATIBLE_NAME`, `ATS_OPENAI_COMPATIBLE_BASE_URL`, and optional `ATS_OPENAI_COMPATIBLE_API_KEY`), public-source behavior, official endpoint references, access classifications, quota caveats, filtering, and operational limits are documented in the workspace [README](../README.md). The optional generic connector is the only listing source that can use a configured bearer key; built-in sources are always unauthenticated. Hugging Face results describe public repositories for self-hosting subject to their licenses—not free hosted inference. Catalog sources do not provide comparable benchmarks across all providers, so missing scores and throughput remain unknown.

The library owns discovery logic only; the `ats-mcp` crate owns MCP protocol handling.
