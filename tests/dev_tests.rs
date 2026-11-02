mod common;
use common::*;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use systui::app::{App, DevPane, Effect, Tab};
use systui::config::Config;
use systui::modules::docker::{Container, DfRow, DockerData};
use systui::modules::gpu::{GpuData, GpuInfo};
use systui::modules::wsl::{Distro, Vhdx, VhdxSize, WslData};
use systui::modules::{ModStatus, ModuleKind, ModuleSnapshot, Updates};
use systui::ui::{self, theme::Theme, RenderOpts};

fn k(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}
fn press(a: &mut App, s: &str) {
    for ch in s.chars() {
        a.handle_key(k(KeyCode::Char(ch)));
    }
}

fn dev_procs() -> Vec<systui::domain::ProcessSample> {
    let mut v = many_procs(20); // proc0001.. unrelated
    v.push(proc(1001, "node.exe", Some(12.0), 400 << 20));
    v.push(proc(1002, "node.exe", Some(3.0), 900 << 20));
    v.push(proc(1003, "dotnet.exe", Some(1.0), 300 << 20));
    v.push(proc(1004, "com.docker.backend.exe", Some(2.0), 1 << 30));
    v
}

fn app_with(cfg: Config) -> App {
    let mut a = App::new(cfg);
    a.ingest(Arc::new(snapshot_with(1, dev_procs())), Instant::now());
    a
}

/// Effects other than the collector `Needs` bookkeeping.
fn eff(a: &mut App) -> Vec<Effect> {
    a.take_effects()
        .into_iter()
        .filter(|e| !matches!(e, Effect::Needs(_)))
        .collect()
}

fn snap<T>(status: ModStatus, data: Option<T>) -> Arc<ModuleSnapshot<T>> {
    Arc::new(ModuleSnapshot {
        seq: 1,
        status,
        data,
        observed_at: SystemTime::now(),
    })
}

