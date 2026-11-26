//! End-to-end gateway tests: the real `orchnes` binary and HTTP client (ureq) against a
//! tiny local HTTP server that plays the providers. No external network is used.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Command, Output};
use std::sync::mpsc;
use std::thread;

/// What the fake provider received.
struct Seen {
    request_line: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Serves `replies` (status, body) to consecutive connections; returns base URL + receiver.
fn fake_provider(replies: Vec<(u16, &'static str)>) -> (String, mpsc::Receiver<Seen>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for (status, body) in replies {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut headers = Vec::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                let (k, v) = line.split_once(':').unwrap();
                headers.push((k.trim().to_string(), v.trim().to_string()));
            }
            let len: usize = headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                .map_or(0, |(_, v)| v.parse().unwrap());
            let mut buf = vec![0; len];
            reader.read_exact(&mut buf).unwrap();
            let mut stream = stream;
            write!(
                stream,
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            tx.send(Seen {
                request_line: request_line.trim_end().to_string(),
                headers,
                body: String::from_utf8(buf).unwrap(),
            })
            .unwrap();
        }
    });
    (base, rx)
}

fn orchnes(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_orchnes"));
    cmd.args(args).env_remove("RUST_BACKTRACE");
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().expect("run orchnes")
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

const ANTHROPIC_OVERLOADED: &str =
    r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
const OPENAI_OK: &str = r#"{"choices":[{"message":{"content":"Merhaba!"},"finish_reason":"stop"}],"usage":{"prompt_tokens":40,"completion_tokens":2}}"#;

#[test]
fn ask_falls_back_over_real_http_and_logs() {
    let (base, rx) = fake_provider(vec![(529, ANTHROPIC_OVERLOADED), (200, OPENAI_OK)]);
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("orchnes.yaml");
    fs::write(
        &cfg,
        format!(
            "timeout_secs: 10\nmodels:\n\
             \x20 - {{id: claude, provider: anthropic, base_url: '{base}', model: claude-sonnet-5-5, tier: T2, api_key_env: FAKE_ANTHROPIC_KEY, server_fallback: true}}\n\
             \x20 - {{id: local, provider: openai, base_url: '{base}/v1', model: qwen3:8b, tier: T2, local: true, price_in: 1.0, price_out: 2.0}}\n"
        ),
    )
    .unwrap();
    let log = dir.path().join("run.log");
    let o = orchnes(
        &[
            "ask",
            "--config",
            cfg.to_str().unwrap(),
            "--tier",
            "T2",
            "--max-tokens",
            "64",
            "--log",
            log.to_str().unwrap(),
            "Selam",
        ],
        &[("FAKE_ANTHROPIC_KEY", "sk-test-123")],
    );
    assert!(o.status.success(), "stderr: {}", text(&o.stderr));
    assert_eq!(text(&o.stdout), "Merhaba!\n");
    let err = text(&o.stderr);
    assert!(err.contains("tried claude: HTTP 529: Overloaded"), "{err}");
    assert!(
        err.contains("model=local tier=T2 tokens in=40 out=2 $0.000044"),
        "{err}"
    );

    let first = rx.recv().unwrap();
    assert_eq!(first.request_line, "POST /v1/messages HTTP/1.1");
    assert_eq!(first.header("x-api-key"), Some("sk-test-123"));
    assert_eq!(first.header("anthropic-version"), Some("2026-10-21"));
    assert_eq!(
        first.header("anthropic-beta"),
        Some("server-side-fallback-2026-07-01")
    );
    let body: serde_json::Value = serde_json::from_str(&first.body).unwrap();
    assert_eq!(body["model"], "claude-sonnet-5-5");
    assert_eq!(body["max_tokens"], 64);
    assert_eq!(body["fallbacks"], "default");

    let second = rx.recv().unwrap();
    assert_eq!(second.request_line, "POST /v1/chat/completions HTTP/1.1");
    assert_eq!(second.header("authorization"), None, "keyless local model");
    let body: serde_json::Value = serde_json::from_str(&second.body).unwrap();
    assert_eq!(
        body["messages"][0],
        serde_json::json!({"role": "user", "content": "Selam"})
    );

    let line = fs::read_to_string(&log).unwrap();
    assert!(line.contains("| ASK | ask | model=local T2 | - | 1 failed before success | 42 tok | $0.000044 | why tier T2 requested"), "{line}");
}

#[test]
fn sensitive_requests_never_reach_hosted_models() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("g.yaml");
    fs::write(
        &cfg,
        "models:\n  - {id: hosted, provider: anthropic, base_url: 'http://127.0.0.1:9', model: m, tier: T1, api_key_env: K}\n",
    )
    .unwrap();
    let o = orchnes(
        &[
            "ask",
            "--config",
            cfg.to_str().unwrap(),
            "--tier",
            "T1",
            "--sensitive",
            "x",
        ],
        &[("K", "secret")],
    );
    assert!(!o.status.success());
    assert!(text(&o.stderr).contains("no configured model for tier T1 that is local or compliant"));
}

