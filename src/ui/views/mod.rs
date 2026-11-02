pub mod developer;
pub mod network;
pub mod overview;
pub mod processes;
pub mod services;
pub mod storage;

/// Keep `scroll` such that `cursor` is inside a window of `height` rows.
pub fn scroll_for(cursor: usize, scroll: usize, height: usize) -> usize {
    if height == 0 {
        return 0;
    }
    if cursor < scroll {
        cursor
    } else if cursor >= scroll + height {
        cursor + 1 - height
    } else {
        scroll
    }
}

/// MEM column text for a process.
pub fn mem_cell(p: &crate::domain::ProcessSample) -> String {
    crate::util::format::bytes_or_denied(p.rss)
}

/// CPU column text: `denied` when restricted, `…` while warming up.
pub fn cpu_cell(p: &crate::domain::ProcessSample, app: &crate::app::App) -> String {
    if p.restricted {
        "denied".to_string()
    } else {
        crate::util::format::percent_fine(p.cpu.map(|c| app.cpu_display(c)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_follows_cursor() {
        assert_eq!(scroll_for(0, 0, 5), 0);
        assert_eq!(scroll_for(7, 0, 5), 3);
        assert_eq!(scroll_for(2, 3, 5), 2);
        assert_eq!(scroll_for(5, 3, 5), 3);
        assert_eq!(scroll_for(9, 0, 0), 0);
    }
}
