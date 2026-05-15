//! Core App state + the `handle(event)` dispatcher.
//!
//! Mirrors Go's bubbletea `Model`: targets + selection + log buffers +
//! mode flags + the run-dir / project-root strings used by reload.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use tukituki_config::{RunTarget, expand_env, load_dotenv, load_targets};
use tukituki_state::Status;

use crate::event::AppEvent;
use crate::handle::ManagerHandle;
use crate::input;
use crate::rows::{Row, compute};

/// How many lines to keep in the TUI's own ring buffer per target.
/// Larger than the manager's 1000 — the TUI is happy to retain more
/// scrollback than headless callers need.
const TUI_RING: usize = 10_000;

pub struct App<H: ManagerHandle> {
    pub targets: Vec<RunTarget>,
    pub manager: Arc<H>,
    pub run_dir: PathBuf,
    pub project_root: PathBuf,

    pub selected: usize,
    pub rows: Vec<Row>,
    pub folder_expanded: BTreeMap<String, bool>,

    /// Per-target log line buffer with viewport offset + at-bottom flag.
    pub logs: HashMap<String, LogBuffer>,
    pub statuses: BTreeMap<String, Status>,

    pub status_msg: String,
    pub status_msg_until_tick: u8,

    pub quitting: bool,
    pub stop_all: bool,
    pub wrap_logs: bool,
    pub zoom_logs: bool,

    pub help_visible: bool,
    pub describe: Option<String>,

    pub last_height: u16,

    // Search state — mirrors the Go TUI's `/` flow. `search_matches`
    // holds the indices into the *currently-selected target's*
    // ring buffer that contain `search_query` (case-insensitive).
    // `search_match_idx` points at the active match within that list;
    // `nextSearchMatch` wraps around.
    pub search_mode: bool,
    pub search_query: String,
    pub search_matches: Vec<usize>,
    pub search_match_idx: usize,
}

pub struct LogBuffer {
    pub lines: VecDeque<String>,
    /// Number of lines scrolled above the visible window. 0 = top.
    pub scroll: usize,
    /// True when the viewport is following the tail; new lines should
    /// keep it pinned to the bottom.
    pub at_bottom: bool,
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self {
            lines: VecDeque::new(),
            scroll: 0,
            at_bottom: true,
        }
    }
}

impl LogBuffer {
    /// Append a line, evicting the oldest when the ring is full.
    /// Returns the number of front-of-buffer lines dropped (0 or 1) so
    /// callers can shift any line-index-keyed data structures (e.g.
    /// `App::search_matches`) along with the eviction.
    pub fn push(&mut self, line: String) -> usize {
        self.lines.push_back(line);
        if self.lines.len() > TUI_RING {
            self.lines.pop_front();
            1
        } else {
            0
        }
    }
}

pub struct Continuation {
    pub continue_loop: bool,
    pub stop_all: bool,
}

impl<H: ManagerHandle> App<H> {
    pub fn new(
        targets: Vec<RunTarget>,
        manager: Arc<H>,
        run_dir: PathBuf,
        project_root: PathBuf,
    ) -> Self {
        let folder_expanded = BTreeMap::new();
        let rows = compute(&targets, &folder_expanded);
        let statuses = manager.get_all_statuses();
        let mut logs = HashMap::new();
        for t in &targets {
            logs.insert(t.name.clone(), LogBuffer::default());
        }
        Self {
            targets,
            manager,
            run_dir,
            project_root,
            selected: 0,
            rows,
            folder_expanded,
            logs,
            statuses,
            status_msg: String::new(),
            status_msg_until_tick: 0,
            quitting: false,
            stop_all: false,
            wrap_logs: false,
            zoom_logs: false,
            help_visible: false,
            describe: None,
            last_height: 24,
            search_mode: false,
            search_query: String::new(),
            search_matches: Vec::new(),
            search_match_idx: 0,
        }
    }

