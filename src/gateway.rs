//! Model gateway: one call interface over OpenAI-compatible Chat Completions and the
//! Anthropic Messages API, with tier-based model selection, fallback and cost accounting.
//!
//! "OpenAI-compatible" covers OpenAI, OpenRouter, LiteLLM, Ollama, vLLM, LM Studio and the
//! OpenAI-compatible endpoints of other providers, so most hosted and local models fit.
//! API keys are read from environment variables named in the config — never stored in it.

use crate::routing::Tier;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::time::Duration;

/// Anthropic API version header value.
pub const ANTHROPIC_VERSION: &str = "2026-10-21";
/// Beta that enables server-side fallback on refusals (Anthropic).
pub const ANTHROPIC_FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// OpenAI-compatible `POST {base_url}/chat/completions`.
    Openai,
    /// Anthropic `POST {base_url}/v1/messages`.
    Anthropic,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSpec {
    /// Name used in logs and `--model`.
    pub id: String,
    pub provider: Provider,
    pub base_url: String,
    /// Model name sent to the provider.
    pub model: String,
    pub tier: Tier,
    /// Name of the environment variable that holds the credential (YAML: `api_key_env`);
    /// omit for keyless local servers. Only the variable's name is stored, never its value.
    #[serde(default, rename = "api_key_env")]
    pub credential_env: Option<String>,
    /// Runs on infrastructure you control (allowed for sensitive work).
    #[serde(default)]
    pub local: bool,
    /// Hosted, but covered by an agreement that allows sensitive data.
    #[serde(default)]
    pub compliant: bool,
    /// USD per 1M input tokens.
    #[serde(default)]
    pub price_in: Option<f64>,
    /// USD per 1M output tokens.
    #[serde(default)]
    pub price_out: Option<f64>,
    /// OpenAI-compatible only: name of the output-limit field (default `max_tokens`).
    #[serde(default)]
    pub max_tokens_param: Option<String>,
    /// Anthropic only: send `fallbacks: "default"` with the server-side fallback beta.
    #[serde(default)]
    pub server_fallback: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub models: Vec<ModelSpec>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 {
    600
}

#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("invalid gateway config: {0}")]
    Config(String),
    #[error("no configured model for tier {tier}{}", if *.local_only { " that is local or compliant" } else { "" })]
    NoCandidate { tier: Tier, local_only: bool },
    #[error("unknown model {id:?}")]
    UnknownModel { id: String },
    #[error("model {id:?} is not local or compliant")]
    NotSensitiveSafe { id: String },
    #[error("all models failed:\n{}", attempts.iter().map(|a| format!("  {}: {}", a.model_id, a.error)).collect::<Vec<_>>().join("\n"))]
    AllFailed { attempts: Vec<Attempt> },
}

pub fn load_config(path: &Path) -> Result<Config, GatewayError> {
    let text = fs::read_to_string(path).map_err(|source| GatewayError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let cfg: Config =
        serde_norway::from_str(&text).map_err(|e| GatewayError::Config(e.to_string()))?;
    let mut ids = HashSet::new();
    for m in &cfg.models {
        if !ids.insert(m.id.as_str()) {
            return Err(GatewayError::Config(format!(
                "duplicate model id {:?}",
                m.id
            )));
        }
        if !(m.base_url.starts_with("http://") || m.base_url.starts_with("https://")) {
            return Err(GatewayError::Config(format!(
                "{}: base_url must start with http:// or https://",
                m.id
            )));
        }
    }
    if cfg.models.is_empty() {
        return Err(GatewayError::Config("no models configured".into()));
    }
    Ok(cfg)
}

/// What to ask a model.
#[derive(Debug, Clone)]
pub struct Request {
    pub system: Option<String>,
    pub prompt: String,
    pub max_tokens: u32,
}

/// A provider-specific HTTP request, before sending.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

impl HttpRequest {
    /// Same request with credential headers masked, for display and logs.
    pub fn redacted(&self) -> HttpRequest {
        let mut r = self.clone();
        for (k, v) in &mut r.headers {
            if k.eq_ignore_ascii_case("authorization") || k.eq_ignore_ascii_case("x-api-key") {
                *v = "***".into();
            }
        }
        r
    }
}

pub fn build(spec: &ModelSpec, req: &Request, api_key: Option<&str>) -> HttpRequest {
    let base = spec.base_url.trim_end_matches('/');
    let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
    match spec.provider {
        Provider::Openai => {
            if let Some(k) = api_key {
                headers.push(("authorization".into(), format!("Bearer {k}")));
            }
            let mut messages = Vec::new();
            if let Some(s) = &req.system {
                messages.push(json!({"role": "system", "content": s}));
            }
            messages.push(json!({"role": "user", "content": req.prompt}));
            let mut body = json!({"model": spec.model, "messages": messages});
            let param = spec.max_tokens_param.as_deref().unwrap_or("max_tokens");
            body[param] = json!(req.max_tokens);
            HttpRequest {
                url: format!("{base}/chat/completions"),
                headers,
                body,
            }
        }
        Provider::Anthropic => {
            if let Some(k) = api_key {
                headers.push(("x-api-key".into(), k.into()));
            }
            headers.push(("anthropic-version".into(), ANTHROPIC_VERSION.into()));
            let mut body = json!({
                "model": spec.model,
                "max_tokens": req.max_tokens,
                "messages": [{"role": "user", "content": req.prompt}],
            });
            if let Some(s) = &req.system {
                body["system"] = json!(s);
            }
            if spec.server_fallback {
                headers.push(("anthropic-beta".into(), ANTHROPIC_FALLBACK_BETA.into()));
                body["fallbacks"] = json!("default");
            }
            HttpRequest {
                url: format!("{base}/v1/messages"),
                headers,
                body,
            }
        }
    }
}

/// A successful model answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    pub model_id: String,
    pub tier: Tier,
    pub text: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// None when the model has no prices configured.
    pub cost_usd: Option<f64>,
}

