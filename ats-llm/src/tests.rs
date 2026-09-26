use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap()
}

fn model(id: &str, score: Option<f64>, context: u64, price: &str) -> Value {
    json!({"id":id,"name":id,"context_length":context,"created":100,
          "pricing":{"prompt":price,"completion":"0"},"supported_parameters":["tools"],
          "benchmarks":{"artificial_analysis":{"intelligence_index":score,"coding_index":null,"agentic_index":null}}})
}

fn serve_once(body: String, status: &str) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = format!("http://{}", listener.local_addr().unwrap());
    let status = status.to_owned();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut data = [0; 4096];
        let n = stream.read(&mut data).unwrap();
        let request = String::from_utf8_lossy(&data[..n]).to_string();
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        request
    });
    (addr, worker)
}

#[test]
fn only_explicit_zero_free_variants() {
    let free: Model = serde_json::from_value(model("a/b:free", None, 100, "0")).unwrap();
    assert!(is_free(&free));
    for (id, price) in [("a/b", "0"), ("a/b:free", "0.01"), ("a/b:free", "missing")] {
        let m: Model = serde_json::from_value(model(id, None, 100, price)).unwrap();
        assert!(!is_free(&m));
    }
    assert!(zero_price("0.000"));
    assert!(!zero_price("1e-999"));
    let mut conditional = model("a/b:free", None, 100, "0");
    conditional["pricing"]["overrides"] = json!([{"completion":"0.001"}]);
    assert!(!is_free(&serde_json::from_value(conditional).unwrap()));
}

#[test]
fn score_sort_null_last_and_stable_id_ties() {
    let mut models: Vec<Model> = [
        model("z/unknown:free", None, 100, "0"),
        model("z/tied:free", Some(20.0), 100, "0"),
        model("a/tied:free", Some(20.0), 100, "0"),
        model("a/best:free", Some(31.0), 100, "0"),
    ]
    .into_iter()
    .map(|v| serde_json::from_value(v).unwrap())
    .collect();
    sort_models(&mut models, "intelligence");
    assert_eq!(
        models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        [
            "a/best:free",
            "a/tied:free",
            "z/tied:free",
            "z/unknown:free"
        ]
    );
    sort_models(&mut models, "context");
    assert_eq!(models[0].id, "a/best:free");
}

#[test]
fn catalog_filters_then_paginates_and_preserves_throughput_order() {
    let source = json!({"total_count":5,"data":[
        model("a/paid", Some(99.0), 300, "0"),
        model("a/first:free", None, 200, "0"),
        model("a/notfree:free", None, 300, "0.01"),
        model("a/second:free", Some(12.0), 300, "0"),
        model("a/third:free", Some(30.0), 400, "0")
    ]})
    .to_string();
    let (url, handle) = serve_once(source, "200 OK");
    let config = ToolConfig {
        models_url: format!("{url}/models"),
        search_url: String::new(),
        search_backend: String::new(),
    };
    let output = model_catalog(
        &client(),
        &config,
        &json!({"sort":"throughput","limit":1,"offset":1}),
    )
    .unwrap();
    assert_eq!(output["total"], 3);
    assert_eq!(output["models"][0]["id"], "a/second:free");
    assert_eq!(output["models"][0]["throughput_order"], 2);
    assert!(output["models"][0]["throughput_tokens_per_second"].is_null());
    assert_eq!(output["has_more"], true);
    assert!(
        handle
            .join()
            .unwrap()
            .contains("sort=throughput-high-to-low")
    );
}

#[test]
fn ddg_html_decodes_redirect_and_skips_invalid_links() {
    let html = r#"<div class="result"><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.org%2Fdoc&amp;rut=x">A <b>title</b></a><span class="result__snippet">A <b>snippet</b></span></div><div class="result"><a class="result__a" href="javascript:alert(1)">Bad</a></div>"#;
    let found = parse_ddg(html, 3);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0]["url"], "https://example.org/doc");
    assert_eq!(found[0]["snippet"], "A snippet");
}

#[test]
fn searxng_mock_search_and_errors() {
    let (url, handle) = serve_once(
        json!({"results":[{"title":"Hello","url":"https://example.org/","content":"Excerpt"}]})
            .to_string(),
        "200 OK",
    );
    let config = ToolConfig {
        models_url: String::new(),
        search_url: url,
        search_backend: "searxng".into(),
    };
    let result = search(
        &client(),
        &config,
        &json!({"query":"hello world","limit":1}),
    )
    .unwrap();
    assert_eq!(result["results"][0]["title"], "Hello");
    let req = handle.join().unwrap();
    assert!(req.contains("q=hello+world") && req.contains("format=json"));
    assert!(search(&client(), &config, &json!({"query":""})).is_err());
    assert!(search(&client(), &config, &json!({"query":"a","limit":21})).is_err());
}

#[test]
fn incomplete_catalog_and_http_failures_are_actionable() {
    let (url, handle) = serve_once(
        json!({"total_count":2,"data":[model("a/b:free",None,20,"0")]}).to_string(),
        "200 OK",
    );
    let config = ToolConfig {
        models_url: url,
        search_url: String::new(),
        search_backend: String::new(),
    };
    assert!(
        model_catalog(&client(), &config, &json!({}))
            .unwrap_err()
            .contains("incomplete catalog")
    );
    handle.join().unwrap();
    let (url, handle) = serve_once("denied".into(), "429 Too Many Requests");
    let config = ToolConfig {
        models_url: url,
        search_url: String::new(),
        search_backend: String::new(),
    };
    assert!(
        model_catalog(&client(), &config, &json!({}))
            .unwrap_err()
            .contains("429")
    );
    handle.join().unwrap();
}