    /// Tear down search state. Called by Esc inside search mode and
    /// whenever the user switches targets (matches the Go behaviour:
    /// matches are relative to the current target, so they're stale
    /// the moment selection moves).
    pub fn reset_search(&mut self) {
        self.search_mode = false;
        self.search_query.clear();
        self.search_matches.clear();
        self.search_match_idx = 0;
    }

    /// Recompute `search_matches` for the current query against the
    /// selected target's log buffer. Resets `search_match_idx` to 0 so
    /// the next "jump" goes to the top match.
    pub fn update_search_matches(&mut self) {
        self.search_matches.clear();
        self.search_match_idx = 0;
        if self.search_query.is_empty() {
            return;
        }
        let Some(name) = self.selected_target_name() else {
            return;
        };
        let Some(buf) = self.logs.get(&name) else {
            return;
        };
        let q = self.search_query.to_lowercase();
        for (i, line) in buf.lines.iter().enumerate() {
            if line.to_lowercase().contains(&q) {
                self.search_matches.push(i);
            }
        }
    }

    /// Advance to the next match (wrapping). No-op when there are
    /// no matches.
    pub fn next_search_match(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        self.search_match_idx = (self.search_match_idx + 1) % self.search_matches.len();
        self.jump_to_current_match();
    }

    /// Position the log viewport so the current match line is visible
    /// near the middle. Uses `last_height` as an approximation; if the
    /// terminal is tiny the math degrades to "match at the bottom".
    pub fn jump_to_current_match(&mut self) {
        let Some(name) = self.selected_target_name() else {
            return;
        };
        let Some(match_line) = self.search_matches.get(self.search_match_idx).copied() else {
            return;
        };
        let Some(buf) = self.logs.get_mut(&name) else {
            return;
        };
        let total = buf.lines.len();
        // Center the match line in a (last_height - 4) tall window.
        // -4 accounts for header/title/borders. Saturates fine when
        // the viewport is small.
        let visible = (self.last_height as usize).saturating_sub(4).max(1);
        let half = visible / 2;
        // We want the line `match_line` to sit roughly in the middle of
        // the visible window. `scroll` counts lines hidden below the
        // bottom; `end = total - scroll`. Solve for scroll so that
        // `end ≈ match_line + half + 1`.
        let target_end = match_line.saturating_add(half + 1).min(total);
        buf.scroll = total.saturating_sub(target_end);
        buf.at_bottom = buf.scroll == 0;
    }

    /// Seed log buffers from the manager's ring buffer so the right
    /// pane isn't empty on first paint.
    pub fn backfill_logs(&mut self) {
        for t in &self.targets {
            let lines = self.manager.get_log_lines(&t.name);
            let buf = self.logs.entry(t.name.clone()).or_default();
            for l in lines {
                buf.push(l);
            }
        }
    }

