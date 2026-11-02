//! Real Docker / WSL / NVML through the real module workers. Each assertion is
//! written so it also holds on a machine WITHOUT the tool (then the module must
//! say "unavailable", not crash or show zeros).

use std::time::{Duration, Instant};
use systui::config::Config;
use systui::modules::{ModStatus, ModuleSet, Updates};

fn collect(set: &ModuleSet, want_all: bool, secs: u64) -> Updates {
    let mut acc = Updates::default();
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        let u = set.take();
        acc.docker = u.docker.or(acc.docker);
        acc.wsl = u.wsl.or(acc.wsl);
        acc.gpu = u.gpu.or(acc.gpu);
        if want_all && acc.docker.is_some() && acc.wsl.is_some() && acc.gpu.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    acc
}

#[test]
fn enabled_modules_report_real_state_or_clean_unavailability() {
    let mut cfg = Config::default();
    cfg.modules.docker = true;
    cfg.modules.wsl = true;
    cfg.modules.gpu = true;
    let mut set = ModuleSet::new(&cfg);
    assert!(!set.started());
    set.set_visible(true);
    let u = collect(&set, true, 40);

    // ---- Docker
    let d = u.docker.expect("docker module answered within 40 s");
    match &d.status {
        ModStatus::Ok => {
            let data = d.data.as_ref().unwrap();
            assert!(data.version.is_some());
            for c in &data.containers {
                assert!(!c.id.is_empty() && !c.name.is_empty(), "{c:?}");
                assert!(!c.name.chars().any(char::is_control));
                if c.state != "running" {
                    assert!(
                        c.cpu_percent.is_none(),
                        "stopped container must have unknown CPU"
                    );
                }
                if let Some(p) = c.cpu_percent {
                    assert!(p >= 0.0 && p.is_finite());
                }
            }
            assert!(data.df.is_none(), "system df is NOT polled automatically");
            eprintln!(
                "docker: ok, engine {:?}, {} containers ({} running)",
                data.version,
                data.containers.len(),
                data.containers
                    .iter()
                    .filter(|c| c.state == "running")
                    .count()
            );
        }
        ModStatus::Unavailable(m) | ModStatus::Error(m) => eprintln!("docker: {:?} {m}", d.status),
        other => panic!("unexpected {other:?}"),
    }

    // ---- WSL
    let w = u.wsl.expect("wsl module answered");
    match &w.status {
        ModStatus::Ok => {
            let data = w.data.as_ref().unwrap();
            for x in &data.distros {
                assert!(
                    !x.name.is_empty() && !x.state.is_empty() && (x.version == 1 || x.version == 2),
                    "{x:?}"
                );
            }
            eprintln!("wsl: ok, distros {:?}", data.distros);
        }
        ModStatus::Unavailable(m) | ModStatus::Error(m) => {
            eprintln!("wsl: {:?} {m}", w.status);
            if !cfg!(windows) {
                assert!(m.contains("Windows-only"));
            }
        }
        other => panic!("unexpected {other:?}"),
    }

    // ---- GPU
    let g = u.gpu.expect("gpu module answered");
    match &g.status {
        ModStatus::Ok => {
            let data = g.data.as_ref().unwrap();
            for x in &data.gpus {
                if let (Some(u), Some(t)) = (x.mem_used, x.mem_total) {
                    assert!(t > 0 && u <= t);
                }
                if let Some(p) = x.util_gpu_percent {
                    assert!(p <= 100);
                }
            }
            eprintln!("gpu: ok {:?}", data.gpus);
        }
        ModStatus::Unavailable(m) | ModStatus::Error(m) => eprintln!("gpu: {:?} {m}", g.status),
        other => panic!("unexpected {other:?}"),
    }

    // Explicit refresh with extras (docker system df) populates df, only on request.
    set.refresh(systui::modules::ModuleKind::Docker, true);
    let u2 = collect(&set, false, 30);
    if let Some(d2) = u2.docker {
        if let (ModStatus::Ok, Some(data)) = (&d2.status, &d2.data) {
            let df = data.df.as_ref().expect("df present after explicit refresh");
            assert!(!df.is_empty());
            eprintln!("docker df: {df:?}");
        }
    }
}

#[test]
fn hidden_dev_tab_means_no_polling() {
    let mut cfg = Config::default();
    cfg.modules.docker = true;
    let mut set = ModuleSet::new(&cfg);
    set.set_visible(true);
    let _ = collect(&set, false, 20);
    set.set_visible(false);
    std::thread::sleep(Duration::from_millis(500));
    let _ = set.take(); // drain
    std::thread::sleep(Duration::from_secs(7)); // > docker cadence (5 s)
    let u = set.take();
    assert!(u.docker.is_none(), "no polls while the tab is hidden");
}
