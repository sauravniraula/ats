use std::{
    collections::BTreeSet,
    env,
    io::Read,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use reqwest::blocking::Client;
use scraper::{Html, Selector};
use serde_json::{Value, json};
use url::Url;

const MAX_BODY: u64 = 8 * 1024 * 1024;
const OR_DOCS: &str = "https://openrouter.ai/docs/overview/models";
const GROQ_DOCS: &str = "https://console.groq.com/docs/rate-limits";
const CF_DOCS: &str = "https://developers.cloudflare.com/workers-ai/models/";
const GEMINI_DOCS: &str = "https://ai.google.dev/gemini-api/docs/pricing";
const NOUS_DOCS: &str = "https://portal.nousresearch.com/models";
const OPENAI_COMPATIBLE_DOCS: &str = "https://platform.openai.com/docs/api-reference/models/list";
const HF_DOCS: &str = "https://huggingface.co/docs/hub/api";

#[derive(Clone)]
struct Provider {
    id: String,
    endpoint: String,
    documentation: &'static str,
    auth: Auth,
    kind: Kind,
}
#[derive(Clone)]
enum Auth {
    None,
    Bearer(String),
}
#[derive(Clone, Copy)]
enum Kind {
    OpenRouter,
    Groq,
    Cloudflare,
    Gemini,
    Nous,
    OpenAiCompatible,
    HuggingFace,
}

fn env_provider(
    id: &str,
    var: &str,
    endpoint: &str,
    documentation: &'static str,
    kind: Kind,
) -> Provider {
    Provider {
        id: id.to_owned(),
        endpoint: env::var(var).unwrap_or_else(|_| endpoint.into()),
        documentation,
        auth: Auth::None,
        kind,
    }
}

fn providers() -> Vec<(Provider, Option<String>)> {
    let or = env_provider(
        "openrouter",
        "ATS_MODELS_URL",
        "https://openrouter.ai/api/v1/models",
        OR_DOCS,
        Kind::OpenRouter,
    );
    let groq = env_provider(
        "groq",
        "ATS_GROQ_MODELS_URL",
        "https://console.groq.com/docs/rate-limits.md",
        GROQ_DOCS,
        Kind::Groq,
    );
    let cf = env_provider(
        "cloudflare",
        "ATS_CLOUDFLARE_MODELS_URL",
        "https://developers.cloudflare.com/workers-ai/platform/pricing/",
        CF_DOCS,
        Kind::Cloudflare,
    );
    let gemini = env_provider(
        "gemini",
        "ATS_GEMINI_MODELS_URL",
        "https://ai.google.dev/gemini-api/docs/pricing",
        GEMINI_DOCS,
        Kind::Gemini,
    );
    let nous = env_provider(
        "nous",
        "ATS_NOUS_MODELS_URL",
        "https://inference-api.nousresearch.com/v1/models",
        NOUS_DOCS,
        Kind::Nous,
    );
    vec![
        (or, None),
        (groq, None),
        (cf, None),
        (gemini, None),
        (nous, None),
        huggingface_provider(),
        openai_compatible_provider(),
    ]
}

fn huggingface_provider() -> (Provider, Option<String>) {
    (
        Provider {
            id: "huggingface".into(),
            endpoint: "https://huggingface.co/api/models?pipeline_tag=text-generation&limit=100&sort=downloads&direction=-1".into(),
            documentation: HF_DOCS,
            auth: Auth::None,
            kind: Kind::HuggingFace,
        },
        None,
    )
}

fn openai_compatible_provider() -> (Provider, Option<String>) {
    let name = env::var("ATS_OPENAI_COMPATIBLE_NAME")
        .ok()
        .filter(|name| valid_provider_id(name))
        .unwrap_or_else(|| "openai_compatible".into());
    let base = env::var("ATS_OPENAI_COMPATIBLE_BASE_URL")
        .ok()
        .filter(|base| !base.trim().is_empty());
    let endpoint = base
        .as_deref()
        .map(|base| format!("{}/models", base.trim_end_matches('/')))
        .unwrap_or_default();
    let mut provider = Provider {
        id: name,
        endpoint,
        documentation: OPENAI_COMPATIBLE_DOCS,
        auth: Auth::None,
        kind: Kind::OpenAiCompatible,
    };
    let status = match base {
        Some(base) if valid_catalog_base(&base) => {
            provider.auth = env::var("ATS_OPENAI_COMPATIBLE_API_KEY")
                .ok()
                .filter(|key| !key.trim().is_empty())
                .map(Auth::Bearer)
                .unwrap_or(Auth::None);
            None
        }
        Some(_) => Some("invalid ATS_OPENAI_COMPATIBLE_BASE_URL: expected an https URL ending in /v1 (http is allowed for localhost)".into()),
        None => Some("not_configured: set ATS_OPENAI_COMPATIBLE_BASE_URL".into()),
    };
    (provider, status)
}

fn valid_provider_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        && value.as_bytes()[0].is_ascii_lowercase()
}

