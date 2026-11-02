//! Ctrl+K command palette. Searches a fixed list of INTERNAL commands only;
//! the query text is never parsed as, or passed to, a shell.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    GoOverview,
    GoProcesses,
    GoNetwork,
    GoStorage,
    GoDeveloper,
    GoServices,
    TogglePause,
    RefreshNow,
    FilterProcesses,
    CycleSort,
    ReverseSort,
    TerminateSelected,
    ExportProcess,
    ToggleTopBy,
    Help,
    WhatChanged,
    ToggleRecording,
    Quit,
}

pub struct Spec {
    pub cmd: Command,
    pub title: &'static str,
    pub keys: &'static str,
}

pub const ALL: &[Spec] = &[
    Spec {
        cmd: Command::GoOverview,
        title: "Go to Overview",
        keys: "1",
    },
    Spec {
        cmd: Command::GoProcesses,
        title: "Go to Processes",
        keys: "2",
    },
    Spec {
        cmd: Command::GoNetwork,
        title: "Go to Network",
        keys: "3",
    },
    Spec {
        cmd: Command::GoStorage,
        title: "Go to Storage",
        keys: "4",
    },
    Spec {
        cmd: Command::GoDeveloper,
        title: "Go to Dev workspace",
        keys: "5",
    },
    Spec {
        cmd: Command::GoServices,
        title: "Go to Services & health",
        keys: "6",
    },
    Spec {
        cmd: Command::TogglePause,
        title: "Pause / resume display",
        keys: "Space",
    },
    Spec {
        cmd: Command::RefreshNow,
        title: "Refresh now",
        keys: "r",
    },
    Spec {
        cmd: Command::FilterProcesses,
        title: "Filter processes",
        keys: "/",
    },
    Spec {
        cmd: Command::CycleSort,
        title: "Cycle process sort key",
        keys: "s",
    },
    Spec {
        cmd: Command::ReverseSort,
        title: "Reverse process sort order",
        keys: "S",
    },
    Spec {
        cmd: Command::ToggleTopBy,
        title: "Overview: top list CPU / memory",
        keys: "s",
    },
    Spec {
        cmd: Command::TerminateSelected,
        title: "Terminate selected process…",
        keys: "K",
    },
    Spec {
        cmd: Command::ExportProcess,
        title: "Export selected process (sanitized JSON)",
        keys: "x",
    },
    Spec {
        cmd: Command::WhatChanged,
        title: "What changed? (process diff)",
        keys: "w",
    },
    Spec {
        cmd: Command::ToggleRecording,
        title: "Start / stop session recording (JSONL)",
        keys: "Ctrl+R",
    },
    Spec {
        cmd: Command::Help,
        title: "Show help",
        keys: "?",
    },
    Spec {
        cmd: Command::Quit,
        title: "Quit",
        keys: "q",
    },
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaletteState {
    pub query: String,
    pub selected: usize,
}

/// Case-insensitive subsequence match. Higher is better; `None` = no match.
/// Rewards consecutive characters and matches at word starts.
pub fn fuzzy(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut qi, mut score, mut prev_match) = (0usize, 0i32, false);
    for (i, &c) in t.iter().enumerate() {
        if qi < q.len() && c == q[qi] {
            score += 1;
            if prev_match {
                score += 3;
            }
            if i == 0 || !t[i - 1].is_alphanumeric() {
                score += 5;
            }
            prev_match = true;
            qi += 1;
        } else {
            prev_match = false;
        }
    }
    if qi != q.len() {
        return None;
    }
    // A contiguous occurrence beats any scattered subsequence (typing "record" should
    // find "recording", not a title that merely contains r-e-c-o-r-d spread out).
    let squashed: String = t.iter().filter(|c| !c.is_whitespace()).collect();
    let needle: String = q.iter().collect();
    if let Some(pos) = squashed.find(&needle) {
        score += 100;
        let at_word_start = pos == 0
            || !squashed[..pos]
                .chars()
                .last()
                .is_some_and(char::is_alphanumeric);
        if at_word_start {
            score += 40;
        }
    }
    Some(score - (t.len() as i32) / 8)
}

/// Commands matching `query`, best first (stable for equal scores).
pub fn search(query: &str) -> Vec<&'static Spec> {
    let mut v: Vec<(i32, usize, &Spec)> = ALL
        .iter()
        .enumerate()
        .filter_map(|(i, s)| fuzzy(query, s.title).map(|sc| (sc, i, s)))
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    v.into_iter().map(|(_, _, s)| s).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence_matching() {
        assert!(fuzzy("gop", "Go to Processes").is_some());
        assert!(fuzzy("zzz", "Go to Processes").is_none());
        assert!(fuzzy("", "anything").is_some());
        assert!(
            fuzzy("GO TO", "go to network").is_some(),
            "case/space insensitive"
        );
    }

    #[test]
    fn better_matches_rank_first() {
        let r = search("quit");
        assert_eq!(r[0].cmd, Command::Quit);
        let r = search("proc");
        assert!(
            r[0].title.to_lowercase().contains("proc"),
            "top hit must contain the typed text, got {:?}",
            r[0].title
        );
    }

    #[test]
    fn contiguous_matches_outrank_scattered_subsequences() {
        assert_eq!(search("record")[0].cmd, Command::ToggleRecording);
        assert_eq!(search("rec")[0].cmd, Command::ToggleRecording);
        assert_eq!(
            search("sort")[0].cmd,
            Command::CycleSort,
            "the sort commands, not scattered letters"
        );
        assert_eq!(search("chang")[0].cmd, Command::WhatChanged);
        assert_eq!(search("pause")[0].cmd, Command::TogglePause);
        assert_eq!(search("network")[0].cmd, Command::GoNetwork);
        // subsequence matching still works when nothing is contiguous
        assert!(search("gonet").iter().any(|s| s.cmd == Command::GoNetwork));
    }

    #[test]
    fn empty_query_lists_everything_in_order() {
        assert_eq!(search("").len(), ALL.len());
        assert_eq!(search("")[0].cmd, Command::GoOverview);
    }

    #[test]
    fn shell_text_matches_nothing_and_executes_nothing() {
        assert!(search("rm -rf / && calc.exe").is_empty());
        assert!(search("; whoami").is_empty());
    }
}