/// Parses a provider response. `Err` means "this model didn't produce an answer" —
/// HTTP errors, refusals, filtered or empty output — and the gateway tries the next model.
pub fn parse(spec: &ModelSpec, status: u16, body: &str) -> Result<Completion, String> {
    if !(200..300).contains(&status) {
        return Err(format!("HTTP {status}: {}", error_message(body)));
    }
    let v: Value = serde_json::from_str(body).map_err(|e| format!("invalid JSON: {e}"))?;
    let (text, input, output) = match spec.provider {
        Provider::Openai => {
            let choice = &v["choices"][0];
            if choice["finish_reason"] == "content_filter" {
                return Err("output blocked by content filter".into());
            }
            (
                choice["message"]["content"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
                v["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
                v["usage"]["completion_tokens"].as_u64().unwrap_or(0),
            )
        }
        Provider::Anthropic => {
            if v["stop_reason"] == "refusal" {
                let category = v["stop_details"]["category"]
                    .as_str()
                    .unwrap_or("unspecified");
                return Err(format!("refused (category: {category})"));
            }
            // Thinking blocks may precede the answer; keep only text blocks.
            let text = v["content"]
                .as_array()
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter(|b| b["type"] == "text")
                        .filter_map(|b| b["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();
            (
                text,
                v["usage"]["input_tokens"].as_u64().unwrap_or(0),
                v["usage"]["output_tokens"].as_u64().unwrap_or(0),
            )
        }
    };
    if text.is_empty() {
        return Err("empty response".into());
    }
    Ok(Completion {
        model_id: spec.id.clone(),
        tier: spec.tier,
        text,
        input_tokens: input,
        output_tokens: output,
        cost_usd: cost(spec, input, output),
    })
}

fn error_message(body: &str) -> String {
    let msg = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| body.chars().take(200).collect());
    msg.replace('\n', " ")
}

pub fn cost(spec: &ModelSpec, input: u64, output: u64) -> Option<f64> {
    match (spec.price_in, spec.price_out) {
        (Some(pi), Some(po)) => Some((input as f64 * pi + output as f64 * po) / 1_000_000.0),
        _ => None,
    }
}

/// Models to try, in order: the requested tier first, then higher tiers
/// (never lower — a weaker model should not silently take a harder task).
/// `local_only` keeps only local or compliant models.
pub fn candidates(cfg: &Config, tier: Tier, local_only: bool) -> Vec<&ModelSpec> {
    let order: &[Tier] = match tier {
        Tier::T3 => &[Tier::T3, Tier::T2, Tier::T1],
        Tier::T2 => &[Tier::T2, Tier::T1],
        Tier::T1 => &[Tier::T1],
    };
    order
        .iter()
        .flat_map(|t| cfg.models.iter().filter(move |m| m.tier == *t))
        .filter(|m| !local_only || m.local || m.compliant)
        .collect()
}

/// Finds a configured model by id, enforcing `local_only` when set.
pub fn select_model<'a>(
    cfg: &'a Config,
    id: &str,
    local_only: bool,
) -> Result<&'a ModelSpec, GatewayError> {
    let spec = cfg
        .models
        .iter()
        .find(|m| m.id == id)
        .ok_or_else(|| GatewayError::UnknownModel { id: id.to_string() })?;
    if local_only && !spec.local && !spec.compliant {
        return Err(GatewayError::NotSensitiveSafe { id: id.to_string() });
    }
    Ok(spec)
}

/// One model that was tried and didn't answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Attempt {
    pub model_id: String,
    pub error: String,
}