fn valid_catalog_base(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| {
        url.username().is_empty()
            && url.password().is_none()
            && url.path().trim_end_matches('/').ends_with("/v1")
            && (url.scheme() == "https"
                || (url.scheme() == "http"
                    && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
            && url.query().is_none()
            && url.fragment().is_none()
    })
}

fn public_url(value: &str) -> String {
    Url::parse(value)
        .map(|mut url| {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        })
        .unwrap_or_else(|_| "invalid configured URL".into())
}
fn fetched_at() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn get_body(client: &Client, provider: &Provider, url: &str) -> Result<Vec<u8>, String> {
    let mut req = client.get(url);
    req = match &provider.auth {
        Auth::None => req,
        Auth::Bearer(key) => req.bearer_auth(key),
    };
    let response = req
        .send()
        .map_err(|e| format!("request failed: {}", e.without_url()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    if response.content_length().is_some_and(|n| n > MAX_BODY) {
        return Err("upstream response exceeds 8 MiB limit".into());
    }
    let mut body = Vec::new();
    response
        .take(MAX_BODY + 1)
        .read_to_end(&mut body)
        .map_err(|_| "reading response failed".to_string())?;
    if body.len() as u64 > MAX_BODY {
        return Err("upstream response exceeds 8 MiB limit".into());
    }
    Ok(body)
}

fn get(client: &Client, provider: &Provider, url: &str) -> Result<Value, String> {
    let body = get_body(client, provider, url)?;
    serde_json::from_slice(&body).map_err(|_| "provider returned malformed JSON".into())
}

fn get_text(client: &Client, provider: &Provider, url: &str) -> Result<String, String> {
    String::from_utf8(get_body(client, provider, url)?)
        .map_err(|_| "provider returned non-UTF-8 public catalog content".into())
}

fn safe_model_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/' | b':' | b'@')
        })
}

fn number_from_text(value: &str) -> Option<u64> {
    let digits: String = value
        .bytes()
        .filter(u8::is_ascii_digit)
        .map(char::from)
        .collect();
    (!digits.is_empty()).then(|| digits.parse().ok()).flatten()
}

fn parse_groq_markdown(body: &str) -> Result<Vec<Value>, String> {
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for line in body.lines().filter(|line| line.starts_with('|')) {
        let columns: Vec<&str> = line
            .split('|')
            .map(str::trim)
            .filter(|column| !column.is_empty())
            .collect();
        if columns.len() < 6 {
            continue;
        }
        let Some((id, requests_per_minute)) = columns
            .first()
            .zip(columns.get(1).and_then(|value| number_from_text(value)))
        else {
            continue;
        };
        if !safe_model_id(id) || !seen.insert((*id).to_owned()) {
            continue;
        }
        rows.push(json!({
            "id": id,
            "name": id,
            "rate_limit_requests_per_minute": requests_per_minute,
            "model_url": format!("https://console.groq.com/docs/model/{id}"),
            "_access_classification": "free_tier_eligible_unverified",
            "_free_evidence": "Groq's public rate-limits page lists this model under Free Plan limits; signup, current quota, and account eligibility remain provider-controlled",
        }));
    }
    if rows.is_empty() {
        Err("Groq public models page did not contain recognizable model rows".into())
    } else {
        Ok(rows)
    }
}

fn parse_cloudflare_pricing_html(body: &str) -> Result<Vec<Value>, String> {
    let document = Html::parse_document(body);
    let paragraph_selector = Selector::parse("article p").expect("static selector");
    let code_selector = Selector::parse("code").expect("static selector");
    let content_selector = Selector::parse("article h2, article table").expect("static selector");
    let row_selector = Selector::parse("tr").expect("static selector");
    let cell_selector = Selector::parse("td").expect("static selector");
    let mut paid_only = BTreeSet::new();
    for paragraph in document.select(&paragraph_selector) {
        let text = paragraph.text().collect::<Vec<_>>().join(" ");
        if text.contains("Some models require a paid billing method") {
            for code in paragraph.select(&code_selector) {
                let id = code.text().collect::<String>();
                if safe_model_id(&id) {
                    paid_only.insert(id);
                }
            }
        }
    }

    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    let mut in_llm_pricing = false;
    for element in document.select(&content_selector) {
        match element.value().name() {
            "h2" => {
                in_llm_pricing = element
                    .text()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .contains("LLM model pricing");
            }
            "table" if in_llm_pricing => {
                for row in element.select(&row_selector) {
                    let Some(id) = row
                        .select(&cell_selector)
                        .next()
                        .map(|cell| cell.text().collect::<Vec<_>>().join(" ").trim().to_owned())
                    else {
                        continue;
                    };
                    if !id.starts_with("@cf/") || !safe_model_id(&id) || !seen.insert(id.clone()) {
                        continue;
                    }
                    let (classification, evidence) = if paid_only.contains(&id) {
                        (
                            "unknown",
                            "Cloudflare's public pricing page explicitly says this model requires a paid billing method; free allocation eligibility is not asserted",
                        )
                    } else {
                        (
                            "free_tier_eligible_unverified",
                            "Cloudflare's public pricing page documents 10,000 Neurons per day at no charge; current per-model and account eligibility remain provider-controlled",
                        )
                    };
                    rows.push(json!({
                        "id": id,
                        "name": id,
                        "model_url": "https://developers.cloudflare.com/workers-ai/platform/pricing/",
                        "_access_classification": classification,
                        "_free_evidence": evidence,
                    }));
                }
            }
            _ => {}
        }
    }
    if rows.is_empty() {
        Err("Cloudflare public pricing page did not contain recognizable LLM model rows".into())
    } else {
        Ok(rows)
    }
}

