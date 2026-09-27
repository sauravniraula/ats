//! Reusable web search and aggregated multi-provider LLM discovery APIs.
#![allow(dead_code)]
//!
//! [`web_search`] accepts a JSON object with `query` and optional `limit` (1–20).
//! [`list_free_llms`] accepts the documented catalog filters, sorting and pagination
//! fields. Both return structured JSON or a human-readable error. Endpoints can be
//! configured with `ATS_SEARCH_URL`, `ATS_SEARCH_BACKEND`, and `ATS_MODELS_URL`.
use std::cmp::Ordering;
use std::env;
use std::io::Read;
use std::time::Duration;

use reqwest::blocking::Client;
use scraper::{Html, Selector};
use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;

mod aggregate;

const MODEL_URL: &str = "https://openrouter.ai/api/v1/models";
const DDG_URL: &str = "https://html.duckduckgo.com/html/";
const MAX_BODY: u64 = 8 * 1024 * 1024;

#[derive(Clone)]
pub struct ToolConfig {
    pub models_url: String,
    pub search_url: String,
    pub search_backend: String,
}

impl ToolConfig {
    pub fn from_env() -> Self {
        Self {
            models_url: env::var("ATS_MODELS_URL").unwrap_or_else(|_| MODEL_URL.into()),
            search_url: env::var("ATS_SEARCH_URL").unwrap_or_else(|_| DDG_URL.into()),
            search_backend: env::var("ATS_SEARCH_BACKEND").unwrap_or_else(|_| "duckduckgo".into()),
        }
    }
}

#[derive(Deserialize)]
struct Catalog {
    data: Vec<Model>,
    total_count: usize,
}

#[derive(Deserialize)]
struct Model {
    id: String,
    name: String,
    context_length: Option<u64>,
    created: Option<i64>,
    pricing: Option<Pricing>,
    benchmarks: Option<Benchmarks>,
    #[serde(default)]
    supported_parameters: Vec<String>,
}

#[derive(Deserialize)]
struct Pricing {
    prompt: String,
    completion: String,
    request: Option<String>,
    #[serde(default)]
    overrides: Vec<PriceOverride>,
}

#[derive(Deserialize)]
struct PriceOverride {
    prompt: Option<String>,
    completion: Option<String>,
    request: Option<String>,
}

#[derive(Deserialize)]
struct Benchmarks {
    artificial_analysis: Option<Indices>,
}

#[derive(Deserialize)]
struct Indices {
    intelligence_index: Option<f64>,
    coding_index: Option<f64>,
    agentic_index: Option<f64>,
}

// API prices are decimal strings. Only explicitly zero prices prove free status.
fn zero_price(value: &str) -> bool {
    // Avoid floating-point underflow silently turning a tiny positive price into zero.
    let (integer, fraction) = value.split_once('.').unwrap_or((value, ""));
    !integer.is_empty()
        && integer.bytes().all(|b| b == b'0')
        && fraction.bytes().all(|b| b == b'0')
        && (!value.contains('.') || !fraction.is_empty())
}

fn is_free(m: &Model) -> bool {
    if !m.id.ends_with(":free") {
        return false;
    }
    let Some(p) = &m.pricing else { return false };
    zero_price(&p.prompt)
        && zero_price(&p.completion)
        && p.request.as_deref().is_none_or(zero_price)
        && p.overrides.iter().all(|o| {
            o.prompt.as_deref().is_none_or(zero_price)
                && o.completion.as_deref().is_none_or(zero_price)
                && o.request.as_deref().is_none_or(zero_price)
        })
}

fn index(m: &Model, sort: &str) -> Option<f64> {
    let b = m.benchmarks.as_ref()?.artificial_analysis.as_ref()?;
    let score = match sort {
        "intelligence" => b.intelligence_index,
        "coding" => b.coding_index,
        "agentic" => b.agentic_index,
        _ => None,
    };
    score.filter(|n| n.is_finite())
}

fn compare_optional<T: Ord>(a: Option<T>, b: Option<T>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => b.cmp(&a),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn sort_models(models: &mut [Model], sort: &str) {
    if sort == "throughput" {
        // The API only exposes its sorted order, not numeric p50 throughput.
        return;
    }
    models.sort_by(|a, b| {
        let order = match sort {
            "intelligence" | "coding" | "agentic" => {
                let (x, y) = (index(a, sort), index(b, sort));
                match (x, y) {
                    (Some(x), Some(y)) => y.total_cmp(&x),
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    _ => Ordering::Equal,
                }
            }
            "context" => compare_optional(a.context_length, b.context_length),
            "newest" => compare_optional(a.created, b.created),
            "name" => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            _ => Ordering::Equal,
        };
        order.then_with(|| a.id.cmp(&b.id))
    });
}

fn http_get(client: &Client, url: &str) -> Result<String, String> {
    let response = client
        .get(url)
        .send()
        .map_err(|e| format!("Request to {url} failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "GET {url} returned HTTP {} (check source availability, credentials or rate limits)",
            response.status()
        ));
    }
    if response.content_length().is_some_and(|n| n > MAX_BODY) {
        return Err("Upstream response exceeds 8 MiB limit".into());
    }
    let mut body = Vec::new();
    response
        .take(MAX_BODY + 1)
        .read_to_end(&mut body)
        .map_err(|e| format!("Reading upstream response failed: {e}"))?;
    if body.len() as u64 > MAX_BODY {
        return Err("Upstream response exceeds 8 MiB limit".into());
    }
    String::from_utf8(body).map_err(|e| format!("Upstream returned non-UTF-8 response: {e}"))
}

