//! Local AI monitoring: asks loopback inference servers (Ollama, LM Studio and any
//! OpenAI-compatible server such as llama.cpp or vLLM) what they report, read-only.
//!
//! Everything here is "what the server says": a field the server does not expose is
//! `None` (shown as `—`), never 0. Nothing leaves the machine: endpoints are validated
//! as loopback `http://` URLs at config load and only fixed GET paths are requested.

use super::ModStatus;
use crate::config::{AiEndpointConfig, AiKind};
use crate::domain::ProcessSample;
use crate::util::http::{self, HttpError};
use crate::util::text::sanitize;
use serde_json::Value;
use std::time::{Duration, Instant};

pub const CADENCE: Duration = Duration::from_secs(2);
const TIMEOUT: Duration = Duration::from_millis(800);
const MAX_BODY: usize = 1 << 20;
/// Installed-model lists change rarely: re-read them every N polls (or on `R`).
const INFO_EVERY: u32 = 15;
const MAX_MODELS: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedModel {
    pub name: String,
    /// Total bytes the server says the loaded model occupies (weights + KV cache as it reports).
    pub size: Option<u64>,
    /// Part of `size` the server says is resident in GPU memory.
    pub size_vram: Option<u64>,
    /// Unix seconds at which the server will unload the model if idle.
    pub expires_unix: Option<i64>,
    pub params: Option<String>,
    pub quant: Option<String>,
    pub family: Option<String>,
    /// Context length the model is loaded with (or its maximum, see `context_is_max`).
    pub context: Option<u64>,
}

