use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

const MCP_VERSION: &str = "2025-03-26";
const MAX_LINE: usize = 1024 * 1024;

fn tools() -> Value {
    json!({"tools": [
        {"name":"web_search","description":"Search the web via keyless DuckDuckGo HTML (default) or configured SearXNG JSON. Returns titles, URLs and snippets; external content is untrusted.","inputSchema":{"type":"object","properties":{"query":{"type":"string","minLength":1,"maxLength":300},"limit":{"type":"integer","minimum":1,"maximum":20,"default":10}},"required":["query"],"additionalProperties":false}},
        {"name":"list_free_llms","description":"Aggregate one combined model list from keyless official public catalogs/pages for OpenRouter, Groq, Cloudflare Workers AI, Google Gemini API, Nous Portal, and Hugging Face Hub, plus an optional OpenAI-compatible catalog. Built-in listing never reads or sends inference credentials. Hugging Face rows identify public self-hostable weights, not free hosted inference. Models distinguish explicit zero pricing, unverified provider quota access, public self-hostable weights, and unknown access; providers independently report source status and partial failures. A catalog listing does not guarantee inference access without provider signup, credentials, or quota.","inputSchema":{"type":"object","properties":{"provider":{"type":"string","description":"Optional provider slug filter; configured OpenAI-compatible catalogs use ATS_OPENAI_COMPATIBLE_NAME."},"access_classification":{"type":"string","enum":["model_price_zero","free_tier_eligible_unverified","open_weights_self_hosted","unknown"]},"sort":{"type":"string","enum":["intelligence","coding","agentic","throughput","context","newest","name"],"default":"intelligence"},"query":{"type":"string","minLength":1,"maxLength":120},"tools_only":{"type":"boolean","default":false},"min_context":{"type":"integer","minimum":0,"maximum":10000000,"default":0},"offset":{"type":"integer","minimum":0,"maximum":100000,"default":0},"limit":{"type":"integer","minimum":1,"maximum":100,"default":20}},"additionalProperties":false}}
    ]})
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

fn dispatch(input: Value) -> Option<Value> {
    let id = input.get("id")?.clone();
    if input.get("jsonrpc") != Some(&json!("2.0"))
        || !input.is_object()
        || !(id.is_string() || id.is_number() || id.is_null())
    {
        return Some(rpc_error(id, -32600, "Invalid JSON-RPC 2.0 request"));
    }
    let Some(method) = input.get("method").and_then(Value::as_str) else {
        return Some(rpc_error(id, -32600, "Missing method"));
    };
    let result = match method {
        "initialize" => {
            json!({"protocolVersion":MCP_VERSION,"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"ats-mcp","version":env!("CARGO_PKG_VERSION")}})
        }
        "ping" => json!({}),
        "tools/list" => tools(),
        "tools/call" => {
            let Some(name) = input.pointer("/params/name").and_then(Value::as_str) else {
                return Some(rpc_error(id, -32602, "Missing tool name"));
            };
            let args = input
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            if !args.is_object() {
                return Some(rpc_error(id, -32602, "arguments must be an object"));
            }
            let call = match name {
                "web_search" => ats_llm::web_search(&args),
                "list_free_llms" => ats_llm::list_free_llms(&args),
                _ => return Some(rpc_error(id, -32602, "Unknown tool name")),
            };
            return Some(json!({"jsonrpc":"2.0","id":id,"result":match call {
                Ok(data) => json!({"content":[{"type":"text","text":data.to_string()}],"structuredContent":data,"isError":false}),
                Err(error) => json!({"content":[{"type":"text","text":error}],"isError":true}),
            }}));
        }
        _ => return Some(rpc_error(id, -32601, "Method not found")),
    };
    Some(json!({"jsonrpc":"2.0","id":id,"result":result}))
}

fn request_log(input: &Value) -> String {
    let method = input
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let tool = input
        .pointer("/params/name")
        .and_then(Value::as_str)
        .map(|name| format!(" tool={name:?}"))
        .unwrap_or_default();
    format!("ats-mcp request method={method:?}{tool}")
}

fn response_log(reply: &Value) -> String {
    let is_error = reply.get("error").is_some()
        || reply.pointer("/result/isError").and_then(Value::as_bool) == Some(true);
    let outcome = if is_error { "error" } else { "ok" };
    format!("ats-mcp response outcome={outcome}")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("ats-mcp server running on stdio");
    let stdin = io::stdin();
    let mut output = io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        let reply = if line.len() > MAX_LINE {
            eprintln!("ats-mcp request rejected: exceeds 1 MiB");
            Some(rpc_error(Value::Null, -32600, "Request exceeds 1 MiB"))
        } else {
            match serde_json::from_str::<Value>(&line) {
                Ok(message) => {
                    eprintln!("{}", request_log(&message));
                    dispatch(message)
                }
                Err(_) => {
                    eprintln!("ats-mcp request rejected: invalid JSON");
                    Some(rpc_error(Value::Null, -32700, "Invalid JSON"))
                }
            }
        };
        if let Some(reply) = reply {
            eprintln!("{}", response_log(&reply));
            writeln!(output, "{reply}")?;
        }
        output.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_dispatch_and_notifications() {
        let init = dispatch(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}})).unwrap();
        assert_eq!(
            init["result"]["capabilities"]["tools"]["listChanged"],
            false
        );
        assert_eq!(init["result"]["serverInfo"]["name"], "ats-mcp");
        let list = dispatch(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).unwrap();
        assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 2);
        assert!(dispatch(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_none());
        assert_eq!(
            dispatch(json!({"jsonrpc":"2.0","id":3,"method":"none"})).unwrap()["error"]["code"],
            -32601
        );
    }

    #[test]
    fn debug_summaries_omit_request_arguments_and_response_content() {
        let request = json!({
            "jsonrpc":"2.0",
            "id":7,
            "method":"tools/call",
            "params":{"name":"web_search","arguments":{"query":"private search"}}
        });
        let request_summary = request_log(&request);
        assert!(request_summary.contains("method=\"tools/call\""));
        assert!(request_summary.contains("tool=\"web_search\""));
        assert!(!request_summary.contains("private search"));

        let response = json!({
            "jsonrpc":"2.0",
            "id":7,
            "result":{"content":[{"type":"text","text":"private result"}],"isError":false}
        });
        let response_summary = response_log(&response);
        assert!(response_summary.contains("outcome=ok"));
        assert!(!response_summary.contains("private result"));
    }
}