fn bounded_string<'a>(args: &'a Value, name: &str, max: usize) -> Result<Option<&'a str>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if !s.trim().is_empty() && s.len() <= max => Ok(Some(s.trim())),
        _ => Err(format!(
            "{name} must be a non-empty string of at most {max} bytes"
        )),
    }
}

fn bounded_number(args: &Value, name: &str, default: u64, max: u64) -> Result<u64, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_u64()
            .filter(|n| *n <= max)
            .ok_or_else(|| format!("{name} must be an integer from 0 to {max}")),
    }
}

fn reject_unknown(args: &Value, fields: &[&str]) -> Result<(), String> {
    for key in args.as_object().expect("validated arguments object").keys() {
        if !fields.contains(&key.as_str()) {
            return Err(format!("Unknown argument: {key}"));
        }
    }
    Ok(())
}

fn model_catalog(client: &Client, config: &ToolConfig, args: &Value) -> Result<Value, String> {
    reject_unknown(
        args,
        &[
            "sort",
            "query",
            "tools_only",
            "min_context",
            "offset",
            "limit",
        ],
    )?;
    let sort = bounded_string(args, "sort", 32)?.unwrap_or("intelligence");
    if ![
        "intelligence",
        "coding",
        "agentic",
        "throughput",
        "context",
        "newest",
        "name",
    ]
    .contains(&sort)
    {
        return Err(
            "sort must be intelligence, coding, agentic, throughput, context, newest, or name"
                .into(),
        );
    }
    let query = bounded_string(args, "query", 120)?.map(str::to_lowercase);
    let tool_support = match args.get("tools_only") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::Bool(true)) => true,
        _ => return Err("tools_only must be a boolean".into()),
    };
    let min_context = bounded_number(args, "min_context", 0, 10_000_000)?;
    let offset = bounded_number(args, "offset", 0, 100_000)? as usize;
    let limit = bounded_number(args, "limit", 20, 100)? as usize;
    if limit == 0 {
        return Err("limit must be between 1 and 100".into());
    }

    let mut url =
        Url::parse(&config.models_url).map_err(|e| format!("Invalid ATS_MODELS_URL: {e}"))?;
    if sort == "throughput" {
        url.query_pairs_mut()
            .append_pair("sort", "throughput-high-to-low");
    }
    // No upstream pagination: filter all free text models before paging.
    let body = http_get(client, url.as_str())?;
    let catalog: Catalog =
        serde_json::from_str(&body).map_err(|e| format!("OpenRouter catalog JSON invalid: {e}"))?;
    if catalog.total_count != catalog.data.len() {
        return Err(
            "OpenRouter returned an incomplete catalog; cannot sort/filter all free models".into(),
        );
    }
    let mut models: Vec<_> = catalog
        .data
        .into_iter()
        .filter(|m| {
            is_free(m)
                && query.as_ref().is_none_or(|q| {
                    m.name.to_lowercase().contains(q) || m.id.to_lowercase().contains(q)
                })
                && (!tool_support || m.supported_parameters.iter().any(|v| v == "tools"))
                && m.context_length.is_some_and(|n| n >= min_context)
        })
        .collect();
    sort_models(&mut models, sort);
    let total = models.len();
    let page = models.iter().skip(offset).take(limit).enumerate().map(|(i, m)| {
        let p = m.pricing.as_ref().expect("free models have a price");
        json!({
            "id": m.id, "name": m.name, "context_length": m.context_length,
            "created_unix": m.created, "supports_tools": m.supported_parameters.iter().any(|s| s == "tools"),
            "pricing_usd_per_token": {"prompt": p.prompt, "completion": p.completion, "request": p.request},
            "intelligence_index": index(m, "intelligence"), "coding_index": index(m, "coding"),
            "agentic_index": index(m, "agentic"),
            "throughput_tokens_per_second": Value::Null,
            "throughput_order": if sort == "throughput" {json!(offset + i + 1)} else {Value::Null},
            "model_url": format!("https://openrouter.ai/models/{}", m.id),
        })
    }).collect::<Vec<_>>();
    Ok(json!({
        "models": page, "total": total, "offset": offset, "limit": limit,
        "has_more": offset.saturating_add(limit) < total,
        "sort": sort,
        "sort_contract": if sort == "throughput" {
            "OpenRouter server-side p50 throughput-high-to-low order; numeric rates are not public in the catalog. Missing measurements are placed last by upstream; ties cannot be resolved locally. throughput_order is ordinal within the filtered set, NOT tokens/sec."
        } else {
            "Known values first; descending except name ascending; null/unknown last; ties by ID ascending. Scores use Artificial Analysis indices as relayed by OpenRouter."
        },
        "source": {"catalog_url": url.as_str(), "documentation": "https://openrouter.ai/docs/overview/models", "benchmark_origin": "Artificial Analysis via OpenRouter models.benchmarks.artificial_analysis", "fetched_live": true},
        "free_definition": "Text-output catalog models with :free ID, zero prompt/completion prices and no nonzero request or conditional override price. Optional paid add-ons may exist; free quotas and availability vary."
    }))
}