    pub fn handle(&mut self, ev: AppEvent) -> Continuation {
        match ev {
            AppEvent::Key(k) => input::handle_key(self, k),
            AppEvent::Resize(_, h) => {
                self.last_height = h;
                Continuation::cont()
            }
            AppEvent::Tick => {
                self.statuses = self.manager.get_all_statuses();
                if self.status_msg_until_tick > 0 {
                    self.status_msg_until_tick -= 1;
                    if self.status_msg_until_tick == 0 {
                        self.status_msg.clear();
                    }
                }
                Continuation::cont()
            }
            AppEvent::LogLine { target, line } => {
                let is_selected = self.selected_target_name().as_deref() == Some(target.as_str());
                // Push first, then update search bookkeeping (if the
                // line belongs to the currently selected target). We
                // capture the new index *before* mutating
                // `search_matches` so any append below uses the right
                // line-index in the (possibly post-eviction) buffer.
                let (dropped, new_index) = {
                    let buf = self.logs.entry(target).or_default();
                    let dropped = buf.push(line.clone());
                    (dropped, buf.lines.len().saturating_sub(1))
                };

                if is_selected && self.search_mode && !self.search_query.is_empty() {
                    if dropped > 0 {
                        let mut adjusted = Vec::with_capacity(self.search_matches.len());
                        for idx in &self.search_matches {
                            if let Some(new) = idx.checked_sub(dropped) {
                                adjusted.push(new);
                            }
                        }
                        if self.search_match_idx >= adjusted.len() {
                            self.search_match_idx = 0;
                        }
                        self.search_matches = adjusted;
                    }
                    if line
                        .to_lowercase()
                        .contains(&self.search_query.to_lowercase())
                    {
                        self.search_matches.push(new_index);
                    }
                }
                Continuation::cont()
            }
            AppEvent::FileChange => {
                if let Ok(mut targets) = load_targets(&self.run_dir) {
                    let dotenv = load_dotenv(&self.project_root).ok().flatten();
                    targets = expand_env(targets, dotenv.as_ref());
                    self.manager.update_targets(targets.clone());
                    self.targets = targets;
                    self.rebuild_rows();
                    // Bring up the OTel collector if a newly-added
                    // target enabled it. Non-fatal on failure.
                    if let Err(e) = self.manager.ensure_otel_collector() {
                        self.flash(&format!("otel collector: {e}"));
                    } else {
                        self.flash("reloaded run files");
                    }
                }
                Continuation::cont()
            }
            AppEvent::ScrollLog(delta) => {
                self.scroll_log(delta);
                Continuation::cont()
            }
            AppEvent::EditorDone(_) => {
                // Force a re-render; the file change should also have
                // fired a FileChange so target state will refresh.
                Continuation::cont()
            }
        }
    }

    pub fn rebuild_rows(&mut self) {
        let new_rows = compute(&self.targets, &self.folder_expanded);
        // Preserve selection on the same target name if possible.
        let prev_target = self.selected_target_name();
        self.rows = new_rows;
        if let Some(name) = prev_target {
            for (i, r) in self.rows.iter().enumerate() {
                if let Row::Target { target_idx, .. } = r
                    && self.targets.get(*target_idx).map(|t| t.name.as_str()) == Some(name.as_str())
                {
                    self.selected = i;
                    return;
                }
            }
        }
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len().saturating_sub(1);
        }
    }

    pub fn selected_target_name(&self) -> Option<String> {
        match self.rows.get(self.selected)? {
            Row::Target { target_idx, .. } => self.targets.get(*target_idx).map(|t| t.name.clone()),
            Row::Folder { .. } => None,
        }
    }

    pub fn selected_target(&self) -> Option<&RunTarget> {
        match self.rows.get(self.selected)? {
            Row::Target { target_idx, .. } => self.targets.get(*target_idx),
            Row::Folder { .. } => None,
        }
    }

    pub fn flash(&mut self, msg: &str) {
        self.status_msg = msg.to_string();
        // 2 ticks ≈ 2s
        self.status_msg_until_tick = 2;
    }

    pub fn scroll_log(&mut self, delta: i32) {
        let Some(name) = self.selected_target_name() else {
            return;
        };
        let buf = self.logs.entry(name).or_default();
        if delta > 0 {
            // Scrolling down — toward newest. If we hit the bottom,
            // re-pin.
            let max = buf.lines.len();
            let new = (buf.scroll + delta as usize).min(max);
            buf.scroll = new;
            // at_bottom is reset on render once it knows window size.
        } else {
            let amt = (-delta) as usize;
            buf.scroll = buf.scroll.saturating_sub(amt);
            buf.at_bottom = false;
        }
    }
}

impl Continuation {
    pub fn cont() -> Self {
        Self {
            continue_loop: true,
            stop_all: false,
        }
    }
    pub fn detach() -> Self {
        Self {
            continue_loop: false,
            stop_all: false,
        }
    }
    pub fn kill_all() -> Self {
        Self {
            continue_loop: false,
            stop_all: true,
        }
    }
}
