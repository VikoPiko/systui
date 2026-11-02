//! Developer workspace: configurable process families and transparent,
//! clearly-hedged heuristics. Pure functions over a snapshot.
//!
//! Heuristics only ever say "possible …"; a process is never called an
//! "orphan" merely because its parent exited or its CPU is low.

use crate::domain::ProcessSample;
use std::collections::HashMap;

pub const HIGH_FAMILY_RSS: u64 = 2 << 30;
pub const HIGH_PROC_RSS: u64 = 1 << 30;
pub const MANY_PROCS: usize = 8;
pub const IDLE_MIN_RSS: u64 = 500 << 20;
pub const IDLE_MIN_AGE_SECS: u64 = 3600;
pub const IDLE_MAX_CPU: f32 = 0.5;

#[derive(Debug, Clone)]
pub struct Family {
    pub name: String,
    tokens: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Families {
    pub defs: Vec<Family>,
}

/// Lowercase alphanumeric tokens of a process name, without a trailing `.exe`.
pub fn tokens(name: &str) -> Vec<String> {
    let lower = name.to_lowercase();
    let stem = lower.strip_suffix(".exe").unwrap_or(&lower);
    stem.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

impl Families {
    /// `names` are matched as whole name tokens (`com.docker.backend.exe` matches
    /// `docker`; `gitkraken` does not match `git`). `aliases` add extra tokens.
    pub fn new(names: &[String], aliases: &HashMap<String, Vec<String>>) -> Self {
        let defs = names
            .iter()
            .map(|n| {
                let mut t = vec![n.to_lowercase()];
                if let Some(extra) = aliases.get(n).or_else(|| aliases.get(&n.to_lowercase())) {
                    t.extend(extra.iter().map(|e| e.to_lowercase()));
                }
                Family {
                    name: n.clone(),
                    tokens: t,
                }
            })
            .collect();
        Self { defs }
    }

    /// Index of the first family (in configured order) that matches.
    pub fn family_of(&self, proc_name: &str) -> Option<usize> {
        let toks = tokens(proc_name);
        let whole = proc_name.to_lowercase();
        let whole = whole.strip_suffix(".exe").unwrap_or(&whole).to_string();
        self.defs
            .iter()
            .position(|f| f.tokens.iter().any(|t| toks.contains(t) || *t == whole))
    }
}

#[derive(Debug, Clone)]
pub struct FamilyStats {
    /// Index into `Families::defs`.
    pub index: usize,
    pub name: String,
    pub count: usize,
    /// Sum of KNOWN working sets. Shared pages are not deduplicated.
    pub rss_total: u64,
    /// Processes whose memory could not be read (not included in `rss_total`).
    pub rss_unknown: usize,
    /// Approximate sum of known CPU readings (percent of one core). `None` if none known.
    pub cpu_sum: Option<f32>,
    /// Most expensive process by memory: (pid, name, rss).
    pub top: Option<(u32, String, u64)>,
    /// Distinct executable names (sorted, at most 6).
    pub names: Vec<String>,
    pub flags: Vec<String>,
    /// PIDs of members (for drill-down).
    pub pids: Vec<u32>,
}

pub fn summarize(fams: &Families, procs: &[ProcessSample], now_unix: u64) -> Vec<FamilyStats> {
    let mut stats: Vec<FamilyStats> = fams
        .defs
        .iter()
        .enumerate()
        .map(|(index, f)| FamilyStats {
            index,
            name: f.name.clone(),
            count: 0,
            rss_total: 0,
            rss_unknown: 0,
            cpu_sum: None,
            top: None,
            names: Vec::new(),
            flags: Vec::new(),
            pids: Vec::new(),
        })
        .collect();
    let mut idle = vec![0usize; stats.len()];
    let mut big = vec![0usize; stats.len()];
    for p in procs {
        let Some(i) = fams.family_of(&p.name) else {
            continue;
        };
        let s = &mut stats[i];
        s.count += 1;
        s.pids.push(p.id.pid);
        if !s.names.contains(&p.name) {
            s.names.push(p.name.clone());
        }
        match p.rss {
            Some(r) => {
                s.rss_total += r;
                if s.top.as_ref().is_none_or(|t| r > t.2) {
                    s.top = Some((p.id.pid, p.name.clone(), r));
                }
                if r >= HIGH_PROC_RSS {
                    big[i] += 1;
                }
                let old =
                    p.id.started_at
                        .is_some_and(|st| now_unix.saturating_sub(st) >= IDLE_MIN_AGE_SECS);
                if old && r >= IDLE_MIN_RSS && p.cpu.is_some_and(|c| c < IDLE_MAX_CPU) {
                    idle[i] += 1;
                }
            }
            None => s.rss_unknown += 1,
        }
        if let Some(c) = p.cpu {
            s.cpu_sum = Some(s.cpu_sum.unwrap_or(0.0) + c);
        }
    }
    for (i, s) in stats.iter_mut().enumerate() {
        s.names.sort();
        s.names.truncate(6);
        if s.rss_total >= HIGH_FAMILY_RSS {
            s.flags
                .push("possible high memory use for this family".into());
        }
        if big[i] > 0 {
            s.flags.push(format!("{} process(es) over 1 GiB", big[i]));
        }
        if s.count >= MANY_PROCS {
            s.flags
                .push(format!("{} parallel processes (build workers?)", s.count));
        }
        if idle[i] > 0 {
            s.flags.push(format!(
                "possibly idle: {} large process(es) running >1h at <0.5% CPU (heuristic)",
                idle[i]
            ));
        }
    }
    stats.sort_by(|a, b| {
        (b.count > 0)
            .cmp(&(a.count > 0))
            .then(b.rss_total.cmp(&a.rss_total))
            .then(a.name.cmp(&b.name))
    });
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::*;
    use std::sync::Arc;

    fn p(
        pid: u32,
        name: &str,
        cpu: Option<f32>,
        rss: Option<u64>,
        started: Option<u64>,
    ) -> ProcessSample {
        ProcessSample {
            id: ProcessId {
                pid,
                started_at: started,
            },
            parent: None,
            name: name.into(),
            cpu,
            rss,
            restricted: rss.is_none(),
            io: None,
            status: None,
            info: Arc::new(ProcessStatic {
                exe: None,
                cmd: CmdLine::NotRequested,
            }),
        }
    }

    fn fams() -> Families {
        let mut a = HashMap::new();
        a.insert(
            "wsl".to_string(),
            vec!["vmmemwsl".to_string(), "wslservice".to_string()],
        );
        Families::new(
            &["dotnet", "node", "git", "docker", "wsl"].map(String::from),
            &a,
        )
    }

    #[test]
    fn token_matching_is_whole_word() {
        let f = fams();
        assert_eq!(f.defs[f.family_of("node.exe").unwrap()].name, "node");
        assert_eq!(f.defs[f.family_of("NODE").unwrap()].name, "node");
        assert_eq!(
            f.defs[f.family_of("com.docker.backend.exe").unwrap()].name,
            "docker"
        );
        assert_eq!(
            f.defs[f.family_of("Docker Desktop.exe").unwrap()].name,
            "docker"
        );
        assert_eq!(
            f.defs[f.family_of("git-remote-https.exe").unwrap()].name,
            "git"
        );
        assert!(
            f.family_of("gitkraken.exe").is_none(),
            "substring is not a match"
        );
        assert!(f.family_of("nodemon-helper").is_none());
        assert!(f.family_of("notepad.exe").is_none());
    }

    #[test]
    fn aliases_extend_a_family() {
        let f = fams();
        assert_eq!(f.defs[f.family_of("vmmemWSL").unwrap()].name, "wsl");
        assert_eq!(f.defs[f.family_of("wslservice.exe").unwrap()].name, "wsl");
    }

    #[test]
    fn first_configured_family_wins() {
        let f = Families::new(&["docker".into(), "node".into()], &HashMap::new());
        assert_eq!(
            f.defs[f.family_of("docker-node-helper").unwrap()].name,
            "docker"
        );
    }

    #[test]
    fn aggregates_known_values_and_counts_unknown_separately() {
        let procs = vec![
            p(1, "node.exe", Some(10.0), Some(100 << 20), Some(1)),
            p(2, "node.exe", Some(5.0), Some(300 << 20), Some(1)),
            p(3, "node.exe", None, None, Some(1)),
            p(4, "other.exe", Some(99.0), Some(1 << 30), Some(1)),
        ];
        let s = summarize(&fams(), &procs, 10);
        let n = s.iter().find(|x| x.name == "node").unwrap();
        assert_eq!(n.count, 3);
        assert_eq!(n.rss_total, 400 << 20);
        assert_eq!(n.rss_unknown, 1, "unknown is not added as 0");
        assert_eq!(n.cpu_sum, Some(15.0));
        assert_eq!(n.top.as_ref().unwrap().0, 2);
        assert_eq!(n.names, ["node.exe"]);
    }

    #[test]
    fn empty_family_has_unknown_cpu_not_zero_and_sorts_last() {
        let s = summarize(
            &fams(),
            &[p(1, "git.exe", Some(1.0), Some(1 << 20), None)],
            0,
        );
        assert_eq!(s[0].name, "git");
        let d = s.iter().find(|x| x.name == "dotnet").unwrap();
        assert_eq!((d.count, d.cpu_sum), (0, None));
        assert!(s.iter().position(|x| x.name == "dotnet").unwrap() > 0);
    }

    #[test]
    fn heuristics_are_hedged_and_conditional() {
        let day = 86_400;
        let procs = vec![
            p(1, "node.exe", Some(0.0), Some(800 << 20), Some(100)), // old + big + idle
            p(2, "node.exe", Some(30.0), Some(900 << 20), Some(100)), // busy
            p(3, "node.exe", Some(0.0), Some(800 << 20), None),      // age unknown: no idle claim
            p(4, "node.exe", Some(0.0), Some(100 << 20), Some(100)), // small
        ];
        let s = summarize(&fams(), &procs, day);
        let n = s.iter().find(|x| x.name == "node").unwrap();
        let txt = n.flags.join(" | ");
        assert!(txt.contains("possibly idle: 1"), "{txt}");
        assert!(!txt.contains("orphan"), "never claim orphans");
        assert!(!txt.contains("over 1 GiB"), "none are over 1 GiB: {txt}");
        // many processes + high family memory
        let many: Vec<_> = (0..9)
            .map(|i| p(10 + i, "node", Some(1.0), Some(300 << 20), Some(day)))
            .collect();
        let n = summarize(&fams(), &many, day);
        let txt = n.iter().find(|x| x.name == "node").unwrap().flags.join("|");
        assert!(
            txt.contains("9 parallel") && txt.contains("high memory"),
            "{txt}"
        );
    }

    #[test]
    fn no_idle_claim_without_cpu_reading() {
        let procs = vec![p(1, "node", None, Some(800 << 20), Some(1))];
        let s = summarize(&fams(), &procs, 1_000_000);
        assert!(s
            .iter()
            .find(|x| x.name == "node")
            .unwrap()
            .flags
            .is_empty());
    }
}