impl LoadedModel {
    /// Fraction of the model the server reports in GPU memory (0..=1).
    pub fn gpu_fraction(&self) -> Option<f64> {
        match (self.size, self.size_vram) {
            (Some(t), Some(v)) if t > 0 => Some((v as f64 / t as f64).clamp(0.0, 1.0)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InstalledModel {
    pub name: String,
    pub size: Option<u64>,
    pub params: Option<String>,
    pub quant: Option<String>,
    pub max_context: Option<u64>,
}

/// Throughput/queue numbers from a Prometheus `/metrics` endpoint, when the server has it enabled.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EngineMetrics {
    /// Server-side AVERAGES (llama.cpp reports cumulative averages, not instantaneous speed).
    pub prompt_tps: Option<f64>,
    pub gen_tps: Option<f64>,
    pub running: Option<f64>,
    pub waiting: Option<f64>,
    /// 0..=1
    pub kv_cache: Option<f64>,
}

impl EngineMetrics {
    fn is_empty(&self) -> bool {
        *self == EngineMetrics::default()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServerInfo {
    pub version: Option<String>,
    /// Models the server reports as loaded / being served right now.
    pub loaded: Vec<LoadedModel>,
    /// What `loaded` means for this API, shown in the panel.
    pub loaded_label: &'static str,
    pub installed: Option<Vec<InstalledModel>>,
    pub metrics: Option<EngineMetrics>,
    /// Short note about something this server does not expose (never an error).
    pub note: Option<String>,
    pub latency_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServerState {
    Up(Box<ServerInfo>),
    /// Nothing is listening on the endpoint.
    NotRunning,
    /// Something answered but we could not read it (timeout, non-200, not the expected API).
    Error(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServerReport {
    pub name: String,
    pub kind: AiKind,
    pub display: String,
    pub state: ServerState,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AiData {
    pub servers: Vec<ServerReport>,
}

// ------------------------------------------------------------------ parsing

fn s_field(v: &Value, k: &str) -> Option<String> {
    let s = v.get(k)?.as_str()?;
    let s = sanitize(s);
    let s: String = s.chars().take(96).collect();
    (!s.is_empty()).then_some(s)
}

/// Ollama writes `"unknown"` when it does not know; that is "no data", not a value.
fn known(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.eq_ignore_ascii_case("unknown"))
}

/// Unix seconds from an RFC 3339 timestamp (`2026-11-11T16:13:54.02+02:00`, `...Z`).
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    let n = |a: usize, e: usize| -> Option<i64> { s.get(a..e)?.parse().ok() };
    let (y, mo, d) = (n(0, 4)?, n(5, 7)?, n(8, 10)?);
    let (h, mi, sec) = (n(11, 13)?, n(14, 16)?, n(17, 19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let mut rest = &s[19..];
    if let Some(f) = rest.strip_prefix('.') {
        let digits = f.chars().take_while(char::is_ascii_digit).count();
        rest = &f[digits..];
    }
    let off = match rest {
        "Z" | "z" => 0,
        r if r.len() == 6 && (r.starts_with('+') || r.starts_with('-')) && &r[3..4] == ":" => {
            let oh: i64 = r.get(1..3)?.parse().ok()?;
            let om: i64 = r.get(4..6)?.parse().ok()?;
            let v = oh * 3600 + om * 60;
            if r.starts_with('-') {
                -v
            } else {
                v
            }
        }
        _ => return None,
    };
    // Days from civil (Howard Hinnant's algorithm).
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + sec - off)
}

pub fn parse_ollama_ps(v: &Value) -> Option<Vec<LoadedModel>> {
    let arr = v.get("models")?.as_array()?;
    Some(
        arr.iter()
            .take(MAX_MODELS)
            .filter_map(|m| {
                let name = s_field(m, "name").or_else(|| s_field(m, "model"))?;
                let d = m.get("details");
                let df = |k: &str| d.and_then(|d| s_field(d, k));
                Some(LoadedModel {
                    name,
                    size: m.get("size").and_then(Value::as_u64),
                    size_vram: m.get("size_vram").and_then(Value::as_u64),
                    expires_unix: m
                        .get("expires_at")
                        .and_then(Value::as_str)
                        .and_then(parse_rfc3339),
                    params: known(df("parameter_size")),
                    quant: known(df("quantization_level")),
                    family: known(df("family")),
                    context: m.get("context_length").and_then(Value::as_u64),
                })
            })
            .collect(),
    )
}

pub fn parse_ollama_tags(v: &Value) -> Option<Vec<InstalledModel>> {
    let arr = v.get("models")?.as_array()?;
    Some(
        arr.iter()
            .take(MAX_MODELS)
            .filter_map(|m| {
                let name = s_field(m, "name").or_else(|| s_field(m, "model"))?;
                let d = m.get("details");
                let df = |k: &str| d.and_then(|d| s_field(d, k));
                Some(InstalledModel {
                    name,
                    size: m.get("size").and_then(Value::as_u64),
                    params: known(df("parameter_size")),
                    quant: known(df("quantization_level")),
                    max_context: d
                        .and_then(|d| d.get("context_length"))
                        .and_then(Value::as_u64),
                })
            })
            .collect(),
    )
}

/// LM Studio `/api/v0/models`: every downloaded model with a `state` of `loaded` / `not-loaded`.
pub fn parse_lmstudio(v: &Value) -> Option<(Vec<LoadedModel>, Vec<InstalledModel>)> {
    let arr = v.get("data")?.as_array()?;
    let (mut loaded, mut installed) = (Vec::new(), Vec::new());
    for m in arr.iter().take(MAX_MODELS * 2) {
        let Some(name) = s_field(m, "id") else {
            continue;
        };
        // Embedding models are listed too; they are real loaded models, so keep them.
        let quant = known(s_field(m, "quantization"));
        let max_ctx = m.get("max_context_length").and_then(Value::as_u64);
        if m.get("state").and_then(Value::as_str) == Some("loaded") {
            loaded.push(LoadedModel {
                name: name.clone(),
                size: None,
                size_vram: None,
                expires_unix: None,
                params: None,
                quant: quant.clone(),
                family: known(s_field(m, "arch")),
                context: m
                    .get("loaded_context_length")
                    .and_then(Value::as_u64)
                    .or(max_ctx),
            });
        }
        installed.push(InstalledModel {
            name,
            size: None,
            params: None,
            quant,
            max_context: max_ctx,
        });
    }
    installed.truncate(MAX_MODELS);
    Some((loaded, installed))
}

/// OpenAI-style `/v1/models`: the ids the server is serving.
pub fn parse_openai_models(v: &Value) -> Option<Vec<LoadedModel>> {
    let arr = v.get("data")?.as_array()?;
    Some(
        arr.iter()
            .take(MAX_MODELS)
            .filter_map(|m| {
                Some(LoadedModel {
                    name: s_field(m, "id")?,
                    size: None,
                    size_vram: None,
                    expires_unix: None,
                    params: None,
                    quant: None,
                    family: None,
                    context: None,
                })
            })
            .collect(),
    )
}

/// First sample of the first metric name that exists in Prometheus text.
fn prom(text: &str, names: &[&str]) -> Option<f64> {
    for want in names {
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') || !line.starts_with(want) {
                continue;
            }
            let after = &line[want.len()..];
            // exact metric name: next char is `{` or whitespace
            if !(after.starts_with('{') || after.starts_with(char::is_whitespace)) {
                continue;
            }
            let val = line.rsplit(char::is_whitespace).next()?;
            if let Ok(f) = val.parse::<f64>() {
                if f.is_finite() {
                    return Some(f);
                }
            }
        }
    }
    None
}

/// llama.cpp (`--metrics`) and vLLM expose different names; unknown ones simply stay `None`.
pub fn parse_metrics(text: &str) -> EngineMetrics {
    EngineMetrics {
        prompt_tps: prom(text, &["llamacpp:prompt_tokens_seconds"]),
        gen_tps: prom(text, &["llamacpp:predicted_tokens_seconds"]),
        running: prom(
            text,
            &["llamacpp:requests_processing", "vllm:num_requests_running"],
        ),
        waiting: prom(
            text,
            &["llamacpp:requests_deferred", "vllm:num_requests_waiting"],
        ),
        kv_cache: prom(
            text,
            &[
                "llamacpp:kv_cache_usage_ratio",
                "vllm:kv_cache_usage_perc",
                "vllm:gpu_cache_usage_perc",
            ],
        )
        .map(|r| r.clamp(0.0, 1.0)),
    }
}

// ------------------------------------------------------------------ probing

#[derive(Debug, Clone)]
pub struct Endpoint {
    pub name: String,
    pub kind: AiKind,
    host: String,
    port: u16,
    pub display: String,
}

impl Endpoint {
    pub fn from_config(c: &AiEndpointConfig) -> Result<Self, String> {
        let (host, port, _pq, _display) = crate::health::parse_http_url(&c.url, false)?;
        let shown = if host.contains(':') {
            format!("[{host}]")
        } else {
            host.clone()
        };
        Ok(Self {
            name: sanitize(c.name.trim()),
            kind: c.kind,
            display: format!("{shown}:{port}"),
            host,
            port,
        })
    }

    fn get(&self, path: &str) -> Result<(u16, Vec<u8>), HttpError> {
        http::get(&self.host, self.port, path, TIMEOUT, MAX_BODY)
    }

    fn get_json(&self, path: &str) -> Result<Value, ProbeErr> {
        let (status, body) = self.get(path).map_err(ProbeErr::from)?;
        if status != 200 {
            return Err(ProbeErr::Status(status));
        }
        serde_json::from_slice(&body).map_err(|_| {
            ProbeErr::Other("reply is not the expected JSON (different server?)".into())
        })
    }
}

enum ProbeErr {
    Down,
    Status(u16),
    Other(String),
}

impl From<HttpError> for ProbeErr {
    fn from(e: HttpError) -> Self {
        match e {
            HttpError::Refused => ProbeErr::Down,
            other => ProbeErr::Other(other.describe()),
        }
    }
}

impl ProbeErr {
    fn into_state(self) -> ServerState {
        match self {
            ProbeErr::Down => ServerState::NotRunning,
            ProbeErr::Status(c) => ServerState::Error(format!("http {c}")),
            ProbeErr::Other(m) => ServerState::Error(sanitize(&m)),
        }
    }
}

/// Slow-changing data kept between polls.
#[derive(Default)]
struct Cache {
    version: Option<String>,
    installed: Option<Vec<InstalledModel>>,
    primed: bool,
}

fn probe_ollama(ep: &Endpoint, cache: &mut Cache, full: bool) -> ServerState {
    let t = Instant::now();
    let ps = match ep.get_json("/api/ps") {
        Ok(v) => v,
        Err(e) => return e.into_state(),
    };
    let latency_ms = t.elapsed().as_millis() as u64;
    let Some(loaded) = parse_ollama_ps(&ps) else {
        return ServerState::Error("`/api/ps` reply has no `models` list (not Ollama?)".into());
    };
    if full || !cache.primed {
        cache.primed = true;
        if let Ok(v) = ep.get_json("/api/version") {
            cache.version = s_field(&v, "version");
        }
        if let Ok(v) = ep.get_json("/api/tags") {
            if let Some(list) = parse_ollama_tags(&v) {
                cache.installed = Some(list);
            }
        }
    }
    ServerState::Up(Box::new(ServerInfo {
        version: cache.version.clone(),
        loaded,
        loaded_label: "Loaded in memory",
        installed: cache.installed.clone(),
        metrics: None,
        note: Some("Ollama does not expose live tokens/s or request counts.".into()),
        latency_ms,
    }))
}

fn probe_lmstudio(ep: &Endpoint, cache: &mut Cache, _full: bool) -> ServerState {
    let t = Instant::now();
    match ep.get_json("/api/v0/models") {
        Ok(v) => {
            let latency_ms = t.elapsed().as_millis() as u64;
            let Some((loaded, installed)) = parse_lmstudio(&v) else {
                return ServerState::Error("`/api/v0/models` reply has no `data` list".into());
            };
            cache.primed = true;
            ServerState::Up(Box::new(ServerInfo {
                version: None,
                loaded,
                loaded_label: "Loaded in memory",
                installed: Some(installed),
                metrics: None,
                note: Some(
                    "LM Studio does not report memory use per model or tokens/s over its API."
                        .into(),
                ),
                latency_ms,
            }))
        }
        // Older builds without the v0 REST API: fall back to the OpenAI-style list.
        Err(ProbeErr::Status(404)) => probe_openai(ep, cache, false),
        Err(e) => e.into_state(),
    }
}

fn probe_openai(ep: &Endpoint, _cache: &mut Cache, _full: bool) -> ServerState {
    let t = Instant::now();
    let v = match ep.get_json("/v1/models") {
        Ok(v) => v,
        Err(e) => return e.into_state(),
    };
    let latency_ms = t.elapsed().as_millis() as u64;
    let Some(loaded) = parse_openai_models(&v) else {
        return ServerState::Error("`/v1/models` reply has no `data` list".into());
    };
    let (metrics, note) = match ep.get("/metrics") {
        Ok((200, body)) => {
            let m = parse_metrics(&String::from_utf8_lossy(&body));
            if m.is_empty() {
                (
                    None,
                    Some("/metrics is on but has no known llama.cpp/vLLM series.".to_string()),
                )
            } else {
                (Some(m), None)
            }
        }
        Ok((404, _)) => (
            None,
            Some(
                "no /metrics (llama.cpp: start with --metrics) so no throughput/queue data.".into(),
            ),
        ),
        Ok((c, _)) => (None, Some(format!("/metrics answered http {c}."))),
        Err(e) => (None, Some(format!("/metrics unreadable: {}", e.describe()))),
    };
    ServerState::Up(Box::new(ServerInfo {
        version: None,
        loaded,
        loaded_label: "Served models",
        installed: None,
        metrics,
        note,
        latency_ms,
    }))
}

fn probe(ep: &Endpoint, cache: &mut Cache, full: bool) -> ServerReport {
    let state = match ep.kind {
        AiKind::Ollama => probe_ollama(ep, cache, full),
        AiKind::Lmstudio => probe_lmstudio(ep, cache, full),
        AiKind::Openai => probe_openai(ep, cache, full),
    };
    ServerReport {
        name: ep.name.clone(),
        kind: ep.kind,
        display: ep.display.clone(),
        state,
    }
}

/// Probe every endpoint concurrently (a hung one cannot delay the others).
fn poll_all(eps: &[Endpoint], caches: &mut [Cache], full: bool) -> AiData {
    let servers = std::thread::scope(|s| {
        let handles: Vec<_> = eps
            .iter()
            .zip(caches.iter_mut())
            .map(|(e, c)| s.spawn(move || probe(e, c, full)))
            .collect();
        handles
            .into_iter()
            .zip(eps)
            .map(|(h, e)| {
                h.join().unwrap_or_else(|_| ServerReport {
                    name: e.name.clone(),
                    kind: e.kind,
                    display: e.display.clone(),
                    state: ServerState::Error("probe failed unexpectedly".into()),
                })
            })
            .collect()
    });
    AiData { servers }
}

pub fn spawn(cfg: &[AiEndpointConfig]) -> super::ModuleHandle<AiData> {
    let eps: Vec<Endpoint> = cfg
        .iter()
        .filter(|c| c.enabled)
        .filter_map(|c| Endpoint::from_config(c).ok())
        .collect();
    let mut caches: Vec<Cache> = eps.iter().map(|_| Cache::default()).collect();
    let mut n = 0u32;
    super::ModuleHandle::spawn("ai", CADENCE, move |extra| {
        if eps.is_empty() {
            return Err(ModStatus::Unavailable(
                "no enabled endpoints (see [[ai.endpoints]] in the config)".into(),
            ));
        }
        let full = extra || n.is_multiple_of(INFO_EVERY);
        n = n.wrapping_add(1);
        Ok(poll_all(&eps, &mut caches, full))
    })
}

/// One-shot probe for `doctor`: which configured servers answer right now.
pub fn probe_once(cfg: &[AiEndpointConfig]) -> Vec<ServerReport> {
    let eps: Vec<Endpoint> = cfg
        .iter()
        .filter(|c| c.enabled)
        .filter_map(|c| Endpoint::from_config(c).ok())
        .collect();
    let mut caches: Vec<Cache> = eps.iter().map(|_| Cache::default()).collect();
    poll_all(&eps, &mut caches, true).servers
}

// ------------------------------------------------------- engine processes

/// Known local-AI executables, matched on the process name only. Python-hosted engines
/// (`vllm serve`, text-generation-webui, ComfyUI) appear as `python` and are NOT detected.
pub fn engine_of(name: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();
    let stem = lower.strip_suffix(".exe").unwrap_or(&lower);
    Some(match stem {
        s if s.starts_with("ollama") => "Ollama",
        "lms" | "lmstudio" => "LM Studio",
        s if s.starts_with("lm studio") || s.starts_with("lm-studio") => "LM Studio",
        s if s.starts_with("llama-") || s == "llama" => "llama.cpp",
        s if s.starts_with("llamafile") => "llamafile",
        s if s.starts_with("koboldcpp") => "KoboldCpp",
        s if s.starts_with("vllm") => "vLLM",
        "localai" | "local-ai" => "LocalAI",
        s if s.starts_with("gpt4all") => "GPT4All",
        "jan" => "Jan",
        _ => return None,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct EngineProcs {
    pub engine: &'static str,
    pub count: usize,
    /// Sum of KNOWN working sets (shared pages not deduplicated).
    pub rss_total: u64,
    pub rss_unknown: usize,
    pub cpu_sum: Option<f32>,
}

pub fn summarize_processes(procs: &[ProcessSample]) -> Vec<EngineProcs> {
    let mut out: Vec<EngineProcs> = Vec::new();
    for p in procs {
        let Some(engine) = engine_of(&p.name) else {
            continue;
        };
        let i = match out.iter().position(|e| e.engine == engine) {
            Some(i) => i,
            None => {
                out.push(EngineProcs {
                    engine,
                    count: 0,
                    rss_total: 0,
                    rss_unknown: 0,
                    cpu_sum: None,
                });
                out.len() - 1
            }
        };
        let e = &mut out[i];
        e.count += 1;
        match p.rss {
            Some(r) => e.rss_total = e.rss_total.saturating_add(r),
            None => e.rss_unknown += 1,
        }
        if let Some(c) = p.cpu {
            e.cpu_sum = Some(e.cpu_sum.unwrap_or(0.0) + c);
        }
    }
    out.sort_by(|a, b| b.rss_total.cmp(&a.rss_total).then(a.engine.cmp(b.engine)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn rfc3339_known_values() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(parse_rfc3339("1970-01-01T02:00:00+02:00"), Some(0));
        assert_eq!(parse_rfc3339("1969-12-31T19:00:00-05:00"), Some(0));
        assert_eq!(
            parse_rfc3339("2026-11-11T16:13:54.0249392+02:00"),
            parse_rfc3339("2026-11-11T14:13:54Z")
        );
        for bad in [
            "",
            "garbage",
            "2026-13-01T00:00:00Z",
            "2026-01-01T25:00:00Z",
            "2026-01-01T00:00:00",
        ] {
            assert_eq!(parse_rfc3339(bad), None, "{bad}");
        }
    }

    #[test]
    fn ollama_ps_reports_vram_split_and_unknown_as_none() {
        let v = json!({"models":[{
            "name":"llama3:8b","model":"llama3:8b","size":6_000_000_000u64,"size_vram":4_500_000_000u64,
            "expires_at":"2026-11-11T16:20:00Z","context_length":8192,
            "details":{"family":"llama","parameter_size":"8.0B","quantization_level":"Q4_0"}},
            {"name":"x\u{1b}[31m","details":{"quantization_level":"unknown"}}]});
        let m = parse_ollama_ps(&v).unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].gpu_fraction(), Some(0.75));
        assert_eq!(m[0].context, Some(8192));
        assert_eq!(m[0].quant.as_deref(), Some("Q4_0"));
        assert!(m[0].expires_unix.is_some());
        assert_eq!(m[1].quant, None, "`unknown` is no data");
        assert_eq!(m[1].size, None);
        assert_eq!(m[1].gpu_fraction(), None, "no sizes => no fraction, not 0");
        assert!(!m[1].name.contains('\u{1b}'), "terminal escapes sanitized");
        assert_eq!(parse_ollama_ps(&json!({"models":[]})).unwrap().len(), 0);
        assert!(parse_ollama_ps(&json!({"nope":1})).is_none());
    }

    #[test]
    fn ollama_tags_real_shape() {
        let v = json!({"models":[{"name":"qwen3.5:9b","size":6547291716u64,
            "details":{"family":"qwen35","parameter_size":"8.95B","quantization_level":"unknown","context_length":262144}}]});
        let t = parse_ollama_tags(&v).unwrap();
        assert_eq!(t[0].size, Some(6_547_291_716));
        assert_eq!(t[0].max_context, Some(262_144));
        assert_eq!(t[0].quant, None);
    }

    #[test]
    fn lmstudio_splits_loaded_from_installed() {
        let v = json!({"data":[
            {"id":"a","state":"loaded","quantization":"Q4_K_M","max_context_length":32768,"loaded_context_length":4096,"arch":"qwen2"},
            {"id":"b","state":"not-loaded","max_context_length":8192}]});
        let (l, i) = parse_lmstudio(&v).unwrap();
        assert_eq!((l.len(), i.len()), (1, 2));
        assert_eq!(l[0].context, Some(4096));
        assert_eq!(l[0].family.as_deref(), Some("qwen2"));
    }

    #[test]
    fn prometheus_llamacpp_and_vllm() {
        let l = "# HELP x\nllamacpp:prompt_tokens_seconds 812.5\nllamacpp:predicted_tokens_seconds 41.2\n\
                 llamacpp:requests_processing 1\nllamacpp:requests_deferred 0\nllamacpp:kv_cache_usage_ratio 0.25\n\
                 llamacpp:prompt_tokens_seconds_total 9\n";
        let m = parse_metrics(l);
        assert_eq!(m.prompt_tps, Some(812.5));
        assert_eq!(m.gen_tps, Some(41.2));
        assert_eq!(
            (m.running, m.waiting, m.kv_cache),
            (Some(1.0), Some(0.0), Some(0.25))
        );
        let v = "vllm:num_requests_running{model_name=\"m\"} 3.0\nvllm:num_requests_waiting{model_name=\"m\"} 2.0\n\
                 vllm:gpu_cache_usage_perc{model_name=\"m\"} 0.5\n";
        let m = parse_metrics(v);
        assert_eq!(
            (m.running, m.waiting, m.kv_cache),
            (Some(3.0), Some(2.0), Some(0.5))
        );
        assert_eq!(m.gen_tps, None, "absent series stay None, not 0");
        assert!(parse_metrics("something_else 1\nllamacpp:requests_processing NaN\n").is_empty());
    }

    #[test]
    fn engine_names_are_matched_conservatively() {
        for (n, e) in [
            ("ollama.exe", "Ollama"),
            ("ollama app.exe", "Ollama"),
            ("ollama_llama_server", "Ollama"),
            ("llama-server.exe", "llama.cpp"),
            ("LM Studio.exe", "LM Studio"),
            ("lms.exe", "LM Studio"),
            ("koboldcpp.exe", "KoboldCpp"),
        ] {
            assert_eq!(engine_of(n), Some(e), "{n}");
        }
        for n in [
            "python.exe",
            "llamafactory",
            "lmsys",
            "janitor.exe",
            "node",
            "gpt",
        ] {
            assert_eq!(engine_of(n), None, "{n}");
        }
    }

    // --- against a tiny local fake server -------------------------------------------------

    fn fake_server(routes: HashMap<&'static str, (u16, String)>) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for c in l.incoming() {
                let Ok(mut c) = c else { break };
                let mut b = [0u8; 2048];
                let n = c.read(&mut b).unwrap_or(0);
                let req = String::from_utf8_lossy(&b[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
                let (code, body) = routes
                    .get(path.as_str())
                    .cloned()
                    .unwrap_or((404, "{}".into()));
                let _ = write!(
                    c,
                    "HTTP/1.1 {code} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        port
    }

    fn ep(kind: AiKind, port: u16) -> Endpoint {
        Endpoint::from_config(&AiEndpointConfig {
            name: "t".into(),
            kind,
            url: format!("http://127.0.0.1:{port}"),
            enabled: true,
        })
        .unwrap()
    }

    #[test]
    fn ollama_end_to_end_and_caching() {
        let mut r = HashMap::new();
        r.insert(
            "/api/ps",
            (
                200,
                json!({"models":[{"name":"m","size":10,"size_vram":10}]}).to_string(),
            ),
        );
        r.insert(
            "/api/version",
            (200, json!({"version":"0.33.0"}).to_string()),
        );
        r.insert(
            "/api/tags",
            (200, json!({"models":[{"name":"m","size":10}]}).to_string()),
        );
        let e = ep(AiKind::Ollama, fake_server(r));
        let mut cache = Cache::default();
        let ServerState::Up(i) = probe_ollama(&e, &mut cache, false) else {
            panic!("expected Up")
        };
        assert_eq!(i.version.as_deref(), Some("0.33.0"));
        assert_eq!(i.loaded.len(), 1);
        assert_eq!(i.installed.as_ref().map(Vec::len), Some(1));
        assert_eq!(i.loaded[0].gpu_fraction(), Some(1.0));
        assert!(
            i.metrics.is_none(),
            "Ollama has no throughput metrics: say so, don't invent"
        );
    }

    #[test]
    fn openai_server_with_and_without_metrics() {
        let models = json!({"data":[{"id":"qwen.gguf"}]}).to_string();
        let mut r = HashMap::new();
        r.insert("/v1/models", (200, models.clone()));
        r.insert(
            "/metrics",
            (200, "llamacpp:predicted_tokens_seconds 33.0\n".to_string()),
        );
        let ServerState::Up(i) = probe_openai(
            &ep(AiKind::Openai, fake_server(r)),
            &mut Cache::default(),
            false,
        ) else {
            panic!()
        };
        assert_eq!(i.metrics.unwrap().gen_tps, Some(33.0));
        let mut r = HashMap::new();
        r.insert("/v1/models", (200, models));
        let ServerState::Up(i) = probe_openai(
            &ep(AiKind::Openai, fake_server(r)),
            &mut Cache::default(),
            false,
        ) else {
            panic!()
        };
        assert!(i.metrics.is_none() && i.note.unwrap().contains("--metrics"));
    }

    #[test]
    fn errors_are_distinguished() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let closed = l.local_addr().unwrap().port();
        drop(l);
        assert_eq!(
            probe(&ep(AiKind::Ollama, closed), &mut Cache::default(), false).state,
            ServerState::NotRunning
        );
        let mut r = HashMap::new();
        r.insert("/api/ps", (500, "{}".to_string()));
        assert_eq!(
            probe_ollama(
                &ep(AiKind::Ollama, fake_server(r)),
                &mut Cache::default(),
                false
            ),
            ServerState::Error("http 500".into())
        );
        let mut r = HashMap::new();
        r.insert("/api/ps", (200, "<html>hi</html>".to_string()));
        assert!(matches!(
            probe_ollama(
                &ep(AiKind::Ollama, fake_server(r)),
                &mut Cache::default(),
                false
            ),
            ServerState::Error(_)
        ));
    }

    #[test]
    fn lmstudio_falls_back_to_openai_list_on_404() {
        let mut r = HashMap::new();
        r.insert(
            "/v1/models",
            (200, json!({"data":[{"id":"x"}]}).to_string()),
        );
        let ServerState::Up(i) = probe_lmstudio(
            &ep(AiKind::Lmstudio, fake_server(r)),
            &mut Cache::default(),
            false,
        ) else {
            panic!()
        };
        assert_eq!((i.loaded.len(), i.loaded_label), (1, "Served models"));
    }

    #[test]
    fn endpoints_must_be_loopback() {
        let bad = |u: &str| {
            Endpoint::from_config(&AiEndpointConfig {
                name: "x".into(),
                kind: AiKind::Ollama,
                url: u.into(),
                enabled: true,
            })
        };
        assert!(bad("http://127.0.0.1:11434").is_ok());
        assert!(bad("http://localhost:11434").is_ok());
        assert!(bad("http://example.com:11434").is_err());
        assert!(bad("http://192.168.1.5:11434").is_err());
        assert!(bad("https://127.0.0.1:11434").is_err());
    }

    #[test]
    fn process_summary_counts_only_known_values() {
        use crate::domain::{ProcessId, ProcessStatic};
        use std::sync::Arc;
        let mk = |pid, name: &str, rss: Option<u64>, cpu: Option<f32>| ProcessSample {
            id: ProcessId {
                pid,
                started_at: Some(1),
            },
            parent: None,
            name: name.into(),
            cpu,
            rss,
            restricted: rss.is_none(),
            status: None,
            io: None,
            info: Arc::new(ProcessStatic {
                exe: None,
                cmd: crate::domain::CmdLine::NotRequested,
            }),
        };
        let s = summarize_processes(&[
            mk(1, "ollama.exe", Some(100), Some(1.0)),
            mk(2, "ollama.exe", None, None),
            mk(3, "chrome.exe", Some(999), Some(5.0)),
            mk(4, "llama-server", Some(500), None),
        ]);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].engine, "llama.cpp");
        assert_eq!(s[0].cpu_sum, None, "no CPU reading => unknown, not 0");
        assert_eq!(
            (s[1].count, s[1].rss_total, s[1].rss_unknown, s[1].cpu_sum),
            (2, 100, 1, Some(1.0))
        );
    }
}
