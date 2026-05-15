//! Core App state + the `handle(event)` dispatcher.
//!
//! Mirrors Go's bubbletea `Model`: targets + selection + log buffers +
//! mode flags + the run-dir / project-root strings used by reload.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use ansi_to_tui::IntoText;
use ratatui::text::Line;
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

    /// Set to true by any handler that mutates user-visible state.
    /// The render loop reads + clears this flag — if no event has
    /// touched anything the user can see, we skip the entire render
    /// path including ratatui's frame diff and the stdout flush.
    /// Decoupling render rate from event rate is what keeps a project
    /// with many concurrent chatty targets (osewa-style) responsive
    /// when the user switches to a quiet one: LogLine events for
    /// non-selected targets buffer silently without triggering a
    /// repaint.
    dirty: bool,

    /// Subset of `dirty` for changes that should bypass the
    /// FRAME_BUDGET rate cap entirely — key presses, ticks where the
    /// status icons actually moved, file-change reloads, terminal
    /// resizes, external-editor exits. The cap exists to keep log-
    /// stream-driven renders below 60fps; it has no business
    /// delaying a key press the user just made by 16ms. Cleared by
    /// `take_urgent()` after the render fires.
    urgent: bool,

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
    /// Raw line text — kept for search matching (case-insensitive
    /// `contains`) and as a fallback when ANSI parsing fails.
    pub lines: VecDeque<String>,
    /// Pre-parsed `Line<'static>` for each entry in `lines`.
    ///
    /// We parse ANSI escape sequences once on append rather than on
    /// every render — chatty backends with structured (color-coded)
    /// logs would otherwise pay multi-millisecond `ansi-to-tui`
    /// costs at 60fps, which manifests as visible lag the moment the
    /// user switches targets. With parse-on-receive, rendering is
    /// just a slice + clone of pre-built `Line` objects.
    pub parsed: VecDeque<Line<'static>>,
    /// Number of newer lines hidden below the visible window.
    /// `scroll == 0` means the viewport is pinned to the bottom.
    pub scroll: usize,
    /// True when the viewport is following the tail; new lines should
    /// keep it pinned to the bottom.
    pub at_bottom: bool,
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self {
            lines: VecDeque::new(),
            parsed: VecDeque::new(),
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
        let parsed = parse_log_line(&line);
        self.lines.push_back(line);
        self.parsed.push_back(parsed);
        if self.lines.len() > TUI_RING {
            self.lines.pop_front();
            self.parsed.pop_front();
            1
        } else {
            0
        }
    }

    /// Clear both deques and reset scroll state.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.parsed.clear();
        self.scroll = 0;
        self.at_bottom = true;
    }
}

