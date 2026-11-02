//! Process-table view model: filter, sort and identity-stable selection.
//!
//! Selection is tracked by `ProcessId` (PID + start time), never by row index,
//! so re-sorting or a refresh does not make the cursor jump to another process.

use crate::config::SortKey;
use crate::domain::{ProcessId, ProcessSample};
use crate::util::ring::History;
use ratatui::widgets::TableState;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

pub const MAX_FILTER_LEN: usize = 64;

/// Per-row tree information (only populated in tree mode).
#[derive(Debug, Clone, PartialEq)]
pub struct TreeRow {
    pub depth: u16,
    /// Number of direct children.
    pub kids: usize,
    pub collapsed: bool,
    /// Whole subtree including this process.
    pub sub_count: usize,
    /// Sum of KNOWN working sets (shared pages counted repeatedly).
    pub sub_rss: u64,
    pub sub_rss_unknown: usize,
    /// Approximate CPU sum over processes with a reading; `None` if none.
    pub sub_cpu: Option<f32>,
}

pub struct ProcessView {
    pub filter: String,
    pub sort: SortKey,
    pub reverse: bool,
    /// Indexes into the snapshot's process list, in display order.
    pub rows: Vec<usize>,
    pub selected: Option<ProcessId>,
    pub cursor: usize,
    pub table: TableState,
    pub scroll: usize,
    /// Show only members of this process family (set by Dev-tab drill-down).
    pub family: Option<(std::sync::Arc<crate::dev::Families>, usize)>,
    /// Show parents with their children (disabled while a filter or family is active).
    pub tree: bool,
    pub collapsed: HashSet<ProcessId>,
    pub tree_rows: Vec<TreeRow>,
    pub detail_open: bool,
    pub reveal_cmd: bool,
    /// Rows visible in the last render (page size for PgUp/PgDn).
    pub page: usize,
    pub hist_id: Option<ProcessId>,
    pub hist_cpu: History,
    pub hist_rss: History,
    hist_cap: usize,
}

fn natural_desc(k: SortKey) -> bool {
    matches!(k, SortKey::Cpu | SortKey::Memory)
}

impl ProcessView {
    pub fn new(sort: SortKey, hist_cap: usize) -> Self {
        Self {
            filter: String::new(),
            sort,
            reverse: false,
            rows: Vec::new(),
            selected: None,
            cursor: 0,
            table: TableState::default(),
            scroll: 0,
            family: None,
            tree: false,
            collapsed: HashSet::new(),
            tree_rows: Vec::new(),
            detail_open: false,
            reveal_cmd: false,
            page: 10,
            hist_id: None,
            hist_cpu: History::new(hist_cap),
            hist_rss: History::new(hist_cap),
            hist_cap,
        }
    }

