mod common;
use common::*;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use systui::app::{App, Effect, Tab};
use systui::config::{AiKind, Config};
use systui::modules::ai::{
    AiData, EngineMetrics, InstalledModel, LoadedModel, ServerInfo, ServerReport, ServerState,
};
use systui::modules::gpu::{GpuData, GpuInfo};
use systui::modules::{ModStatus, ModuleKind, ModuleSnapshot, Updates};
use systui::ui::{self, theme::Theme, RenderOpts};

fn press(a: &mut App, c: char) {
    a.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
}

fn snap<T>(status: ModStatus, data: Option<T>) -> Arc<ModuleSnapshot<T>> {
    Arc::new(ModuleSnapshot {
        seq: 1,
        status,
        data,
        observed_at: SystemTime::now(),
    })
}

fn draw(w: u16, h: u16, a: &mut App, unicode: bool) -> String {
    let opts = RenderOpts {
        theme: Theme::mono(),
        unicode,
    };
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::render(f, a, &opts)).unwrap();
    let b = t.backend().buffer().clone();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| b[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn app_on_ai(cfg: Config) -> App {
    let mut a = App::new(cfg);
    let mut procs = many_procs(5);
    procs.push(proc(900, "ollama.exe", Some(4.0), 300 << 20));
    procs.push(proc(901, "ollama_llama_server.exe", Some(30.0), 5 << 30));
    a.ingest(Arc::new(snapshot_with(1, procs)), Instant::now());
    press(&mut a, '7');
    a
}

fn model(name: &str, size: Option<u64>, vram: Option<u64>) -> LoadedModel {
    LoadedModel {
        name: name.into(),
        size,
        size_vram: vram,
        expires_unix: None,
        params: Some("8.0B".into()),
        quant: Some("Q4_K_M".into()),
        family: Some("llama".into()),
        context: Some(8192),
    }
}

fn up(name: &str, kind: AiKind, info: ServerInfo) -> ServerReport {
    ServerReport {
        name: name.into(),
        kind,
        display: "127.0.0.1:11434".into(),
        state: ServerState::Up(Box::new(info)),
    }
}

fn info(loaded: Vec<LoadedModel>) -> ServerInfo {
    ServerInfo {
        version: Some("0.33.0".into()),
        loaded,
        loaded_label: "Loaded in memory",
        installed: Some(vec![InstalledModel {
            name: "qwen3.5:9b".into(),
            size: Some(6 << 30),
            params: Some("9B".into()),
            quant: None,
            max_context: Some(262_144),
        }]),
        metrics: None,
        note: Some("Ollama does not expose live tokens/s or request counts.".into()),
        latency_ms: 12,
    }
}

fn feed(a: &mut App, d: AiData) {
    a.ingest_modules(
        Updates {
            ai: Some(snap(ModStatus::Ok, Some(d))),
            ..Default::default()
        },
        Instant::now(),
    );
}

fn gpu_data(used: u64, total: u64, util: Option<u32>) -> GpuData {
    GpuData {
        driver: Some("1.0".into()),
        gpus: vec![GpuInfo {
            index: 0,
            name: Some("Test GPU".into()),
            util_gpu_percent: util,
            util_mem_percent: None,
            mem_used: Some(used),
            mem_total: Some(total),
            temp_c: None,
            power_watts: None,
            fan_percent: None,
        }],
        more: 0,
    }
}

fn effects(a: &mut App) -> Vec<Effect> {
    a.take_effects()
        .into_iter()
        .filter(|e| !matches!(e, Effect::Needs(_)))
        .collect()
}

#[test]
fn opening_and_leaving_the_tab_toggles_the_worker_visibility() {
    let mut a = app_on_ai(Config::default());
    assert!(effects(&mut a).contains(&Effect::AiVisible(true)));
    press(&mut a, '1');
    assert!(effects(&mut a).contains(&Effect::AiVisible(false)));
    assert_eq!(a.tab, Tab::Overview);
}

#[test]
fn r_requests_a_full_refresh_only_when_enabled() {
    let mut a = app_on_ai(Config::default());
    effects(&mut a);
    press(&mut a, 'R');
    assert!(effects(&mut a).contains(&Effect::ModuleRefresh(ModuleKind::Ai, true)));

    let mut cfg = Config::default();
    cfg.modules.ai = false;
    let mut a = app_on_ai(cfg);
    effects(&mut a);
    press(&mut a, 'R');
    assert!(effects(&mut a).is_empty());
    assert!(a.notice.unwrap().contains("disabled"));
}

#[test]
fn disabled_tab_says_so_and_probes_nothing() {
    let mut cfg = Config::default();
    cfg.modules.ai = false;
    let mut a = app_on_ai(cfg);
    let s = draw(120, 40, &mut a, true);
    assert!(s.contains("disabled") && s.contains("modules] ai"), "{s}");
}

#[test]
fn before_the_first_poll_it_says_checking_not_zero() {
    let mut a = app_on_ai(Config::default());
    let s = draw(120, 40, &mut a, true);
    assert!(s.contains("checking local servers"), "{s}");
}

#[test]
fn loaded_model_shows_gpu_split_and_engine_processes() {
    let mut a = app_on_ai(Config::default());
    feed(
        &mut a,
        AiData {
            servers: vec![
                up(
                    "Ollama",
                    AiKind::Ollama,
                    info(vec![model("llama3:8b", Some(6 << 30), Some(3 << 30))]),
                ),
                ServerReport {
                    name: "LM Studio".into(),
                    kind: AiKind::Lmstudio,
                    display: "127.0.0.1:1234".into(),
                    state: ServerState::NotRunning,
                },
            ],
        },
    );
    let s = draw(140, 60, &mut a, true);
    for want in [
        "Ollama",
        "v0.33.0",
        "llama3:8b",
        "8.0B",
        "Q4_K_M",
        "ctx 8192",
        "50% on GPU, 50% on CPU/RAM",
        "not running",
        "installed",
        "qwen3.5:9b",
        "does not expose live tokens/s",
        // process family detection feeds the "engine processes" section
        "Ollama",
        "2 proc",
    ] {
        assert!(s.contains(want), "missing {want:?} in\n{s}");
    }
}

#[test]
fn unknown_sizes_are_dashes_never_zero() {
    let mut a = app_on_ai(Config::default());
    let mut m = model("mystery", None, None);
    m.quant = None;
    m.params = None;
    m.family = None;
    m.context = None;
    feed(
        &mut a,
        AiData {
            servers: vec![up("Ollama", AiKind::Ollama, info(vec![m]))],
        },
    );
    let s = draw(140, 60, &mut a, true);
    assert!(s.contains("mystery"), "{s}");
    assert!(!s.contains("0 B"), "no invented zero sizes:\n{s}");
    assert!(!s.contains("on GPU"), "no GPU split without sizes:\n{s}");
}

#[test]
fn metrics_show_dashes_for_absent_series() {
    let mut a = app_on_ai(Config::default());
    let mut i = info(vec![model("m", None, None)]);
    i.loaded_label = "Served models";
    i.installed = None;
    i.metrics = Some(EngineMetrics {
        gen_tps: Some(41.25),
        running: Some(1.0),
        waiting: Some(0.0),
        kv_cache: Some(0.5),
        prompt_tps: None,
    });
    feed(
        &mut a,
        AiData {
            servers: vec![up("llama.cpp", AiKind::Openai, i)],
        },
    );
    let s = draw(140, 60, &mut a, true);
    for want in [
        "Served models",
        "generation 41.2 tok/s",
        "prompt —",
        "KV cache 50%",
        "server averages",
    ] {
        assert!(s.contains(want), "missing {want:?}\n{s}");
    }
}

#[test]
fn errors_and_empty_state_are_reported() {
    let mut a = app_on_ai(Config::default());
    feed(
        &mut a,
        AiData {
            servers: vec![
                ServerReport {
                    name: "Ollama".into(),
                    kind: AiKind::Ollama,
                    display: "127.0.0.1:11434".into(),
                    state: ServerState::Error("http 500".into()),
                },
                ServerReport {
                    name: "vLLM".into(),
                    kind: AiKind::Openai,
                    display: "127.0.0.1:8000".into(),
                    state: ServerState::NotRunning,
                },
            ],
        },
    );
    let s = draw(140, 60, &mut a, true);
    assert!(s.contains("error: http 500"), "{s}");
    assert!(s.contains("No local AI server answered"), "{s}");
}

#[test]
fn gpu_pressure_and_trend_use_real_readings_only() {
    let mut a = app_on_ai(Config::default());
    for (u, util) in [(2u64, Some(10u32)), (4, Some(90)), (6, None)] {
        a.ingest_modules(
            Updates {
                gpu: Some(snap(ModStatus::Ok, Some(gpu_data(u << 30, 10 << 30, util)))),
                ..Default::default()
            },
            Instant::now(),
        );
    }
    assert_eq!(a.ai.vram.len(), 3);
    assert_eq!(
        a.ai.gpu_util.len(),
        2,
        "an unreported util is skipped, not pushed as 0"
    );
    let s = draw(140, 60, &mut a, true);
    assert!(s.contains("VRAM 6.0 GiB / 10.0 GiB"), "{s}");
    assert!(s.contains("util —"), "{s}");

    // A failed poll must not extend the trend with stale numbers.
    a.ingest_modules(
        Updates {
            gpu: Some(snap(
                ModStatus::Error("boom".into()),
                Some(gpu_data(1 << 30, 10 << 30, Some(1))),
            )),
            ..Default::default()
        },
        Instant::now(),
    );
    assert_eq!(a.ai.vram.len(), 3);
}

#[test]
fn ascii_mode_has_no_unicode_glyphs_and_small_sizes_do_not_panic() {
    let mut a = app_on_ai(Config::default());
    feed(
        &mut a,
        AiData {
            servers: vec![up(
                "Ollama",
                AiKind::Ollama,
                info(vec![model(
                    &"very-long-model-name/".repeat(10),
                    Some(1 << 30),
                    Some(1 << 30),
                )]),
            )],
        },
    );
    let s = draw(100, 40, &mut a, false);
    assert!(
        !s.chars()
            .any(|c| matches!(c, '\u{2500}'..='\u{259F}' | '●' | '○')),
        "ASCII mode leaked box/block/dot glyphs:\n{s}"
    );
    for (w, h) in [(60, 18), (50, 12), (80, 20), (0, 0), (1, 1)] {
        let _ = draw(w, h, &mut a, true);
    }
}

#[test]
fn scrolling_is_clamped_and_resets_with_home() {
    let mut a = app_on_ai(Config::default());
    for _ in 0..500 {
        press(&mut a, 'j');
    }
    draw(100, 30, &mut a, true);
    assert!(a.ai.scroll < 500, "render clamps scroll to the content");
    press(&mut a, 'g');
    assert_eq!(a.ai.scroll, 0);
}

#[test]
fn palette_can_open_the_tab() {
    let mut a = App::new(Config::default());
    a.ingest(Arc::new(snapshot_with(1, many_procs(3))), Instant::now());
    a.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
    for c in "local ai".chars() {
        press(&mut a, c);
    }
    a.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(a.tab, Tab::Ai);
}

#[test]
fn config_defaults_and_validation() {
    let c = Config::default();
    assert!(c.modules.ai);
    assert_eq!(c.ai.endpoints.len(), 4);
    assert!(c.validate().is_ok());
    let load = |t: &str| Config::from_toml(t, std::path::Path::new("t.toml"));

    let e = load(
        "[[ai.endpoints]]\nname = \"Remote\"\nkind = \"ollama\"\nurl = \"http://192.168.1.10:11434\"\n",
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("Remote") && e.contains("loopback"), "{e}");
    assert!(
        !e.contains("allow_lan"),
        "must not point at the health setting: {e}"
    );

    let e = load(
        "[[ai.endpoints]]\nname = \"Mine\"\nkind = \"openai\"\nurl = \"http://localhost:9999\"\n\
         [[ai.endpoints]]\nname = \"mine\"\nurl = \"http://127.0.0.1:1\"\n",
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("duplicate"), "{e}");

    let e = load("[[ai.endpoints]]\nname = \"T\"\nurl = \"https://localhost:1\"\n")
        .unwrap_err()
        .to_string();
    assert!(e.contains("TLS"), "{e}");

    let c =
        load("[[ai.endpoints]]\nname = \"A\"\nkind = \"ollama\"\nurl = \"http://localhost:1\"\n")
            .unwrap();
    assert_eq!(
        c.ai.endpoints.len(),
        1,
        "a configured list replaces the defaults"
    );
}

/// Against whatever local AI server is really running (Ollama here). Skips silently
/// when nothing answers, so it is safe on any machine; never loads or changes anything.
#[test]
fn live_local_servers_are_read_sanely_and_render() {
    let cfg = Config::default();
    let reports = systui::modules::ai::probe_once(&cfg.ai.endpoints);
    assert_eq!(reports.len(), cfg.ai.endpoints.len());
    for r in &reports {
        let ServerState::Up(i) = &r.state else {
            continue;
        };
        for m in &i.loaded {
            if let (Some(s), Some(v)) = (m.size, m.size_vram) {
                assert!(v <= s, "VRAM part cannot exceed the model size: {m:?}");
            }
            assert!(!m.name.is_empty() && !m.name.contains('\u{1b}'));
        }
        assert!(i.latency_ms < 800);
    }
    let mut a = app_on_ai(cfg);
    feed(&mut a, AiData { servers: reports });
    let s = draw(140, 50, &mut a, true);
    println!("{s}");
    assert!(s.contains("Local AI"));
}