/// Convert a raw log line (possibly containing ANSI escape sequences)
/// into a styled `Line<'static>`. Owned-input variant of
/// `IntoText::into_text` so the resulting spans don't borrow from
/// `line`'s buffer — they end up in `Cow::Owned` form, suitable for
/// storage in our ring buffer.
fn parse_log_line(line: &str) -> Line<'static> {
    // Caller's invariant: incoming lines never contain `\n` (the
    // tailer splits on newlines before broadcasting). If that
    // changes, we still produce a single combined Line so the buffer
    // stays index-aligned with `lines`.
    let owned: String = line.to_string();
    match owned.into_text() {
        Ok(mut text) => {
            if text.lines.len() == 1 {
                text.lines.remove(0)
            } else if text.lines.is_empty() {
                Line::default()
            } else {
                // Defensive: a multi-line parse result gets flattened
                // back into one Line so `parsed` keeps the same
                // length as `lines`.
                let mut spans = Vec::new();
                for (i, l) in text.lines.into_iter().enumerate() {
                    if i > 0 {
                        spans.push(ratatui::text::Span::raw(" "));
                    }
                    spans.extend(l.spans);
                }
                Line::from(spans)
            }
        }
        Err(_) => Line::from(line.to_string()),
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
            // Start dirty + urgent so the first iteration paints the
            // initial frame without waiting on the rate cap.
            dirty: true,
            urgent: true,
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

    /// Has the App's user-visible state changed since the last
    /// `clear_dirty()`?
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Reset the dirty flag — called by the render loop right after
    /// a `terminal.draw`.
    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    /// Returns the current urgent flag and resets it. Used by the
    /// render loop to decide whether to bypass the FRAME_BUDGET cap.
    pub fn take_urgent(&mut self) -> bool {
        let was = self.urgent;
        self.urgent = false;
        was
    }

    pub fn handle(&mut self, ev: AppEvent) -> Continuation {
        match ev {
            AppEvent::Key(k) => {
                // Every key potentially changes visible state. Mark
                // dirty AND urgent so the render fires immediately —
                // the FRAME_BUDGET cap exists to coalesce log floods,
                // not to delay user input by 16ms.
                self.dirty = true;
                self.urgent = true;
                input::handle_key(self, k)
            }
            AppEvent::Resize(_, h) => {
                self.last_height = h;
                self.dirty = true;
                self.urgent = true;
                Continuation::cont()
            }
            AppEvent::Tick => {
                let prev_statuses = self.statuses.clone();
                self.statuses = self.manager.get_all_statuses();
                if self.status_msg_until_tick > 0 {
                    self.status_msg_until_tick -= 1;
                    if self.status_msg_until_tick == 0 {
                        self.status_msg.clear();
                    }
                    self.dirty = true;
                    self.urgent = true;
                }
                // Tick is also the canary for status icon changes —
                // mark dirty only when something actually moved so
                // 1Hz ticks don't force a render when nothing changed.
                if prev_statuses != self.statuses {
                    self.dirty = true;
                    self.urgent = true;
                }
                Continuation::cont()
            }
            AppEvent::LogLine { target, line } => {
                let is_selected = self.selected_target_name().as_deref() == Some(target.as_str());
                if is_selected {
                    // Only mark dirty when the line will actually
                    // appear on screen. LogLine events for non-
                    // selected targets buffer silently — this is the
                    // big win for projects with many concurrent
                    // chatty targets.
                    self.dirty = true;
                }
                // Push first, then update search bookkeeping (if the
                // line belongs to the currently selected target). We
                // capture the new index *before* mutating
                // `search_matches` so any append below uses the right
                // line-index in the (possibly post-eviction) buffer.
                let (dropped, new_index) = {
                    let buf = self.logs.entry(target).or_default();
                    let dropped = buf.push(line.clone());
                    // When the user is scrolled up reading older logs
                    // (at_bottom=false), bump `scroll` to keep their
                    // reading position stable as new lines arrive at
                    // the bottom. With `scroll` measured from the
                    // bottom, an appended line shifts that bottom
                    // forward by 1 — so without a compensating bump
                    // the viewport would drift "forward in history"
                    // every time a line lands. Ring-buffer eviction
                    // pulls the bottom-anchor back by 1, so the net
                    // adjustment is `1 - dropped`.
                    if !buf.at_bottom {
                        let adj = 1usize.saturating_sub(dropped);
                        if adj > 0 {
                            buf.scroll = (buf.scroll + adj).min(buf.lines.len());
                        }
                    }
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
                    self.dirty = true;
                    self.urgent = true;
                }
                Continuation::cont()
            }
            AppEvent::ScrollLog(delta) => {
                // Mouse-wheel scrolls stay rate-limited intentionally:
                // they're a high-frequency event class (a single
                // physical wheel tick can produce dozens of mpsc
                // events) that the FRAME_BUDGET was designed to
                // coalesce. Marking them urgent would defeat the cap
                // under a sustained scroll.
                self.scroll_log(delta);
                self.dirty = true;
                Continuation::cont()
            }
            AppEvent::EditorDone(_) => {
                // Force a re-render; the file change should also have
                // fired a FileChange so target state will refresh.
                self.dirty = true;
                self.urgent = true;
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
        // Nudge off a Separator if we landed on one (shouldn't
        // normally happen, but possible after a reload that removed
        // the previously-selected target).
        if let Some(r) = self.rows.get(self.selected)
            && !crate::rows::is_selectable(r)
        {
            for (i, r) in self.rows.iter().enumerate() {
                if crate::rows::is_selectable(r) {
                    self.selected = i;
                    break;
                }
            }
        }
    }

    pub fn selected_target_name(&self) -> Option<String> {
        match self.rows.get(self.selected)? {
            Row::Target { target_idx, .. } => self.targets.get(*target_idx).map(|t| t.name.clone()),
            Row::Folder { .. } | Row::Separator { .. } => None,
        }
    }

    pub fn selected_target(&self) -> Option<&RunTarget> {
        match self.rows.get(self.selected)? {
            Row::Target { target_idx, .. } => self.targets.get(*target_idx),
            Row::Folder { .. } | Row::Separator { .. } => None,
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
        // `buf.scroll` = number of newer lines hidden below the viewport.
        //   scroll=0 → viewport is pinned at the bottom (newest line).
        //   scroll=N → viewport's bottom edge is N lines back from newest.
        // So PgUp (`delta < 0`, scroll toward older) INCREASES scroll;
        // PgDn (`delta > 0`, scroll toward newer) DECREASES scroll.
        if delta < 0 {
            let amt = delta.unsigned_abs() as usize;
            // Cap so we can't scroll past the first line in the buffer.
            buf.scroll = (buf.scroll + amt).min(buf.lines.len());
            buf.at_bottom = false;
        } else if delta > 0 {
            buf.scroll = buf.scroll.saturating_sub(delta as usize);
            if buf.scroll == 0 {
                buf.at_bottom = true;
            }
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
