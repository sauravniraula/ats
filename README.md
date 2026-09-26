# ATS discovery tools

ATS is a Rust Cargo workspace split into a reusable discovery library and a stdio MCP executable:

- `ats-llm/` — library APIs for web search and free OpenRouter text-LLM discovery.
- `ats-mcp/` — `ats-mcp` executable; owns JSON-RPC/MCP framing, schemas, and dispatch, and delegates calls to `ats-llm`.

## Build and verify

Run these commands from this workspace root:

```sh
cargo build -p ats-mcp --release
cargo test --workspace
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --release
```

MCP clients should launch the release binary directly (stdio transport):

```json
{
  "mcpServers": {
    "ats": {
      "command": "/absolute/path/to/ats/target/release/ats-mcp",
      "args": []
    }
  }
}
```

The server keeps stdout reserved for newline-delimited JSON-RPC 2.0 messages. It supports MCP `initialize`, `ping`, `tools/list`, `tools/call`, and notifications (which receive no response). Tool results return text plus structured JSON content.

## Tools

`web_search` takes required `query` (1–300 bytes) and optional `limit` (1–20, default 10). It searches DuckDuckGo HTML by default, or a configured SearXNG JSON endpoint. Results include title, URL, and snippet; snippets are third-party unverified content.

`list_free_llms` returns a live catalog of OpenRouter text models meeting the conservative free criteria: `:free` ID, explicitly zero prompt and completion prices, and no nonzero request or conditional override pricing. Supported sorts: `intelligence` (default), `coding`, `agentic`, `throughput`, `context`, `newest`, `name`. Optional filters are `query`, `tools_only`, and `min_context`; pagination uses `offset` and `limit`. Filters apply before sorting/pagination. Benchmark scores come from Artificial Analysis as relayed by OpenRouter; unknown scores are null and sort last. Throughput is only OpenRouter's server-side p50 order: numeric rates are not published, `throughput_tokens_per_second` remains null, and `throughput_order` is an ordinal, not a speed measurement.

## Configuration and sources

No API keys are needed. `ATS_MODELS_URL` overrides the OpenRouter models endpoint; `ATS_SEARCH_URL` overrides the search URL; `ATS_SEARCH_BACKEND` is `duckduckgo` (default) or `searxng`. For SearXNG set `ATS_SEARCH_BACKEND=searxng` and `ATS_SEARCH_URL` to a JSON-enabled instance. Only use trusted endpoints: queries and requests are sent to the configured services.

HTTP connect/overall timeouts are 5/15 seconds; responses over 8 MiB and protocol lines over 1 MiB are rejected. Provider HTTP errors, timeouts, malformed data, and invalid arguments become tool errors. Live catalog/search availability can change and free quotas are provider-controlled.

Sources: [OpenRouter Models API](https://openrouter.ai/docs/overview/models), [Artificial Analysis](https://artificialanalysis.ai/), [DuckDuckGo HTML](https://html.duckduckgo.com/html/), and [SearXNG Search API](https://docs.searxng.org/dev/search_api.html).
