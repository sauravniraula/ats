# ats-llm library

`ats-llm` provides reusable APIs for web search and free OpenRouter model discovery. Each API accepts a JSON object conforming to the MCP tool argument schema and returns structured `serde_json::Value` data or a descriptive `String` error.

```rust,no_run
use serde_json::json;

fn main() -> Result<(), String> {
    let matches = ats_llm::list_free_llms(&json!({
        "sort": "intelligence",
        "tools_only": true,
        "limit": 5,
    }))?;
    println!("{}", matches["models"]);

    let pages = ats_llm::web_search(&json!({"query": "Rust MCP protocol", "limit": 3}))?;
    println!("{}", pages["results"]);
    Ok(())
}
```

The library reads optional endpoint configuration from `ATS_MODELS_URL`, `ATS_SEARCH_URL`, and `ATS_SEARCH_BACKEND`; see the workspace [README](../README.md) for argument schemas, data provenance, operational limits, build, and test instructions. The library owns discovery logic only; the `ats-mcp` crate owns MCP protocol handling.