fn search(client: &Client, config: &ToolConfig, args: &Value) -> Result<Value, String> {
    reject_unknown(args, &["query", "limit"])?;
    let query = bounded_string(args, "query", 300)?.ok_or("query is required")?;
    let limit = bounded_number(args, "limit", 10, 20)? as usize;
    if limit == 0 {
        return Err("limit must be between 1 and 20".into());
    }
    let mut url =
        Url::parse(&config.search_url).map_err(|e| format!("Invalid ATS_SEARCH_URL: {e}"))?;
    let (results, backend): (Vec<Value>, &str) = match config.search_backend.as_str() {
        "duckduckgo" => {
            url.query_pairs_mut().append_pair("q", query);
            let html = http_get(client, url.as_str())?;
            let found = parse_ddg(&html, limit);
            if found.is_empty() && !html.contains("result__a") && !html.contains("no-results") {
                return Err("DuckDuckGo returned no recognizable search page (possible bot challenge or markup change); try configured SearXNG".into());
            }
            (found, "DuckDuckGo HTML (unofficial, markup may change)")
        }
        "searxng" => {
            url.set_path("/search");
            url.query_pairs_mut()
                .append_pair("q", query)
                .append_pair("format", "json");
            let text = http_get(client, url.as_str())?;
            let data: Value =
                serde_json::from_str(&text).map_err(|e| format!("SearXNG JSON invalid: {e}"))?;
            let items = data
                .get("results")
                .and_then(Value::as_array)
                .ok_or("SearXNG response lacks results array (enable JSON format)")?;
            (items.iter().filter_map(|r| {
                let title = r.get("title")?.as_str()?;
                let link = r.get("url")?.as_str()?;
                if !valid_web_url(link) { return None; }
                Some(json!({"title":title,"url":link,"snippet":r.get("content").and_then(Value::as_str).unwrap_or("")}))
            }).take(limit).collect(), "SearXNG JSON API")
        }
        _ => return Err("ATS_SEARCH_BACKEND must be duckduckgo or searxng".into()),
    };
    Ok(
        json!({"query": query, "results": results, "source": {"backend": backend, "request_url": url.as_str()}, "note": "Search result snippets are third-party content, not verified facts."}),
    )
}

fn valid_web_url(s: &str) -> bool {
    Url::parse(s).is_ok_and(|u| matches!(u.scheme(), "https" | "http") && u.host_str().is_some())
}

fn parse_ddg(html: &str, limit: usize) -> Vec<Value> {
    let document = Html::parse_document(html);
    let result_sel = Selector::parse(".result").expect("constant selector");
    let title_sel = Selector::parse(".result__a").expect("constant selector");
    let snippet_sel = Selector::parse(".result__snippet").expect("constant selector");
    let mut results = Vec::new();
    for item in document.select(&result_sel) {
        let Some(anchor) = item.select(&title_sel).next() else {
            continue;
        };
        let raw = anchor.value().attr("href").unwrap_or("");
        let link = if raw.starts_with("//duckduckgo.com/l/") {
            Url::parse(&format!("https:{raw}"))
                .ok()
                .and_then(|u| {
                    u.query_pairs()
                        .find(|(k, _)| k == "uddg")
                        .map(|(_, v)| v.into_owned())
                })
                .unwrap_or_default()
        } else {
            raw.to_owned()
        };
        if !valid_web_url(&link) {
            continue;
        }
        let title = anchor.text().collect::<Vec<_>>().join("").trim().to_owned();
        if title.is_empty() {
            continue;
        }
        let snippet = item
            .select(&snippet_sel)
            .next()
            .map(|node| node.text().collect::<Vec<_>>().join("").trim().to_owned())
            .unwrap_or_default();
        results.push(json!({"title": title, "url": link, "snippet": snippet}));
        if results.len() == limit {
            break;
        }
    }
    results
}

pub fn web_search(args: &Value) -> Result<Value, String> {
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .user_agent("ats-mcp/0.1 (MCP discovery)")
        .build()
        .map_err(|e| format!("HTTP client setup failed: {e}"))?;
    search(&client, &ToolConfig::from_env(), args)
}

pub fn list_free_llms(args: &Value) -> Result<Value, String> {
    aggregate::list_free_llms(args)
}

#[cfg(test)]
mod tests;