fn parse_gemini_pricing_html(body: &str) -> Result<Vec<Value>, String> {
    let document = Html::parse_document(body);
    let content_selector =
        Selector::parse("article h2, article code, article table").expect("static selector");
    let row_selector = Selector::parse("tr").expect("static selector");
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current_name: Option<String> = None;
    let mut current_id: Option<String> = None;
    let mut free_input = false;
    let mut free_output = false;
    let mut finish_section = |name: &Option<String>,
                              id: &Option<String>,
                              input: bool,
                              output: bool| {
        let Some(id) = id
            .as_ref()
            .filter(|id| input && output && seen.insert((*id).clone()))
        else {
            return;
        };
        rows.push(json!({
                "id": id,
                "name": name.as_deref().unwrap_or(id),
                "pricing": {"prompt": "0", "completion": "0"},
                "model_url": format!("https://ai.google.dev/gemini-api/docs/pricing#{}", id),
                "_access_classification": "model_price_zero",
                "_free_evidence": "Google's public Gemini pricing page explicitly lists Free of charge input and output for this model's Free Tier; inference still requires provider signup and is quota-limited",
            }));
    };

    for element in document.select(&content_selector) {
        match element.value().name() {
            "h2" => {
                finish_section(&current_name, &current_id, free_input, free_output);
                current_name = Some(
                    element
                        .text()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .trim()
                        .to_owned(),
                );
                current_id = None;
                free_input = false;
                free_output = false;
            }
            "code" if current_id.is_none() => {
                let candidate = element.text().collect::<String>();
                if candidate.starts_with("gemini-") && safe_model_id(&candidate) {
                    current_id = Some(candidate);
                }
            }
            "table" if current_id.is_some() => {
                let table_text = element.text().collect::<Vec<_>>().join(" ");
                if table_text.contains("Free Tier") {
                    for row in element.select(&row_selector) {
                        let text = row.text().collect::<Vec<_>>().join(" ");
                        if text.contains("Input price") && text.contains("Free of charge") {
                            free_input = true;
                        }
                        if text.contains("Output price") && text.contains("Free of charge") {
                            free_output = true;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    finish_section(&current_name, &current_id, free_input, free_output);
    if rows.is_empty() {
        Err(
            "Gemini public pricing page did not contain models with explicit free input and output"
                .into(),
        )
    } else {
        Ok(rows)
    }
}
fn page_url(base: &str, page: u32, token: Option<&str>, gemini: bool) -> Result<String, String> {
    let mut u = Url::parse(base).map_err(|_| "invalid configured provider URL".to_string())?;
    if !matches!(u.scheme(), "https" | "http") || u.host_str().is_none() {
        return Err("provider URL must be http(s) with a host".into());
    }
    if gemini {
        u.query_pairs_mut().append_pair("pageSize", "1000");
        if let Some(t) = token {
            u.query_pairs_mut().append_pair("pageToken", t);
        }
    } else if page > 1 {
        u.query_pairs_mut()
            .append_pair("page", &page.to_string())
            .append_pair("per_page", "100");
    } else if matches!(u.host_str(), Some("api.cloudflare.com")) {
        u.query_pairs_mut()
            .append_pair("page", "1")
            .append_pair("per_page", "100");
    }
    Ok(u.to_string())
}
fn extract_rows(kind: Kind, value: &Value) -> Result<(Vec<Value>, Option<String>, bool), String> {
    if matches!(kind, Kind::HuggingFace) {
        return value
            .as_array()
            .map(|rows| (rows.clone(), None, true))
            .ok_or_else(|| "Hugging Face returned a non-array model catalog".into());
    }
    let rows = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "provider response lacks model array".to_string())?
        .clone();
    if matches!(kind, Kind::OpenRouter)
        && value
            .get("total_count")
            .and_then(Value::as_u64)
            .is_some_and(|n| n as usize != rows.len())
    {
        return Err("OpenRouter returned an incomplete catalog".into());
    }
    Ok((rows, None, true))
}
fn normalized(provider: &Provider, row: &Value) -> Option<Value> {
    if matches!(provider.kind, Kind::HuggingFace)
        && (row.get("private").and_then(Value::as_bool) != Some(false)
            || row.get("gated").and_then(Value::as_bool) == Some(true))
    {
        return None;
    }
    let raw_id = row.get("id")?.as_str()?;
    if raw_id.is_empty() {
        return None;
    }
    if matches!(provider.kind, Kind::OpenRouter) && !openrouter_zero_free(row) {
        return None;
    }
    if matches!(provider.kind, Kind::Nous) && !catalog_prices_zero(row) {
        return None;
    }
    let id = format!("{}/{}", provider.id, raw_id);
    let name = row
        .get("displayName")
        .or_else(|| row.get("name"))
        .and_then(Value::as_str)
        .unwrap_or(raw_id);
    let model_license = row.get("tags").and_then(Value::as_array).and_then(|tags| {
        tags.iter()
            .filter_map(Value::as_str)
            .find_map(|tag| tag.strip_prefix("license:"))
    });
    let context = row
        .get("context_length")
        .or_else(|| row.get("context_window"))
        .or_else(|| row.get("inputTokenLimit"));
    let supports_tools = match provider.kind {
        Kind::Gemini | Kind::Cloudflare => None,
        _ => row
            .get("supported_parameters")
            .and_then(Value::as_array)
            .map(|a| a.iter().any(|x| x.as_str() == Some("tools"))),
    };
    let (classification, evidence) = if let (Some(classification), Some(evidence)) = (
        row.get("_access_classification").and_then(Value::as_str),
        row.get("_free_evidence").and_then(Value::as_str),
    ) {
        (classification, evidence)
    } else {
        match provider.kind {
            Kind::OpenRouter => (
                "model_price_zero",
                "OpenRouter :free variant and zero prompt/completion/request/override prices",
            ),
            Kind::Nous => (
                "model_price_zero",
                "Nous' public models API explicitly reports zero prompt and completion prices; inference still requires provider signup and may be rate-limited",
            ),
            Kind::HuggingFace => (
                "open_weights_self_hosted",
                "Public, non-gated Hugging Face Hub repository is discoverable for self-hosting subject to its license; hosted inference is not implied free",
            ),
            Kind::Groq | Kind::Cloudflare | Kind::Gemini | Kind::OpenAiCompatible => (
                "unknown",
                "Provider catalog does not expose trustworthy per-model zero pricing or free-tier entitlement evidence",
            ),
        }
    };
    let prompt_price = row.pointer("/pricing/prompt").cloned();
    let completion_price = row.pointer("/pricing/completion").cloned();
    Some(
        json!({"id":id,"provider":provider.id,"provider_model_id":raw_id,"name":name,"model_license":model_license,"context_length":context,
        "supports_tools":supports_tools,"created_unix":row.get("created"),"intelligence_index":row.pointer("/benchmarks/artificial_analysis/intelligence_index"),"coding_index":row.pointer("/benchmarks/artificial_analysis/coding_index"),"agentic_index":row.pointer("/benchmarks/artificial_analysis/agentic_index"),
        "throughput_tokens_per_second":null,"throughput_order":null,"pricing_usd_per_token":{"prompt":prompt_price,"completion":completion_price,"request":row.pointer("/pricing/request"),"overrides":row.pointer("/pricing/overrides"),"evidence":evidence},
        "access_classification":classification,"free_evidence":evidence,"source_url":public_url(&provider.endpoint),"documentation_url":provider.documentation,
        "model_url":row.get("model_url").and_then(Value::as_str).map(str::to_owned).or_else(|| provider_model_url(provider,raw_id))}),
    )
}
fn zero_price(value: Option<&Value>) -> bool {
    value.is_some_and(|value| {
        value.as_f64() == Some(0.0)
            || value.as_str().is_some_and(|price| {
                let (whole, fraction) = price.split_once('.').unwrap_or((price, ""));
                !whole.is_empty()
                    && whole.bytes().all(|byte| byte == b'0')
                    && fraction.bytes().all(|byte| byte == b'0')
            })
    })
}

fn catalog_prices_zero(row: &Value) -> bool {
    let pricing = &row["pricing"];
    zero_price(pricing.get("prompt"))
        && zero_price(pricing.get("completion"))
        && (pricing.get("request").is_none() || zero_price(pricing.get("request")))
        && pricing
            .get("overrides")
            .and_then(Value::as_array)
            .is_none_or(|overrides| {
                overrides.iter().all(|override_price| {
                    ["prompt", "completion", "request"].iter().all(|key| {
                        override_price.get(*key).is_none() || zero_price(override_price.get(*key))
                    })
                })
            })
}

fn openrouter_zero_free(row: &Value) -> bool {
    let Some(id) = row.get("id").and_then(Value::as_str) else {
        return false;
    };
    id.ends_with(":free") && catalog_prices_zero(row)
}

fn provider_model_url(provider: &Provider, model: &str) -> Option<String> {
    let base = match provider.id.as_str() {
        "openrouter" => "https://openrouter.ai/models/",
        "groq" => "https://console.groq.com/docs/models",
        "gemini" => "https://ai.google.dev/gemini-api/docs/models",
        "huggingface" => "https://huggingface.co/",
        _ => return None,
    };
    if provider.id == "groq" || provider.id == "gemini" {
        return Some(base.into());
    }
    let mut url = Url::parse(base).ok()?;
    {
        let mut segments = url.path_segments_mut().ok()?;
        segments.pop_if_empty();
        for part in model.split('/') {
            segments.push(part);
        }
    }
    Some(url.to_string())
}

fn fetch_provider(client: &Client, provider: &Provider, sort: &str) -> Result<Vec<Value>, String> {
    if matches!(provider.kind, Kind::Groq | Kind::Cloudflare | Kind::Gemini) {
        let body = get_text(client, provider, &provider.endpoint)?;
        let items = match provider.kind {
            Kind::Groq => parse_groq_markdown(&body)?,
            Kind::Cloudflare => parse_cloudflare_pricing_html(&body)?,
            Kind::Gemini => parse_gemini_pricing_html(&body)?,
            _ => unreachable!("public-page kinds matched above"),
        };
        return Ok(items
            .iter()
            .filter_map(|row| normalized(provider, row))
            .collect());
    }

    let mut rows = Vec::new();
    let mut url = Url::parse(&page_url(&provider.endpoint, 1, None, false)?)
        .map_err(|_| "invalid configured provider URL".to_string())?;
    if matches!(provider.kind, Kind::OpenRouter) && sort == "throughput" {
        url.query_pairs_mut()
            .append_pair("sort", "throughput-high-to-low");
    }
    let data = get(client, provider, url.as_str())?;
    let (items, _, _) = extract_rows(provider.kind, &data)?;
    for row in &items {
        if let Some(mut model) = normalized(provider, row) {
            if matches!(provider.kind, Kind::OpenRouter) && sort == "throughput" {
                model["throughput_order"] = json!(rows.len() + 1);
            }
            rows.push(model);
        }
    }
    Ok(rows)
}

fn listing_authentication(provider: &Provider) -> &'static str {
    match provider.auth {
        Auth::None => "none",
        Auth::Bearer(_) => "configured_bearer",
    }
}

fn fetch_providers(
    client: &Client,
    sources: Vec<(Provider, Option<String>)>,
    provider_filter: Option<&str>,
    sort: &str,
) -> (Vec<Value>, Vec<Value>) {
    let mut models = Vec::new();
    let mut statuses = Vec::new();
    for (provider, setup_error) in sources {
        if provider_filter.is_some_and(|filter| filter != provider.id) {
            continue;
        }
        let fetched = fetched_at();
        let result = if let Some(error) = setup_error {
            Err(error)
        } else {
            fetch_provider(client, &provider, sort)
        };
        match result {
            Ok(mut rows) => {
                let count = rows.len();
                for model in &mut rows {
                    model["fetched_at_unix"] = json!(fetched);
                }
                models.extend(rows);
                statuses.push(json!({"provider":provider.id,"status":"ok","model_count":count,"listing_authentication":listing_authentication(&provider),"fetched_at_unix":fetched,"source_url":public_url(&provider.endpoint),"documentation_url":provider.documentation}));
            }
            Err(error) => statuses.push(json!({"provider":provider.id,"status":if error.starts_with("not_configured") {"not_configured"} else {"error"},"diagnostic":error,"listing_authentication":listing_authentication(&provider),"fetched_at_unix":fetched,"source_url":public_url(&provider.endpoint),"documentation_url":provider.documentation})),
        }
    }
    (models, statuses)
}

pub fn list_free_llms(args: &Value) -> Result<Value, String> {
    let allowed = [
        "sort",
        "query",
        "tools_only",
        "min_context",
        "offset",
        "limit",
        "provider",
        "access_classification",
    ];
    let obj = args.as_object().ok_or("arguments must be an object")?;
    if let Some(k) = obj.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(format!("Unknown argument: {k}"));
    }
    let provider_filter = args
        .get("provider")
        .and_then(Value::as_str)
        .map(str::to_lowercase);
    if args
        .get("provider")
        .is_some_and(|v| !v.is_null() && !v.is_string())
    {
        return Err("provider must be a string".into());
    }
    let provider_filter = provider_filter.filter(|provider| valid_provider_id(provider));
    if args.get("provider").is_some_and(|v| v.is_string()) && provider_filter.is_none() {
        return Err("provider must be a lowercase provider slug (letters, digits, _ or -)".into());
    }
    let classification_filter = args.get("access_classification").and_then(Value::as_str);
    if args.get("access_classification").is_some_and(|v| {
        !v.is_null()
            && ![
                "model_price_zero",
                "free_tier_eligible_unverified",
                "open_weights_self_hosted",
                "unknown",
            ]
            .contains(&v.as_str().unwrap_or(""))
    }) {
        return Err("invalid access_classification".into());
    }
    let sort = args
        .get("sort")
        .and_then(Value::as_str)
        .unwrap_or("intelligence");
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
        return Err("invalid sort".into());
    }
    let query = args.get("query").and_then(Value::as_str).unwrap_or("");
    if query.len() > 120 {
        return Err("query must be at most 120 bytes".into());
    }
    let query = query.to_lowercase();
    let tools_only = match args.get("tools_only") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::Bool(true)) => true,
        _ => return Err("tools_only must be a boolean".into()),
    };
    let min_context = args.get("min_context").and_then(Value::as_u64).unwrap_or(0);
    let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0);
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(20);
    if offset > 100_000 || limit == 0 || limit > 100 {
        return Err("offset must be <=100000 and limit must be 1..100".into());
    }
    let offset = offset as usize;
    let limit = limit as usize;
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .user_agent("ats-mcp/0.1 (MCP discovery)")
        .build()
        .map_err(|_| "HTTP client setup failed".to_string())?;
    let (mut models, statuses) =
        fetch_providers(&client, providers(), provider_filter.as_deref(), sort);
    models.retain(|m| {
        (query.is_empty()
            || m["id"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .contains(&query)
            || m["name"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .contains(&query))
            && (!tools_only || m["supports_tools"] == true)
            && (min_context == 0
                || m["context_length"]
                    .as_u64()
                    .is_some_and(|n| n >= min_context))
            && classification_filter.is_none_or(|c| m["access_classification"] == c)
            && (provider_filter.is_some()
                || classification_filter.is_some()
                || m["access_classification"] != "unknown")
    });
    models.sort_by(|a, b| {
        let primary = match sort {
            "context" => {
                descending_optional(a["context_length"].as_u64(), b["context_length"].as_u64())
            }
            "intelligence" => score_order(a, b, "intelligence_index"),
            "coding" => score_order(a, b, "coding_index"),
            "agentic" => score_order(a, b, "agentic_index"),
            "newest" => descending_optional(a["created_unix"].as_i64(), b["created_unix"].as_i64()),
            "throughput" => a["throughput_order"]
                .as_u64()
                .cmp(&b["throughput_order"].as_u64()),
            _ => a["name"]
                .as_str()
                .map(str::to_lowercase)
                .cmp(&b["name"].as_str().map(str::to_lowercase)),
        };
        primary.then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
    });
    let total = models.len();
    let page: Vec<Value> = models.into_iter().skip(offset).take(limit).collect();
    Ok(
        json!({"models":page,"total":total,"offset":offset,"limit":limit,"has_more":offset.saturating_add(limit)<total,"sort":sort,
        "providers":statuses,"partial_results":statuses_success_count(&statuses)>0 && statuses.iter().any(|s| s["status"]!="ok"),
        "listing_contract":"Built-in catalogs are fetched without provider inference credentials; listing does not grant inference access, which can still require signup, credentials, and quota.",
        "fetched_at_unix":fetched_at(),"free_definition":"model_price_zero means an official public source explicitly reports zero input/output prices; free_tier_eligible_unverified means a provider offers quota-limited free access but per-model eligibility is unproven; open_weights_self_hosted means a public non-gated Hub repository is listed for self-hosting subject to its license, not free hosted inference; unknown is not asserted free.",
        "benchmark_contract":"Cross-provider benchmark values are unknown; null means not supplied by a comparable source. Throughput values are not inferred."}),
    )
}
fn descending_optional<T: Ord>(a: Option<T>, b: Option<T>) -> std::cmp::Ordering {
    match (a, b) {
        (Some(x), Some(y)) => y.cmp(&x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}
fn score_order(a: &Value, b: &Value, field: &str) -> std::cmp::Ordering {
    match (a[field].as_f64(), b[field].as_f64()) {
        (Some(x), Some(y)) => y.total_cmp(&x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}
fn statuses_success_count(statuses: &[Value]) -> usize {
    statuses.iter().filter(|s| s["status"] == "ok").count()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_catalog_fixtures_preserve_free_evidence_distinctions() {
        let groq_rows = parse_groq_markdown(include_str!("../tests/fixtures/groq_models.md"))
            .expect("Groq fixture");
        assert_eq!(groq_rows.len(), 2);
        assert_eq!(groq_rows[0]["rate_limit_requests_per_minute"], 30);
        assert_eq!(
            groq_rows[0]["_access_classification"],
            "free_tier_eligible_unverified"
        );
        assert_eq!(
            groq_rows[1]["_access_classification"],
            "free_tier_eligible_unverified"
        );

        let cloudflare_rows =
            parse_cloudflare_pricing_html(include_str!("../tests/fixtures/cloudflare_models.md"))
                .expect("Cloudflare fixture");
        assert_eq!(cloudflare_rows.len(), 2);
        assert_eq!(
            cloudflare_rows[0]["id"],
            "@cf/meta/llama-3.3-70b-instruct-fp8-fast"
        );
        assert_eq!(
            cloudflare_rows[0]["_access_classification"],
            "free_tier_eligible_unverified"
        );
        assert_eq!(cloudflare_rows[1]["_access_classification"], "unknown");

        let gemini_rows =
            parse_gemini_pricing_html(include_str!("../tests/fixtures/gemini_pricing.html"))
                .expect("Gemini fixture");
        assert_eq!(gemini_rows.len(), 1);
        assert_eq!(gemini_rows[0]["id"], "gemini-fixture-free");
        assert_eq!(gemini_rows[0]["_access_classification"], "model_price_zero");

        let nous_value: Value =
            serde_json::from_str(include_str!("../tests/fixtures/nous_models.json")).unwrap();
        let (nous_rows, _, _) = extract_rows(Kind::Nous, &nous_value).unwrap();
        let nous = Provider {
            id: "nous".into(),
            endpoint: "https://example.test/models".into(),
            documentation: NOUS_DOCS,
            auth: Auth::None,
            kind: Kind::Nous,
        };
        assert_eq!(
            normalized(&nous, &nous_rows[0]).unwrap()["access_classification"],
            "model_price_zero"
        );
        assert!(normalized(&nous, &nous_rows[1]).is_none());
    }

    #[test]
    fn provider_rows_keep_public_page_metadata() {
        let p = Provider {
            id: "gemini".into(),
            endpoint: "https://example.test/pricing".into(),
            documentation: GEMINI_DOCS,
            auth: Auth::None,
            kind: Kind::Gemini,
        };
        let row = normalized(
            &p,
            &json!({
                "id":"gemini-foo",
                "name":"Gemini Foo",
                "pricing":{"prompt":"0","completion":"0"},
                "model_url":"https://example.test/pricing#gemini-foo",
                "_access_classification":"model_price_zero",
                "_free_evidence":"fixture evidence"
            }),
        )
        .unwrap();
        assert_eq!(row["provider_model_id"], "gemini-foo");
        assert_eq!(row["access_classification"], "model_price_zero");
        assert_eq!(row["pricing_usd_per_token"]["prompt"], "0");
        assert_eq!(row["model_url"], "https://example.test/pricing#gemini-foo");
        assert_eq!(
            page_url("https://example.test/models", 1, Some("a/b +"), true).unwrap(),
            "https://example.test/models?pageSize=1000&pageToken=a%2Fb+%2B"
        );
    }
    #[test]
    fn malformed_provider_rows_and_public_pages_are_diagnosed() {
        assert!(extract_rows(Kind::Groq, &json!({"data":{}})).is_err());
        assert!(!safe_model_id("../bad model"));
        assert!(parse_groq_markdown("not a catalog").is_err());
        assert!(parse_cloudflare_pricing_html("<article></article>").is_err());
        assert!(parse_gemini_pricing_html("<article></article>").is_err());
    }

    #[test]
    fn provider_free_labels_follow_evidence_and_model_schemas() {
        let or = Provider {
            id: "openrouter".into(),
            endpoint: "https://example.test/models".into(),
            documentation: OR_DOCS,
            auth: Auth::None,
            kind: Kind::OpenRouter,
        };
        let free = json!({"id":"lab/model:free","name":"Model","pricing":{"prompt":"0","completion":"0"},"supported_parameters":["tools"]});
        assert_eq!(
            normalized(&or, &free).unwrap()["access_classification"],
            "model_price_zero"
        );
        let priced = json!({"id":"lab/model:free","pricing":{"prompt":"0","completion":"0.00001"}});
        assert!(normalized(&or, &priced).is_none());
        let nous = Provider {
            id: "nous".into(),
            endpoint: "https://example.test/models".into(),
            documentation: NOUS_DOCS,
            auth: Auth::None,
            kind: Kind::Nous,
        };
        let row = normalized(&nous, &json!({"id":"llama-x:free","context_window":8192,"pricing":{"prompt":"0","completion":"0"}})).unwrap();
        assert_eq!(row["access_classification"], "model_price_zero");
        assert!(row["intelligence_index"].is_null());
        assert_eq!(
            page_url("https://example.test/models", 3, None, false).unwrap(),
            "https://example.test/models?page=3&per_page=100"
        );
        assert_eq!(
            provider_model_url(&or, "thinkingmachines/inkling:free").unwrap(),
            "https://openrouter.ai/models/thinkingmachines/inkling:free"
        );
    }

    #[test]
    fn one_provider_failure_preserves_another_providers_mock_results() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            thread,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let good_endpoint = format!("http://{}/v1/models", listener.local_addr().unwrap());
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request).unwrap();
            let body = r#"{"data":[{"id":"available-model"}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let provider = |id: &str, endpoint: String| Provider {
            id: id.into(),
            endpoint,
            documentation: OPENAI_COMPATIBLE_DOCS,
            auth: Auth::None,
            kind: Kind::OpenAiCompatible,
        };
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(1))
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let (models, statuses) = fetch_providers(
            &client,
            vec![
                (provider("available", good_endpoint), None),
                (
                    provider("unavailable", "http://127.0.0.1:1/v1/models".into()),
                    None,
                ),
            ],
            None,
            "name",
        );
        worker.join().unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["provider_model_id"], "available-model");
        assert_eq!(statuses[0]["status"], "ok");
        assert_eq!(statuses[1]["status"], "error");
        assert!(
            statuses.iter().any(|status| status["status"] == "ok")
                && statuses.iter().any(|status| status["status"] != "ok")
        );
    }

    #[test]
    fn huggingface_public_catalog_is_keyless_and_not_hosted_free_claim() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            thread,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            let size = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..size]).to_lowercase();
            let body = r#"[{"id":"org/open-model","private":false,"gated":false,"pipeline_tag":"text-generation"},{"id":"org/gated","private":false,"gated":true},{"id":"org/private","private":true}]"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            request
        });
        let provider = Provider {
            id: "huggingface".into(),
            endpoint: format!(
                "http://{}/api/models?pipeline_tag=text-generation&limit=100",
                addr
            ),
            documentation: HF_DOCS,
            auth: Auth::None,
            kind: Kind::HuggingFace,
        };
        let client = Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let models = fetch_provider(&client, &provider, "name").unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(
            models[0]["access_classification"],
            "open_weights_self_hosted"
        );
        assert_eq!(
            models[0]["model_url"],
            "https://huggingface.co/org/open-model"
        );
        let request = worker.join().unwrap();
        assert!(
            request.starts_with("get /api/models?pipeline_tag=text-generation&limit=100 http/1.1")
        );
        assert!(!request.contains("authorization:"));
    }

    #[test]
    fn openai_compatible_catalog_is_keyless_and_unknown_by_default() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            thread,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            let size = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..size]).to_lowercase();
            let body = r#"{"data":[{"id":"public-model","context_length":8192}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            request
        });
        let provider = Provider {
            id: "openai_compatible".into(),
            endpoint: format!("{base}/models"),
            documentation: OPENAI_COMPATIBLE_DOCS,
            auth: Auth::None,
            kind: Kind::OpenAiCompatible,
        };
        assert!(valid_catalog_base(&base));
        let client = Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let models = fetch_provider(&client, &provider, "name").unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["provider_model_id"], "public-model");
        assert_eq!(models[0]["access_classification"], "unknown");
        let request = worker.join().unwrap();
        assert!(request.starts_with("get /v1/models http/1.1"));
        assert!(!request.contains("authorization:"));
        assert!(!request.contains("api-key"));
    }

    #[test]
    fn built_in_public_sources_send_no_authorization_headers() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            thread::{self, JoinHandle},
        };

        fn serve(body: &'static str) -> (String, JoinHandle<String>) {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let worker = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 4096];
                let size = stream.read(&mut request).unwrap();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
                String::from_utf8_lossy(&request[..size]).to_lowercase()
            });
            (endpoint, worker)
        }

        let cases = [
            (
                "groq",
                Kind::Groq,
                GROQ_DOCS,
                include_str!("../tests/fixtures/groq_models.md"),
            ),
            (
                "cloudflare",
                Kind::Cloudflare,
                CF_DOCS,
                include_str!("../tests/fixtures/cloudflare_models.md"),
            ),
            (
                "gemini",
                Kind::Gemini,
                GEMINI_DOCS,
                include_str!("../tests/fixtures/gemini_pricing.html"),
            ),
            (
                "nous",
                Kind::Nous,
                NOUS_DOCS,
                include_str!("../tests/fixtures/nous_models.json"),
            ),
        ];
        let client = Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        for (id, kind, documentation, body) in cases {
            let (endpoint, worker) = serve(body);
            let provider = Provider {
                id: id.into(),
                endpoint,
                documentation,
                auth: Auth::None,
                kind,
            };
            assert!(
                !fetch_provider(&client, &provider, "name")
                    .unwrap()
                    .is_empty()
            );
            let request = worker.join().unwrap();
            assert!(!request.contains("authorization:"), "{id}: {request}");
            assert!(!request.contains("x-goog-api-key:"), "{id}: {request}");
        }
    }

    #[test]
    #[ignore = "live network smoke for public keyless catalogs"]
    fn live_keyless_public_catalog_smoke() {
        for provider in [
            "openrouter",
            "groq",
            "cloudflare",
            "gemini",
            "nous",
            "huggingface",
        ] {
            let result = list_free_llms(&json!({
                "provider": provider,
                "sort": "name",
                "limit": 1
            }))
            .unwrap();
            assert_eq!(
                result["providers"][0]["status"], "ok",
                "{provider}: {}",
                result["providers"][0]
            );
            assert_eq!(result["providers"][0]["listing_authentication"], "none");
            assert!(result["total"].as_u64().is_some_and(|total| total > 0));
        }
    }

    #[test]
    fn bearer_auth_is_sent_as_header_to_mock_provider() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            thread,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0; 1024];
            let size = stream.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..size]).to_string();
            let body = r#"{"data":[{"id":"model-a"}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            request
        });
        let provider = Provider {
            id: "openai_compatible".into(),
            endpoint: endpoint.clone(),
            documentation: OPENAI_COMPATIBLE_DOCS,
            auth: Auth::Bearer("mock-secret".into()),
            kind: Kind::OpenAiCompatible,
        };
        let client = Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        assert_eq!(
            get(&client, &provider, &endpoint).unwrap()["data"][0]["id"],
            "model-a"
        );
        let request = worker.join().unwrap().to_lowercase();
        assert!(request.contains("authorization: bearer mock-secret"));
        assert!(!endpoint.contains("mock-secret"));
    }
}