    pub fn sort_label(&self) -> &'static str {
        match self.sort {
            SortKey::Cpu => "CPU",
            SortKey::Memory => "MEM",
            SortKey::Name => "NAME",
            SortKey::Pid => "PID",
        }
    }

    /// Ascending (`^`) or descending (`v`) as actually applied.
    pub fn descending(&self) -> bool {
        natural_desc(self.sort) != self.reverse
    }

    fn matches(&self, p: &ProcessSample) -> bool {
        if let Some((f, i)) = &self.family {
            if f.family_of(&p.name) != Some(*i) {
                return false;
            }
        }
        if self.filter.is_empty() {
            return true;
        }
        let f = self.filter.to_lowercase();
        p.name.to_lowercase().contains(&f) || p.id.pid.to_string().contains(&f)
    }

    fn compare(&self, procs: &[ProcessSample], a: usize, b: usize) -> Ordering {
        let (pa, pb) = (&procs[a], &procs[b]);
        let ord = match self.sort {
            // Sort-only key: unknown CPU sorts below any real reading.
            SortKey::Cpu => pa
                .cpu
                .unwrap_or(-1.0)
                .partial_cmp(&pb.cpu.unwrap_or(-1.0))
                .unwrap_or(Ordering::Equal),
            SortKey::Memory => pa.rss.cmp(&pb.rss),
            SortKey::Name => pa.name.to_lowercase().cmp(&pb.name.to_lowercase()),
            SortKey::Pid => pa.id.pid.cmp(&pb.id.pid),
        };
        let ord = if self.descending() {
            ord.reverse()
        } else {
            ord
        };
        ord.then(pa.id.pid.cmp(&pb.id.pid)) // deterministic tie-break
    }

    /// True when rows are shown as a tree right now.
    pub fn tree_active(&self) -> bool {
        self.tree && self.filter.is_empty() && self.family.is_none()
    }

    /// Order `procs` as a forest (siblings sorted by the current key) and compute
    /// subtree totals. A parent is only accepted if it did not start AFTER its
    /// child (guards against PID reuse), and cycles cannot hang or drop rows.
    fn build_tree(&mut self, procs: &[ProcessSample]) {
        let n = procs.len();
        let mut by_pid: HashMap<u32, usize> = HashMap::with_capacity(n);
        for (i, p) in procs.iter().enumerate() {
            by_pid.entry(p.id.pid).or_insert(i);
        }
        let mut kids: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut roots: Vec<usize> = Vec::new();
        for (i, p) in procs.iter().enumerate() {
            let parent = p
                .parent
                .filter(|pp| *pp != p.id.pid)
                .and_then(|pp| by_pid.get(&pp).copied())
                .filter(|&pi| match (procs[pi].id.started_at, p.id.started_at) {
                    (Some(a), Some(b)) => a <= b,
                    _ => true,
                });
            match parent {
                Some(pi) => kids[pi].push(i),
                None => roots.push(i),
            }
        }
        let sorter =
            |v: &mut Vec<usize>, this: &Self| v.sort_by(|&a, &b| this.compare(procs, a, b));
        sorter(&mut roots, self);
        for k in kids.iter_mut() {
            sorter(k, self);
        }

        // Pre-order walk of the whole forest (iterative: no recursion limit).
        let mut visited = vec![false; n];
        let mut tparent: Vec<Option<usize>> = vec![None; n];
        let mut pre: Vec<(usize, u16)> = Vec::with_capacity(n);
        let walk = |start: usize,
                    pre: &mut Vec<(usize, u16)>,
                    visited: &mut Vec<bool>,
                    tparent: &mut Vec<Option<usize>>| {
            let mut stack = vec![(start, 0u16, None::<usize>)];
            while let Some((i, d, par)) = stack.pop() {
                if visited[i] {
                    continue;
                }
                visited[i] = true;
                tparent[i] = par;
                pre.push((i, d));
                for &c in kids[i].iter().rev() {
                    if !visited[c] {
                        stack.push((c, d.saturating_add(1), Some(i)));
                    }
                }
            }
        };
        for &r in &roots {
            walk(r, &mut pre, &mut visited, &mut tparent);
        }
        for i in 0..n {
            if !visited[i] {
                walk(i, &mut pre, &mut visited, &mut tparent); // members of a parent cycle
            }
        }

        // Subtree sums, children before parents.
        let mut sub_count = vec![1usize; n];
        let mut sub_rss: Vec<u64> = procs.iter().map(|p| p.rss.unwrap_or(0)).collect();
        let mut sub_unknown: Vec<usize> =
            procs.iter().map(|p| usize::from(p.rss.is_none())).collect();
        let mut sub_cpu: Vec<Option<f32>> = procs.iter().map(|p| p.cpu).collect();
        for &(i, _) in pre.iter().rev() {
            if let Some(p) = tparent[i] {
                sub_count[p] += sub_count[i];
                sub_rss[p] += sub_rss[i];
                sub_unknown[p] += sub_unknown[i];
                sub_cpu[p] = match (sub_cpu[p], sub_cpu[i]) {
                    (Some(a), Some(b)) => Some(a + b),
                    (a, b) => a.or(b),
                };
            }
        }

        // Visible rows: skip everything under a collapsed node.
        let mut rows = Vec::with_capacity(n);
        let mut info = Vec::with_capacity(n);
        let mut skip_below: Option<u16> = None;
        for &(i, d) in &pre {
            if let Some(sd) = skip_below {
                if d > sd {
                    continue;
                }
                skip_below = None;
            }
            let nk = kids[i].iter().filter(|&&c| tparent[c] == Some(i)).count();
            let collapsed = nk > 0 && self.collapsed.contains(&procs[i].id);
            if collapsed {
                skip_below = Some(d);
            }
            rows.push(i);
            info.push(TreeRow {
                depth: d,
                kids: nk,
                collapsed,
                sub_count: sub_count[i],
                sub_rss: sub_rss[i],
                sub_rss_unknown: sub_unknown[i],
                sub_cpu: sub_cpu[i],
            });
        }
        self.rows = rows;
        self.tree_rows = info;
    }

    pub fn toggle_tree(&mut self, procs: &[ProcessSample]) {
        self.tree = !self.tree;
        self.rebuild(procs);
    }

    /// Collapse the selected node, or if it cannot collapse, move to its parent row.
    pub fn tree_left(&mut self, procs: &[ProcessSample]) {
        if !self.tree_active() {
            return;
        }
        let Some(tr) = self.tree_rows.get(self.cursor).cloned() else {
            return;
        };
        if tr.kids > 0 && !tr.collapsed {
            if let Some(id) = self.selected {
                self.collapsed.insert(id);
                self.rebuild(procs);
            }
        } else if tr.depth > 0 {
            if let Some(pos) = (0..self.cursor)
                .rev()
                .find(|&r| self.tree_rows[r].depth < tr.depth)
            {
                self.move_to(pos, procs);
            }
        }
    }

    /// Expand the selected node, or if already expanded, move to its first child.
    pub fn tree_right(&mut self, procs: &[ProcessSample]) {
        if !self.tree_active() {
            return;
        }
        let Some(tr) = self.tree_rows.get(self.cursor).cloned() else {
            return;
        };
        if tr.collapsed {
            if let Some(id) = self.selected {
                self.collapsed.remove(&id);
                self.rebuild(procs);
            }
        } else if tr.kids > 0 {
            self.move_to(self.cursor + 1, procs);
        }
    }

    /// Recompute rows after a new snapshot / filter / sort change and keep the
    /// selection on the same process whenever it is still visible.
    pub fn rebuild(&mut self, procs: &[ProcessSample]) {
        let mut rows: Vec<usize> = (0..procs.len())
            .filter(|&i| self.matches(&procs[i]))
            .collect();
        rows.sort_by(|&a, &b| self.compare(procs, a, b));
        self.rows = rows;
        self.tree_rows.clear();
        if self.tree_active() {
            self.build_tree(procs);
        }

        let found = self
            .selected
            .and_then(|id| self.rows.iter().position(|&i| procs[i].id == id));
        match found {
            Some(pos) => self.cursor = pos,
            None if self.rows.is_empty() => {
                self.selected = None;
                self.cursor = 0;
            }
            None => {
                // Selected process exited / was filtered out: stay near the same
                // visual position instead of jumping to the top.
                self.cursor = self.cursor.min(self.rows.len() - 1);
                self.selected = Some(procs[self.rows[self.cursor]].id);
            }
        }
        self.table.select(if self.rows.is_empty() {
            None
        } else {
            Some(self.cursor)
        });
    }

    pub fn move_to(&mut self, idx: usize, procs: &[ProcessSample]) {
        if self.rows.is_empty() {
            return;
        }
        self.cursor = idx.min(self.rows.len() - 1);
        self.selected = Some(procs[self.rows[self.cursor]].id);
        self.table.select(Some(self.cursor));
    }

    pub fn move_by(&mut self, delta: isize, procs: &[ProcessSample]) {
        let target = (self.cursor as isize + delta).clamp(0, self.rows.len().max(1) as isize - 1);
        self.move_to(target as usize, procs);
    }

    pub fn selected_sample<'a>(&self, procs: &'a [ProcessSample]) -> Option<&'a ProcessSample> {
        let id = self.selected?;
        self.rows
            .get(self.cursor)
            .map(|&i| &procs[i])
            .filter(|p| p.id == id)
    }

    pub fn cycle_sort(&mut self, procs: &[ProcessSample]) {
        self.sort = match self.sort {
            SortKey::Cpu => SortKey::Memory,
            SortKey::Memory => SortKey::Name,
            SortKey::Name => SortKey::Pid,
            SortKey::Pid => SortKey::Cpu,
        };
        self.reverse = false;
        self.rebuild(procs);
    }

    pub fn toggle_reverse(&mut self, procs: &[ProcessSample]) {
        self.reverse = !self.reverse;
        self.rebuild(procs);
    }

    /// Record history for the selected process; resets when selection changes.
    pub fn record_history(&mut self, procs: &[ProcessSample]) {
        if self.hist_id != self.selected {
            self.hist_id = self.selected;
            self.hist_cpu = History::new(self.hist_cap);
            self.hist_rss = History::new(self.hist_cap);
        }
        if let Some(p) = self.selected_sample(procs) {
            if let Some(c) = p.cpu {
                self.hist_cpu.push(c as f64);
            }
            if let Some(r) = p.rss {
                self.hist_rss.push(r as f64);
            }
        }
    }
}