#[test]
fn dry_run_and_models_never_print_keys() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("g.yaml");
    fs::write(
        &cfg,
        "models:\n  - {id: a, provider: anthropic, base_url: 'https://api.anthropic.com', model: claude-haiku-5-5, tier: T3, api_key_env: SECRET_KEY_VAR}\n  - {id: b, provider: openai, base_url: 'https://example.test/v1', model: m, tier: T2, api_key_env: OTHER_KEY_VAR}\n",
    )
    .unwrap();
    let envs = [("SECRET_KEY_VAR", "sk-should-not-appear")];
    let o = orchnes(
        &[
            "ask",
            "--config",
            cfg.to_str().unwrap(),
            "--complexity",
            "1",
            "--risk",
            "1",
            "--context",
            "1",
            "--capability",
            "1",
            "--dry-run",
            "hi",
        ],
        &envs,
    );
    let out = text(&o.stdout);
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert!(out.contains("model: a (tier T3"), "{out}");
    assert!(out.contains("order: a → b"), "{out}");
    assert!(out.contains("x-api-key: ***"), "{out}");
    assert!(!out.contains("sk-should-not-appear"));

    let o = orchnes(&["models", "--config", cfg.to_str().unwrap()], &envs);
    let out = text(&o.stdout);
    assert!(out.contains("env SECRET_KEY_VAR: set"), "{out}");
    assert!(out.contains("env OTHER_KEY_VAR: MISSING"), "{out}");
    assert!(!out.contains("sk-should-not-appear"));
}

#[test]
fn ask_model_is_strict_and_supports_dry_run_and_log() {
    let (base, rx) = fake_provider(vec![(200, OPENAI_OK), (529, ANTHROPIC_OVERLOADED)]);
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("g.yaml");
    fs::write(
        &cfg,
        format!(
            "models:\n\
             \x20 - {{id: first, provider: openai, base_url: '{base}/v1', model: m1, tier: T3, price_in: 1.0, price_out: 2.0}}\n\
             \x20 - {{id: second, provider: anthropic, base_url: '{base}', model: m2, tier: T2, api_key_env: K}}\n"
        ),
    )
    .unwrap();
    let cfg_str = cfg.to_str().unwrap();

    // --dry-run shows only the requested model and makes no HTTP request.
    let o = orchnes(
        &[
            "ask",
            "--config",
            cfg_str,
            "--model",
            "second",
            "--dry-run",
            "hi",
        ],
        &[("K", "secret")],
    );
    let out = text(&o.stdout);
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert!(
        out.contains("model: second (tier T2, model second requested)"),
        "{out}"
    );
    assert!(out.contains("order: second\n"), "{out}");
    assert!(!out.contains("first"));
    assert!(!out.contains("secret"));

    // Successful --model call writes --log with existing format.
    let log = dir.path().join("run.log");
    let o = orchnes(
        &[
            "ask",
            "--config",
            cfg_str,
            "--model",
            "first",
            "--log",
            log.to_str().unwrap(),
            "hi",
        ],
        &[],
    );
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert_eq!(text(&o.stdout), "Merhaba!\n");
    let seen = rx.recv().unwrap();
    assert_eq!(seen.request_line, "POST /v1/chat/completions HTTP/1.1");
    let line = fs::read_to_string(&log).unwrap();
    assert!(
        line.contains("| ASK | ask | model=first T3 | - | 0 failed before success | 42 tok | $0.000044 | why model first requested"),
        "{line}"
    );

    // Failed --model call does not fall back to `first`.
    let o = orchnes(
        &["ask", "--config", cfg_str, "--model", "second", "hi"],
        &[("K", "secret")],
    );
    assert!(!o.status.success());
    let err = text(&o.stderr);
    assert!(err.contains("second: HTTP 529: Overloaded"), "{err}");
    assert!(!err.contains("first"), "{err}");
    let _ = rx.recv().unwrap();
    assert!(rx.try_recv().is_err(), "no fallback request should be sent");
}

#[test]
fn ask_model_rejects_unknown_id_and_routing_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("g.yaml");
    fs::write(
        &cfg,
        "models:\n  - {id: local, provider: openai, base_url: 'http://127.0.0.1:9/v1', model: m, tier: T3, local: true}\n",
    )
    .unwrap();
    let cfg_str = cfg.to_str().unwrap();

    let o = orchnes(&["ask", "--config", cfg_str, "--model", "nope", "hi"], &[]);
    assert!(!o.status.success());
    assert!(
        text(&o.stderr).contains("unknown model \"nope\""),
        "{}",
        text(&o.stderr)
    );

    for conflict in [
        &[
            "ask", "--config", cfg_str, "--model", "local", "--tier", "T3", "hi",
        ][..],
        &[
            "ask",
            "--config",
            cfg_str,
            "--model",
            "local",
            "--complexity",
            "1",
            "hi",
        ][..],
        &[
            "ask", "--config", cfg_str, "--model", "local", "--risk", "1", "hi",
        ][..],
    ] {
        let o = orchnes(conflict, &[]);
        assert!(!o.status.success());
        assert!(
            text(&o.stderr).contains("--model cannot be combined with --tier or routing flags"),
            "{}",
            text(&o.stderr)
        );
    }
}