/// Sends HTTP requests; a trait so tests can run without a network.
pub trait Transport {
    /// Returns (status, body) for any HTTP response; `Err` only for transport failures.
    fn post(&self, req: &HttpRequest) -> Result<(u16, String), String>;
}

pub struct HttpTransport {
    agent: ureq::Agent,
}

impl HttpTransport {
    pub fn new(timeout: Duration) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .build()
            .into();
        HttpTransport { agent }
    }
}

impl Transport for HttpTransport {
    fn post(&self, req: &HttpRequest) -> Result<(u16, String), String> {
        let mut r = self.agent.post(&req.url);
        for (k, v) in &req.headers {
            r = r.header(k, v);
        }
        let mut resp = r.send(req.body.to_string()).map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let body = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| e.to_string())?;
        Ok((status, body))
    }
}

/// Tries candidate models in order until one answers. `env` looks up API keys.
pub fn complete(
    cfg: &Config,
    transport: &dyn Transport,
    req: &Request,
    tier: Tier,
    local_only: bool,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<(Completion, Vec<Attempt>), GatewayError> {
    let models = candidates(cfg, tier, local_only);
    if models.is_empty() {
        return Err(GatewayError::NoCandidate { tier, local_only });
    }
    complete_models(&models, transport, req, env)
}

/// Calls a single configured model by id with no fallback.
pub fn complete_model(
    cfg: &Config,
    transport: &dyn Transport,
    req: &Request,
    id: &str,
    local_only: bool,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<(Completion, Vec<Attempt>), GatewayError> {
    let spec = select_model(cfg, id, local_only)?;
    complete_models(&[spec], transport, req, env)
}

fn complete_models(
    models: &[&ModelSpec],
    transport: &dyn Transport,
    req: &Request,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<(Completion, Vec<Attempt>), GatewayError> {
    let mut attempts = Vec::new();
    for spec in models {
        let key = match &spec.credential_env {
            Some(var) => match env(var).filter(|k| !k.is_empty()) {
                Some(k) => Some(k),
                None => {
                    attempts.push(Attempt {
                        model_id: spec.id.clone(),
                        error: format!("skipped: environment variable {var} is not set"),
                    });
                    continue;
                }
            },
            None => None,
        };
        let http = build(spec, req, key.as_deref());
        let result = transport
            .post(&http)
            .and_then(|(status, body)| parse(spec, status, &body));
        match result {
            Ok(c) => return Ok((c, attempts)),
            Err(error) => attempts.push(Attempt {
                model_id: spec.id.clone(),
                error,
            }),
        }
    }
    Err(GatewayError::AllFailed { attempts })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn spec(id: &str, provider: Provider, tier: Tier) -> ModelSpec {
        ModelSpec {
            id: id.into(),
            provider,
            base_url: "https://example.test/v1/".into(),
            model: format!("{id}-model"),
            tier,
            credential_env: None,
            local: false,
            compliant: false,
            price_in: None,
            price_out: None,
            max_tokens_param: None,
            server_fallback: false,
        }
    }

    fn req() -> Request {
        Request {
            system: Some("Be brief.".into()),
            prompt: "Hi".into(),
            max_tokens: 100,
        }
    }

    struct Mock {
        replies: RefCell<Vec<Result<(u16, String), String>>>,
        seen: RefCell<Vec<HttpRequest>>,
    }

    impl Mock {
        fn new(replies: Vec<Result<(u16, &str), &str>>) -> Self {
            Mock {
                replies: RefCell::new(
                    replies
                        .into_iter()
                        .rev()
                        .map(|r| r.map(|(s, b)| (s, b.to_string())).map_err(str::to_string))
                        .collect(),
                ),
                seen: RefCell::new(Vec::new()),
            }
        }
    }

    impl Transport for Mock {
        fn post(&self, req: &HttpRequest) -> Result<(u16, String), String> {
            self.seen.borrow_mut().push(req.clone());
            self.replies.borrow_mut().pop().expect("unexpected request")
        }
    }

    const OPENAI_OK: &str = r#"{"choices":[{"message":{"content":"Hello!"},"finish_reason":"stop"}],"usage":{"prompt_tokens":12,"completion_tokens":3}}"#;
    const ANTHROPIC_OK: &str = r#"{"content":[{"type":"thinking","thinking":""},{"type":"text","text":"Hel"},{"type":"text","text":"lo!"}],"stop_reason":"end_turn","usage":{"input_tokens":20,"output_tokens":5}}"#;

    #[test]
    fn builds_openai_request() {
        let h = build(&spec("o", Provider::Openai, Tier::T2), &req(), Some("sk-1"));
        assert_eq!(h.url, "https://example.test/v1/chat/completions");
        assert!(
            h.headers
                .contains(&("authorization".into(), "Bearer sk-1".into()))
        );
        assert_eq!(
            h.body,
            json!({"model": "o-model", "max_tokens": 100, "messages": [
                {"role": "system", "content": "Be brief."},
                {"role": "user", "content": "Hi"}]})
        );
        let mut s = spec("o", Provider::Openai, Tier::T2);
        s.max_tokens_param = Some("max_completion_tokens".into());
        let h = build(&s, &req(), None);
        assert_eq!(h.body["max_completion_tokens"], 100);
        assert!(h.body.get("max_tokens").is_none());
        assert!(!h.headers.iter().any(|(k, _)| k == "authorization"));
    }

    #[test]
    fn builds_anthropic_request() {
        let mut s = spec("a", Provider::Anthropic, Tier::T1);
        s.base_url = "https://api.anthropic.com".into();
        let h = build(&s, &req(), Some("key"));
        assert_eq!(h.url, "https://api.anthropic.com/v1/messages");
        assert!(h.headers.contains(&("x-api-key".into(), "key".into())));
        assert!(
            h.headers
                .contains(&("anthropic-version".into(), ANTHROPIC_VERSION.into()))
        );
        assert_eq!(
            h.body,
            json!({"model": "a-model", "max_tokens": 100, "system": "Be brief.",
                   "messages": [{"role": "user", "content": "Hi"}]})
        );
        s.server_fallback = true;
        let h = build(&s, &req(), Some("key"));
        assert!(
            h.headers
                .contains(&("anthropic-beta".into(), ANTHROPIC_FALLBACK_BETA.into()))
        );
        assert_eq!(h.body["fallbacks"], "default");
        let red = h.redacted();
        assert!(red.headers.contains(&("x-api-key".into(), "***".into())));
    }

    #[test]
    fn parses_responses() {
        let mut o = spec("o", Provider::Openai, Tier::T2);
        o.price_in = Some(2.0);
        o.price_out = Some(10.0);
        let c = parse(&o, 200, OPENAI_OK).unwrap();
        assert_eq!(
            (c.text.as_str(), c.input_tokens, c.output_tokens),
            ("Hello!", 12, 3)
        );
        assert!((c.cost_usd.unwrap() - 0.000054).abs() < 1e-12);
        let a = parse(&spec("a", Provider::Anthropic, Tier::T1), 200, ANTHROPIC_OK).unwrap();
        assert_eq!(
            (a.text.as_str(), a.input_tokens, a.output_tokens),
            ("Hello!", 20, 5)
        );
        assert_eq!(a.cost_usd, None);
    }

    #[test]
    fn non_answers_are_errors() {
        let a = spec("a", Provider::Anthropic, Tier::T1);
        let refusal = r#"{"content":[],"stop_reason":"refusal","stop_details":{"type":"refusal","category":"cyber"}}"#;
        assert_eq!(
            parse(&a, 200, refusal).unwrap_err(),
            "refused (category: cyber)"
        );
        let overloaded =
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        assert_eq!(
            parse(&a, 529, overloaded).unwrap_err(),
            "HTTP 529: Overloaded"
        );
        let o = spec("o", Provider::Openai, Tier::T2);
        let filtered =
            r#"{"choices":[{"message":{"content":null},"finish_reason":"content_filter"}]}"#;
        assert!(
            parse(&o, 200, filtered)
                .unwrap_err()
                .contains("content filter")
        );
        assert_eq!(
            parse(&o, 200, "not json").unwrap_err().split(':').next(),
            Some("invalid JSON")
        );
    }

    #[test]
    fn candidate_order_escalates_upward_only() {
        let mut local = spec("local", Provider::Openai, Tier::T3);
        local.local = true;
        let cfg = Config {
            models: vec![
                spec("t1", Provider::Anthropic, Tier::T1),
                spec("t2", Provider::Openai, Tier::T2),
                local,
            ],
            timeout_secs: 5,
        };
        let ids = |v: Vec<&ModelSpec>| v.into_iter().map(|m| m.id.clone()).collect::<Vec<_>>();
        assert_eq!(
            ids(candidates(&cfg, Tier::T3, false)),
            ["local", "t2", "t1"]
        );
        assert_eq!(ids(candidates(&cfg, Tier::T2, false)), ["t2", "t1"]);
        assert_eq!(ids(candidates(&cfg, Tier::T1, false)), ["t1"]);
        assert_eq!(ids(candidates(&cfg, Tier::T3, true)), ["local"]);
        assert!(candidates(&cfg, Tier::T1, true).is_empty());
    }

    #[test]
    fn falls_back_and_skips_missing_keys() {
        let mut keyed = spec("keyed", Provider::Openai, Tier::T2);
        keyed.credential_env = Some("NO_SUCH_KEY".into());
        let cfg = Config {
            models: vec![
                keyed,
                spec("busy", Provider::Anthropic, Tier::T2),
                spec("ok", Provider::Openai, Tier::T1),
            ],
            timeout_secs: 5,
        };
        let mock = Mock::new(vec![Ok((529, "{}")), Ok((200, OPENAI_OK))]);
        let (c, attempts) = complete(&cfg, &mock, &req(), Tier::T2, false, &|_| None).unwrap();
        assert_eq!(c.model_id, "ok");
        assert_eq!(c.tier, Tier::T1);
        assert_eq!(attempts.len(), 2);
        assert!(attempts[0].error.contains("NO_SUCH_KEY is not set"));
        assert!(attempts[1].error.starts_with("HTTP 529"));
        assert_eq!(mock.seen.borrow().len(), 2, "no request without a key");
    }

    #[test]
    fn reports_when_everything_fails() {
        let cfg = Config {
            models: vec![spec("x", Provider::Openai, Tier::T1)],
            timeout_secs: 5,
        };
        let mock = Mock::new(vec![Err("connection refused")]);
        let err = complete(&cfg, &mock, &req(), Tier::T1, false, &|_| None).unwrap_err();
        assert!(err.to_string().contains("x: connection refused"), "{err}");
        let err = complete(&cfg, &mock, &req(), Tier::T1, true, &|_| None).unwrap_err();
        assert!(matches!(
            err,
            GatewayError::NoCandidate {
                local_only: true,
                ..
            }
        ));
    }

    #[test]
    fn complete_model_is_strict_and_checks_id_and_sensitivity() {
        let mut local = spec("local", Provider::Openai, Tier::T3);
        local.local = true;
        let cfg = Config {
            models: vec![spec("hosted", Provider::Openai, Tier::T2), local],
            timeout_secs: 5,
        };
        let mock = Mock::new(vec![Ok((200, OPENAI_OK))]);
        let (c, attempts) = complete_model(&cfg, &mock, &req(), "local", true, &|_| None).unwrap();
        assert_eq!(c.model_id, "local");
        assert!(attempts.is_empty());
        assert_eq!(mock.seen.borrow().len(), 1);

        // No fallback when the selected model fails, even though `local` is in config.
        let mock = Mock::new(vec![Ok((529, "{}"))]);
        let err = complete_model(&cfg, &mock, &req(), "hosted", false, &|_| None).unwrap_err();
        assert!(matches!(err, GatewayError::AllFailed { ref attempts } if attempts.len() == 1));
        assert_eq!(mock.seen.borrow().len(), 1);

        let err = complete_model(&cfg, &mock, &req(), "missing", false, &|_| None).unwrap_err();
        assert!(matches!(err, GatewayError::UnknownModel { .. }));

        let err = complete_model(&cfg, &mock, &req(), "hosted", true, &|_| None).unwrap_err();
        assert!(matches!(err, GatewayError::NotSensitiveSafe { .. }));
    }

    #[test]
    fn config_validation() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("g.yaml");
        let model = "- {id: a, provider: openai, base_url: 'http://localhost:11434/v1', model: m, tier: T3, local: true}";
        fs::write(&p, format!("models:\n  {model}\n")).unwrap();
        let cfg = load_config(&p).unwrap();
        assert_eq!(cfg.timeout_secs, 600);
        assert!(cfg.models[0].local);
        fs::write(&p, format!("models:\n  {model}\n  {model}\n")).unwrap();
        assert!(
            load_config(&p)
                .unwrap_err()
                .to_string()
                .contains("duplicate model id")
        );
        fs::write(
            &p,
            "models:\n  - {id: a, provider: openai, base_url: 'localhost', model: m, tier: T3}\n",
        )
        .unwrap();
        assert!(
            load_config(&p)
                .unwrap_err()
                .to_string()
                .contains("base_url")
        );
        fs::write(&p, "models:\n  - {id: a, provider: openai, base_url: 'http://x', model: m, tier: T3, apikey: K}\n").unwrap();
        assert!(
            load_config(&p)
                .unwrap_err()
                .to_string()
                .contains("unknown field")
        );
    }
}