fn draw(w: u16, h: u16, a: &mut App) -> String {
    let opts = RenderOpts {
        theme: Theme::mono(),
        unicode: true,
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

fn container(name: &str, state: &str, cpu: Option<f32>) -> Container {
    Container {
        id: "abc123".into(),
        name: name.into(),
        image: "img".into(),
        project: String::new(),
        state: state.into(),
        status: "Up".into(),
        cpu_percent: cpu,
        mem_usage: cpu.map(|_| "45.5MiB / 7.6GiB".into()),
        mem_percent: cpu.map(|_| 0.5),
    }
}

fn gpu_data() -> GpuData {
    GpuData {
        driver: Some("551.23".into()),
        more: 0,
        gpus: vec![GpuInfo {
            index: 0,
            name: Some("Test GPU 9000".into()),
            util_gpu_percent: Some(25),
            util_mem_percent: None,
            mem_used: Some(2 << 30),
            mem_total: Some(8 << 30),
            temp_c: Some(61),
            power_watts: None, // driver did not report it
            fan_percent: None,
        }],
    }
}

// ------------------------------------------------------------ effects / state

#[test]
fn dev_visibility_effects_fire_only_on_enter_and_leave() {
    let mut a = app_with(Config::default());
    assert!(eff(&mut a).is_empty(), "nothing before the tab is opened");
    press(&mut a, "2");
    assert!(!eff(&mut a).contains(&Effect::DevVisible(true)));
    press(&mut a, "5");
    assert_eq!(a.tab, Tab::Developer);
    assert_eq!(eff(&mut a), vec![Effect::DevVisible(true)]);
    press(&mut a, "j"); // staying on the tab: no repeat
    assert!(eff(&mut a).is_empty());
    press(&mut a, "1");
    assert_eq!(eff(&mut a), vec![Effect::DevVisible(false)]);
}

#[test]
fn services_tab_is_available_since_m5() {
    let mut a = app_with(Config::default());
    press(&mut a, "6");
    assert_eq!(a.tab, Tab::Services);
}

#[test]
fn tab_key_cycles_dev_panes() {
    let mut a = app_with(Config::default());
    press(&mut a, "5");
    assert_eq!(a.dev.pane, DevPane::Families);
    let seen: Vec<DevPane> = (0..4)
        .map(|_| {
            a.handle_key(k(KeyCode::Tab));
            a.dev.pane
        })
        .collect();
    assert_eq!(
        seen,
        [
            DevPane::Docker,
            DevPane::Wsl,
            DevPane::Gpu,
            DevPane::Families
        ]
    );
    a.handle_key(k(KeyCode::BackTab));
    assert_eq!(a.dev.pane, DevPane::Gpu);
}

#[test]
fn refresh_key_targets_the_focused_enabled_module() {
    let mut cfg = Config::default();
    cfg.modules.docker = true;
    let mut a = app_with(cfg);
    press(&mut a, "5");
    eff(&mut a);
    press(&mut a, "R");
    assert!(
        a.notice.as_deref().unwrap().contains("Tab"),
        "families pane explains"
    );
    a.handle_key(k(KeyCode::Tab)); // docker
    press(&mut a, "R");
    assert_eq!(
        eff(&mut a),
        vec![Effect::ModuleRefresh(ModuleKind::Docker, true)]
    );
    a.handle_key(k(KeyCode::Tab)); // wsl: disabled by default
    press(&mut a, "R");
    assert!(eff(&mut a).is_empty());
    assert!(a.notice.as_deref().unwrap().contains("disabled"));
}

#[test]
fn defaults_enable_only_gpu() {
    let c = Config::default();
    assert!(c.modules.gpu && !c.modules.docker && !c.modules.wsl);
}

// ----------------------------------------------------------------- families

#[test]
fn families_table_aggregates_real_snapshot_values() {
    let mut a = app_with(Config::default());
    press(&mut a, "5");
    let node = a.dev.stats.iter().find(|s| s.name == "node").unwrap();
    assert_eq!(node.count, 2);
    assert_eq!(node.rss_total, 1300 << 20);
    let docker = a.dev.stats.iter().find(|s| s.name == "docker").unwrap();
    assert_eq!(
        docker.count, 1,
        "com.docker.backend.exe is a docker-family member"
    );
    let s = draw(120, 40, &mut a);
    assert!(
        s.contains("node") && s.contains("1.3 GiB")
            || s.contains("1.27 GiB")
            || s.contains("1.3 GiB"),
        "{s}"
    );
    assert!(s.contains("Process families"));
}

#[test]
fn enter_opens_only_that_family_in_processes_and_esc_clears() {
    let mut a = app_with(Config::default());
    press(&mut a, "5");
    let idx = a.dev.stats.iter().position(|s| s.name == "node").unwrap();
    a.dev.family_table.select(Some(idx));
    a.handle_key(k(KeyCode::Enter));
    assert_eq!(a.tab, Tab::Processes);
    let snap = a.snapshot.clone().unwrap();
    assert_eq!(a.procs.rows.len(), 2);
    assert!(a
        .procs
        .rows
        .iter()
        .all(|&i| snap.processes[i].name == "node.exe"));
    let s = draw(120, 40, &mut a);
    assert!(s.contains("Processes 2/"), "{s}");
    a.handle_key(k(KeyCode::Esc));
    assert!(a.procs.family.is_none());
    assert_eq!(a.procs.rows.len(), snap.processes.len());
}

#[test]
fn drilling_into_an_empty_family_explains_instead_of_showing_nothing() {
    let mut a = app_with(Config::default());
    press(&mut a, "5");
    let idx = a.dev.stats.iter().position(|s| s.name == "rustc").unwrap();
    a.dev.family_table.select(Some(idx));
    a.handle_key(k(KeyCode::Enter));
    assert_eq!(a.tab, Tab::Developer);
    assert!(a.notice.as_deref().unwrap().contains("no running"));
}

#[test]
fn family_filter_combines_with_text_filter() {
    let mut a = app_with(Config::default());
    press(&mut a, "5");
    let idx = a.dev.stats.iter().position(|s| s.name == "node").unwrap();
    a.dev.family_table.select(Some(idx));
    a.handle_key(k(KeyCode::Enter));
    press(&mut a, "/1002");
    a.handle_key(k(KeyCode::Enter));
    assert_eq!(a.procs.rows.len(), 1);
}

// ------------------------------------------------------------------ rendering

#[test]
fn every_module_state_is_labelled_not_hidden() {
    let mut cfg = Config::default();
    cfg.modules.docker = true;
    cfg.modules.wsl = true;
    let mut a = app_with(cfg);
    press(&mut a, "5");

    // before the first poll
    let s = draw(120, 40, &mut a);
    assert!(s.contains("checking"), "{s}");

    a.ingest_modules(
        Updates {
            docker: Some(snap::<DockerData>(
                ModStatus::Unavailable("engine not reachable: pipe not found".into()),
                None,
            )),
            wsl: Some(snap::<WslData>(
                ModStatus::Error("wsl.exe did not answer in time (timeout)".into()),
                None,
            )),
            gpu: Some(snap::<GpuData>(
                ModStatus::Unavailable("NVIDIA driver/NVML not found".into()),
                None,
            )),
        },
        Instant::now(),
    );
    let s = draw(120, 40, &mut a);
    assert!(s.contains("engine not reachable"), "{s}");
    assert!(s.contains("timeout"), "{s}");
    assert!(s.contains("NVML not found"), "{s}");
}

#[test]
fn disabled_modules_say_how_to_enable_and_nothing_else() {
    let mut cfg = Config::default();
    cfg.modules.gpu = false;
    let mut a = app_with(cfg);
    press(&mut a, "5");
    let s = draw(120, 40, &mut a);
    assert!(s.contains("[modules] docker = true"), "{s}");
    assert!(s.contains("[modules] wsl = true"));
    assert!(s.contains("[modules] gpu = true"));
    assert!(s.contains("disabled"));
}

#[test]
fn good_data_renders_with_units_and_na_for_unreported_fields() {
    let mut cfg = Config::default();
    cfg.modules.docker = true;
    cfg.modules.wsl = true;
    let mut a = app_with(cfg);
    press(&mut a, "5");
    a.ingest_modules(
        Updates {
            docker: Some(snap(
                ModStatus::Ok,
                Some(DockerData {
                    version: Some("28.3.2".into()),
                    containers: vec![
                        container("web-1", "running", Some(1.5)),
                        container("old", "exited", None),
                    ],
                    df: Some(vec![DfRow {
                        kind: "Images".into(),
                        total: "12".into(),
                        active: "5".into(),
                        size: "6.2GB".into(),
                        reclaimable: "3.1GB (50%)".into(),
                    }]),
                    df_at: Some(SystemTime::now()),
                    warning: Some("stats unavailable: timeout".into()),
                    skipped_lines: 0,
                }),
            )),
            wsl: Some(snap(
                ModStatus::Ok,
                Some(WslData {
                    distros: vec![Distro {
                        name: "Ubuntu".into(),
                        state: "Running".into(),
                        version: 2,
                        default: true,
                    }],
                    vhdx: vec![
                        Vhdx {
                            path: "C:\\wsl\\ext4.vhdx".into(),
                            size: VhdxSize::Bytes(30 << 30),
                        },
                        Vhdx {
                            path: "C:\\gone.vhdx".into(),
                            size: VhdxSize::NotFound,
                        },
                    ],
                    skipped_lines: 0,
                }),
            )),
            gpu: Some(snap(ModStatus::Ok, Some(gpu_data()))),
        },
        Instant::now(),
    );
    let s = draw(130, 44, &mut a);
    for want in [
        "28.3.2",
        "1 running / 2 total",
        "web-1",
        "1.5%",
        "45.5MiB / 7.6GiB",
        "system df",
        "6.2GB",
        "not .vhdx savings",
        "do not add it",
        "warning: stats unavailable",
        "Ubuntu",
        "WSL2",
        "30.0 GiB",
        "not found",
        "not guest free space",
        "Test GPU 9000",
        "25%",
        "2.0 GiB / 8.0 GiB",
        "61",
        "power n/a",
        "fan n/a",
    ] {
        assert!(s.contains(want), "missing `{want}` in:\n{s}");
    }
    // the stopped container shows an unknown CPU, not 0
    let old = s.lines().find(|l| l.contains("old")).unwrap();
    assert!(old.contains('—') && !old.contains("0.0%"), "{old}");
}

#[test]
fn error_after_good_data_shows_stale_banner_and_keeps_data() {
    let mut cfg = Config::default();
    cfg.modules.docker = true;
    let mut a = app_with(cfg);
    press(&mut a, "5");
    a.ingest_modules(
        Updates {
            docker: Some(snap(
                ModStatus::Error("docker did not answer in time (timeout)".into()),
                Some(DockerData {
                    version: None,
                    containers: vec![container("web-1", "running", Some(1.0))],
                    df: None,
                    df_at: None,
                    warning: None,
                    skipped_lines: 0,
                }),
            )),
            ..Default::default()
        },
        Instant::now(),
    );
    let s = draw(130, 44, &mut a);
    assert!(
        s.contains("error: docker did not answer") && s.contains("last good data"),
        "{s}"
    );
    assert!(s.contains("web-1"));
}

#[test]
fn old_updates_are_marked_stale() {
    let mut cfg = Config::default();
    cfg.modules.docker = true;
    let mut a = app_with(cfg);
    press(&mut a, "5");
    let old = Instant::now()
        .checked_sub(std::time::Duration::from_secs(120))
        .unwrap();
    a.ingest_modules(
        Updates {
            docker: Some(snap(
                ModStatus::Ok,
                Some(DockerData {
                    version: None,
                    containers: vec![],
                    df: None,
                    df_at: None,
                    warning: None,
                    skipped_lines: 0,
                }),
            )),
            ..Default::default()
        },
        old,
    );
    assert!(draw(130, 44, &mut a).contains("STALE"));
}

#[test]
fn hostile_container_text_cannot_inject_escapes() {
    // Parsers sanitize; verify the full path parser -> screen.
    let (c, _) = systui::modules::docker::parse_ps("id1\t\x1b[2Jevil\timg\trunning\tUp\t\n");
    let mut cfg = Config::default();
    cfg.modules.docker = true;
    let mut a = app_with(cfg);
    press(&mut a, "5");
    a.ingest_modules(
        Updates {
            docker: Some(snap(
                ModStatus::Ok,
                Some(DockerData {
                    version: None,
                    containers: c,
                    df: None,
                    df_at: None,
                    warning: None,
                    skipped_lines: 0,
                }),
            )),
            ..Default::default()
        },
        Instant::now(),
    );
    assert!(!draw(130, 44, &mut a).contains('\x1b'));
}

#[test]
fn dev_tab_renders_at_every_size_in_every_pane_without_panic() {
    let mut cfg = Config::default();
    cfg.modules.docker = true;
    cfg.modules.wsl = true;
    let mut a = app_with(cfg);
    press(&mut a, "5");
    a.ingest_modules(
        Updates {
            gpu: Some(snap(ModStatus::Ok, Some(gpu_data()))),
            ..Default::default()
        },
        Instant::now(),
    );
    for _ in 0..4 {
        for (w, h) in [
            (120, 40),
            (90, 26),
            (70, 20),
            (60, 18),
            (50, 10),
            (0, 0),
            (1, 1),
            (300, 6),
        ] {
            let _ = draw(w, h, &mut a);
        }
        a.handle_key(k(KeyCode::Tab));
    }
}

#[test]
fn compact_layout_shows_one_panel_at_a_time() {
    let mut a = app_with(Config::default());
    press(&mut a, "5");
    let s = draw(70, 20, &mut a);
    assert!(s.contains("Process families") && !s.contains("GPU (NVIDIA)"));
    a.handle_key(k(KeyCode::Tab));
    a.handle_key(k(KeyCode::Tab));
    a.handle_key(k(KeyCode::Tab));
    assert!(draw(70, 20, &mut a).contains("GPU (NVIDIA)"));
}

// --------------------------------------------------------------------- config

mod config {
    use std::path::Path;
    use systui::config::Config;

    fn parse(s: &str) -> Result<Config, systui::config::ConfigError> {
        Config::from_toml(s, Path::new("t.toml"))
    }

    #[test]
    fn modules_and_developer_sections_parse() {
        // an absolute path in this platform's own syntax (TOML literal string: no escapes)
        let vhdx = if cfg!(windows) {
            r"C:\WSL\Ubuntu\ext4.vhdx"
        } else {
            "/home/u/wsl/ext4.vhdx"
        };
        let c = parse(&format!(
            "[modules]\ndocker = true\nwsl = true\ngpu = false\n\
             [developer]\nprocess_families = [\"node\", \"mycorp-build\"]\n\
             [developer.aliases]\nnode = [\"bun\"]\n\
             [wsl]\ntracked_vhdx_paths = ['{vhdx}']\n"
        ));
        let c = c.unwrap();
        assert!(c.modules.docker && c.modules.wsl && !c.modules.gpu);
        assert_eq!(c.developer.process_families, ["node", "mycorp-build"]);
        assert_eq!(c.developer.aliases["node"], ["bun"]);
        assert_eq!(c.wsl.tracked_vhdx_paths.len(), 1);
    }

    #[test]
    fn default_families_follow_the_spec_and_alias_wsl() {
        let c = Config::default();
        for f in [
            "dotnet", "node", "pnpm", "npm", "cargo", "rustc", "git", "docker",
        ] {
            assert!(c.developer.process_families.iter().any(|x| x == f), "{f}");
        }
        assert!(c.developer.aliases.contains_key("wsl"));
        assert!(
            c.wsl.tracked_vhdx_paths.is_empty(),
            "no implicit paths ever"
        );
    }

    #[test]
    fn vhdx_paths_must_be_explicit_absolute_vhdx_files() {
        let bad = [
            "relative\\\\ext4.vhdx",
            "C:\\\\x\\\\notvhdx.txt",
            "C:\\\\a\\\\..\\\\b\\\\x.vhdx",
        ];
        for p in bad {
            let r = parse(&format!("[wsl]\ntracked_vhdx_paths = [\"{p}\"]\n"));
            if cfg!(windows) || p.starts_with('/') {
                assert!(r.is_err(), "{p} should be rejected");
            }
        }
        let e = parse("[wsl]\ntracked_vhdx_paths = [\"C:\\\\x\\\\notvhdx.txt\"]\n");
        if cfg!(windows) {
            assert!(e.unwrap_err().to_string().contains("tracked_vhdx_paths"));
        }
    }

    #[test]
    fn family_list_is_bounded() {
        assert!(parse("[developer]\nprocess_families = [\"\"]\n").is_err());
        let many: Vec<String> = (0..40).map(|i| format!("\"f{i}\"")).collect();
        assert!(parse(&format!(
            "[developer]\nprocess_families = [{}]\n",
            many.join(",")
        ))
        .is_err());
    }

    #[test]
    fn example_config_documents_new_sections() {
        let t = std::fs::read_to_string("config/config.example.toml").unwrap();
        for k in ["[modules]", "[developer]", "[wsl]", "tracked_vhdx_paths"] {
            assert!(t.contains(k), "{k}");
        }
        parse(&t).unwrap();
    }
}
