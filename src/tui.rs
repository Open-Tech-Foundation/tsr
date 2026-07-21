//! `tsr --config` — a terminal UI for editing `tasks.toml` (SPEC §1.5, the
//! "TUI-primary, hand-edit-safe" principle).
//!
//! The TUI is the intended way to author tasks with all their options, instead
//! of hand-editing TOML. Edits go through the `toml_edit` document, so comments
//! and unknown keys in an existing file survive a round-trip, and every change is
//! validated (`config::validate_str`) before it is committed in memory or written
//! to disk.

use std::fs;
use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value};

use crate::config;
use crate::error::{Result, TsrError};

// Form field indices (fixed order).
const F_NAME: usize = 0;
const F_TYPE: usize = 1;
const F_RUN: usize = 2;
const F_DBIN: usize = 3;
const F_DARGS: usize = 4;
const F_LOC: usize = 5;
const F_DIR: usize = 6;
const F_PKGS: usize = 7;
const F_DEPS: usize = 8;
const F_ARGS: usize = 9;
const F_PARALLEL: usize = 10;
const F_ENV: usize = 11;

const ACCENT: Color = Color::Green;

/// Launch the config TUI against `path` (an existing `tasks.toml` or a new file
/// to be created there). Parse errors in an existing file surface as a config
/// error (exit `64`) before the UI starts.
pub fn run(path: &Path) -> Result<()> {
    let doc = if path.is_file() {
        let text = fs::read_to_string(path)
            .map_err(|e| TsrError::config(format!("cannot read '{}': {e}", path.display())))?;
        text.parse::<DocumentMut>()
            .map_err(|e| TsrError::config(format!("invalid TOML in '{}': {e}", path.display())))?
    } else {
        DocumentMut::new()
    };

    let mut app = App::new(path.to_path_buf(), doc);
    let mut terminal = ratatui::init();
    let result = app.run_loop(&mut terminal);
    ratatui::restore();
    result
}

/// UI mode.
enum Mode {
    List,
    Form(FormState),
    ConfirmQuit,
}

/// The application state.
struct App {
    path: PathBuf,
    doc: DocumentMut,
    tasks: Vec<String>,
    list: ListState,
    mode: Mode,
    status: String,
    dirty: bool,
    quit: bool,
}

impl App {
    fn new(path: PathBuf, doc: DocumentMut) -> App {
        let tasks = task_keys(&doc);
        let mut list = ListState::default();
        list.select(Some(0));
        App {
            path,
            doc,
            tasks,
            list,
            mode: Mode::List,
            status: String::new(),
            dirty: false,
            quit: false,
        }
    }

    fn run_loop(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.quit {
            terminal
                .draw(|frame| self.render(frame))
                .map_err(|e| TsrError::runtime(e.to_string()))?;
            match event::read().map_err(|e| TsrError::runtime(e.to_string()))? {
                Event::Key(key) if key.kind == KeyEventKind::Press => self.on_key(key),
                _ => {}
            }
        }
        Ok(())
    }

    fn refresh_tasks(&mut self) {
        self.tasks = task_keys(&self.doc);
        let sel = self.list.selected().unwrap_or(0);
        self.list
            .select(Some(sel.min(self.tasks.len().saturating_sub(1))));
    }

    fn save_file(&mut self) {
        match fs::write(&self.path, self.doc.to_string()) {
            Ok(()) => {
                self.dirty = false;
                self.status = format!("saved {}", self.path.display());
            }
            Err(e) => self.status = format!("write failed: {e}"),
        }
    }

    // --- input ---

    fn on_key(&mut self, key: ratatui::crossterm::event::KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match &mut self.mode {
            Mode::List => self.on_key_list(key.code, ctrl),
            Mode::Form(_) => self.on_key_form(key.code, ctrl),
            Mode::ConfirmQuit => self.on_key_confirm(key.code),
        }
    }

    fn on_key_list(&mut self, code: KeyCode, ctrl: bool) {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => {
                if self.dirty {
                    self.mode = Mode::ConfirmQuit;
                } else {
                    self.quit = true;
                }
            }
            KeyCode::Char('s') if ctrl => self.save_file(),
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1),
            KeyCode::Char('a') => {
                self.status.clear();
                self.mode = Mode::Form(FormState::new_task());
            }
            KeyCode::Enter | KeyCode::Char('e') => {
                if let Some(key) = self.selected_task() {
                    self.status.clear();
                    self.mode = Mode::Form(FormState::from_doc(&self.doc, &key));
                }
            }
            KeyCode::Char('d') => {
                if let Some(key) = self.selected_task()
                    && let Some(tasks) = self.doc.get_mut("tasks").and_then(Item::as_table_mut)
                {
                    tasks.remove(&key);
                    self.dirty = true;
                    self.status = format!("removed '{key}' (unsaved)");
                    self.refresh_tasks();
                }
            }
            _ => {}
        }
    }

    fn on_key_form(&mut self, code: KeyCode, ctrl: bool) {
        // Extract the form; put it back at the end (avoids borrow tangles).
        let Mode::Form(mut form) = std::mem::replace(&mut self.mode, Mode::List) else {
            return;
        };
        match code {
            KeyCode::Esc => {
                self.status = "edit cancelled".into();
                return; // mode already reset to List
            }
            KeyCode::Char('s') if ctrl => {
                match self.commit_form(&form) {
                    Ok(name) => {
                        self.status =
                            format!("task '{name}' updated (unsaved — ^S in list writes)");
                        return; // back to List
                    }
                    Err(e) => form.error = Some(e),
                }
            }
            KeyCode::Up => form.focus_prev(),
            KeyCode::Down | KeyCode::Tab => form.focus_next(),
            KeyCode::Left => form.adjust(false),
            KeyCode::Right => form.adjust(true),
            KeyCode::Char(' ') if !form.focus_is_text() => form.adjust(true),
            KeyCode::Backspace => form.backspace(),
            KeyCode::Char(c) if !ctrl => form.type_char(c),
            _ => {}
        }
        self.mode = Mode::Form(form);
    }

    fn on_key_confirm(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('y') => {
                self.save_file();
                self.quit = true;
            }
            KeyCode::Char('n') => self.quit = true,
            KeyCode::Esc | KeyCode::Char('c') => self.mode = Mode::List,
            _ => {}
        }
    }

    /// Validate `form` against a clone of the document; commit only if the whole
    /// resulting config still validates.
    fn commit_form(&mut self, form: &FormState) -> std::result::Result<String, String> {
        let mut candidate = self.doc.clone();
        let name = apply_form(&mut candidate, form)?;
        config::validate_str(&candidate.to_string()).map_err(|e| strip_banner(&e))?;
        self.doc = candidate;
        self.dirty = true;
        self.refresh_tasks();
        Ok(name)
    }

    fn move_sel(&mut self, delta: i32) {
        if self.tasks.is_empty() {
            return;
        }
        let n = self.tasks.len() as i32;
        let cur = self.list.selected().unwrap_or(0) as i32;
        self.list.select(Some((cur + delta).rem_euclid(n) as usize));
    }

    fn selected_task(&self) -> Option<String> {
        self.list
            .selected()
            .and_then(|i| self.tasks.get(i))
            .cloned()
    }

    // --- rendering ---

    fn render(&mut self, frame: &mut Frame) {
        let chunks = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(frame.area());

        let title = Line::from(vec![
            Span::styled(" tsr ", Style::default().fg(Color::Black).bg(ACCENT).bold()),
            Span::raw(" config  "),
            Span::styled(
                self.path.display().to_string(),
                Style::default().fg(Color::DarkGray),
            ),
            if self.dirty {
                Span::styled("  ●", Style::default().fg(Color::Yellow))
            } else {
                Span::raw("")
            },
        ]);
        frame.render_widget(Paragraph::new(title), chunks[0]);

        match &self.mode {
            Mode::List => self.render_list(frame, chunks[1]),
            Mode::Form(form) => render_form(frame, chunks[1], form),
            Mode::ConfirmQuit => render_confirm(frame, chunks[1]),
        }

        let help = match &self.mode {
            Mode::List => "↑↓ move · a add · e/⏎ edit · d delete · ^S save file · q quit",
            Mode::Form(_) => {
                "↑↓/Tab field · ←→/Space change · type to edit · ^S apply · Esc cancel"
            }
            Mode::ConfirmQuit => "unsaved changes — y save & quit · n discard · Esc cancel",
        };
        let status = if self.status.is_empty() {
            Span::styled(help, Style::default().fg(Color::DarkGray))
        } else {
            Span::styled(
                format!("{help}    {}", self.status),
                Style::default().fg(Color::DarkGray),
            )
        };
        frame.render_widget(Paragraph::new(Line::from(status)), chunks[2]);
    }

    fn render_list(&mut self, frame: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = if self.tasks.is_empty() {
            vec![ListItem::new(Line::from(Span::styled(
                "  (no tasks yet — press 'a' to add one)",
                Style::default().fg(Color::DarkGray),
            )))]
        } else {
            self.tasks
                .iter()
                .map(|k| {
                    let desc = describe(&self.doc, k);
                    ListItem::new(Line::from(vec![
                        Span::styled(format!("{k:<18}"), Style::default().fg(ACCENT)),
                        Span::styled(desc, Style::default().fg(Color::Gray)),
                    ]))
                })
                .collect()
        };
        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title(" tasks "))
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
            .highlight_symbol("▌ ");
        frame.render_stateful_widget(list, area, &mut self.list);
    }
}

fn render_confirm(frame: &mut Frame, area: Rect) {
    let text = Paragraph::new(vec![
        Line::from(""),
        Line::from("  You have unsaved changes.").yellow(),
        Line::from(""),
        Line::from("  y  save and quit"),
        Line::from("  n  discard and quit"),
        Line::from("  Esc  keep editing"),
    ])
    .block(Block::default().borders(Borders::ALL).title(" quit "));
    frame.render_widget(text, area);
}

fn render_form(frame: &mut Frame, area: Rect, form: &FormState) {
    let mut lines: Vec<Line> = Vec::new();
    let title = match &form.original {
        Some(k) => format!(" edit task: {k} "),
        None => " new task ".to_string(),
    };
    for (i, field) in form.fields.iter().enumerate() {
        let active = form.is_active(i);
        let focused = form.focus == i;
        let label = format!("{:>12}", field.label);
        let label_style = if focused {
            Style::default().fg(ACCENT).bold()
        } else if active {
            Style::default().fg(Color::Gray)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let value_span = match &field.kind {
            FieldKind::Text(s) => {
                let shown = if focused {
                    format!("{s}█")
                } else if s.is_empty() {
                    "—".to_string()
                } else {
                    s.clone()
                };
                Span::styled(shown, value_style(active, focused))
            }
            FieldKind::Toggle(b) => Span::styled(
                if *b { "[x] true" } else { "[ ] false" }.to_string(),
                value_style(active, focused),
            ),
            FieldKind::Choice { options, idx } => Span::styled(
                format!("‹ {} ›", options[*idx]),
                value_style(active, focused),
            ),
        };
        let mut spans = vec![
            Span::styled(label, label_style),
            Span::raw("  "),
            value_span,
        ];
        if let Some(hint) = field_hint(i) {
            spans.push(Span::styled(
                format!("   {hint}"),
                Style::default().fg(Color::DarkGray),
            ));
        }
        lines.push(Line::from(spans));
    }
    if let Some(err) = &form.error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  ✗ {err}"),
            Style::default().fg(Color::Red),
        )));
    }
    let para = Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(para, area);
}

fn value_style(active: bool, focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::White).bold()
    } else if active {
        Style::default().fg(Color::Gray)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

/// A per-field hint shown only when it clarifies the format.
fn field_hint(i: usize) -> Option<&'static str> {
    match i {
        F_DARGS | F_ARGS => Some("space-separated"),
        F_PKGS | F_DEPS => Some("comma-separated"),
        F_ENV => Some("KEY=VALUE, comma-separated"),
        _ => None,
    }
}

// --- form model ---

struct FormState {
    /// The existing key when editing; `None` when adding.
    original: Option<String>,
    fields: Vec<Field>,
    focus: usize,
    error: Option<String>,
}

struct Field {
    label: &'static str,
    kind: FieldKind,
}

enum FieldKind {
    Text(String),
    Toggle(bool),
    Choice {
        options: Vec<&'static str>,
        idx: usize,
    },
}

impl FormState {
    fn blank_fields() -> Vec<Field> {
        let t = |label| Field {
            label,
            kind: FieldKind::Text(String::new()),
        };
        vec![
            t("name"),
            Field {
                label: "type",
                kind: FieldKind::Choice {
                    options: vec!["run", "delegate", "delegate(table)", "auto-detect"],
                    idx: 0,
                },
            },
            t("run"),
            t("delegate bin"),
            t("delegate args"),
            Field {
                label: "location",
                kind: FieldKind::Choice {
                    options: vec!["root", "dir", "packages"],
                    idx: 0,
                },
            },
            t("dir"),
            t("packages"),
            t("deps"),
            t("args"),
            Field {
                label: "parallel",
                kind: FieldKind::Toggle(false),
            },
            t("env"),
        ]
    }

    fn new_task() -> FormState {
        FormState {
            original: None,
            fields: Self::blank_fields(),
            focus: 0,
            error: None,
        }
    }

    /// Populate a form from an existing task table in the document.
    fn from_doc(doc: &DocumentMut, key: &str) -> FormState {
        let mut form = FormState::new_task();
        form.original = Some(key.to_string());
        form.set_text(F_NAME, key);

        let Some(t) = doc
            .get("tasks")
            .and_then(|x| x.get(key))
            .and_then(Item::as_table_like)
        else {
            return form;
        };

        if let Some(run) = t.get("run").and_then(Item::as_str) {
            form.set_choice(F_TYPE, 0);
            form.set_text(F_RUN, run);
        } else if let Some(del) = t.get("delegate") {
            if let Some(s) = del.as_str() {
                form.set_choice(F_TYPE, 1);
                form.set_text(F_DBIN, s);
            } else if let Some(tbl) = del.as_table_like() {
                form.set_choice(F_TYPE, 2);
                if let Some(bin) = tbl.get("bin").and_then(Item::as_str) {
                    form.set_text(F_DBIN, bin);
                }
                form.set_text(F_DARGS, &join_ws(tbl.get("args")));
            }
        } else {
            form.set_choice(F_TYPE, 3); // auto-detect
        }

        if let Some(dir) = t.get("dir").and_then(Item::as_str) {
            form.set_choice(F_LOC, 1);
            form.set_text(F_DIR, dir);
        } else if t.get("packages").is_some() {
            form.set_choice(F_LOC, 2);
            form.set_text(F_PKGS, &join_csv(t.get("packages")));
        }

        form.set_text(F_DEPS, &join_csv(t.get("deps")));
        form.set_text(F_ARGS, &join_ws(t.get("args")));
        if let Some(b) = t.get("parallel").and_then(Item::as_bool) {
            form.set_toggle(F_PARALLEL, b);
        }
        form.set_text(F_ENV, &join_env(t.get("env")));
        form
    }

    // field accessors
    fn text(&self, i: usize) -> &str {
        match &self.fields[i].kind {
            FieldKind::Text(s) => s,
            _ => "",
        }
    }
    fn choice(&self, i: usize) -> usize {
        match &self.fields[i].kind {
            FieldKind::Choice { idx, .. } => *idx,
            _ => 0,
        }
    }
    fn toggle(&self, i: usize) -> bool {
        match &self.fields[i].kind {
            FieldKind::Toggle(b) => *b,
            _ => false,
        }
    }
    fn set_text(&mut self, i: usize, v: &str) {
        if let FieldKind::Text(s) = &mut self.fields[i].kind {
            *s = v.to_string();
        }
    }
    fn set_choice(&mut self, i: usize, v: usize) {
        if let FieldKind::Choice { idx, .. } = &mut self.fields[i].kind {
            *idx = v;
        }
    }
    fn set_toggle(&mut self, i: usize, v: bool) {
        if let FieldKind::Toggle(b) = &mut self.fields[i].kind {
            *b = v;
        }
    }

    /// Whether a field applies given the current type/location choices.
    fn is_active(&self, i: usize) -> bool {
        match i {
            F_RUN => self.choice(F_TYPE) == 0,
            F_DBIN => matches!(self.choice(F_TYPE), 1 | 2),
            F_DARGS => self.choice(F_TYPE) == 2,
            F_DIR => self.choice(F_LOC) == 1,
            F_PKGS => self.choice(F_LOC) == 2,
            _ => true,
        }
    }

    fn focus_is_text(&self) -> bool {
        matches!(self.fields[self.focus].kind, FieldKind::Text(_))
    }

    fn focus_next(&mut self) {
        let n = self.fields.len();
        for step in 1..=n {
            let i = (self.focus + step) % n;
            if self.is_active(i) {
                self.focus = i;
                break;
            }
        }
    }
    fn focus_prev(&mut self) {
        let n = self.fields.len();
        for step in 1..=n {
            let i = (self.focus + n - step) % n;
            if self.is_active(i) {
                self.focus = i;
                break;
            }
        }
    }

    fn type_char(&mut self, c: char) {
        if let FieldKind::Text(s) = &mut self.fields[self.focus].kind {
            s.push(c);
        }
    }
    fn backspace(&mut self) {
        if let FieldKind::Text(s) = &mut self.fields[self.focus].kind {
            s.pop();
        }
    }
    /// Adjust a Choice/Toggle field (`forward` cycles/enables).
    fn adjust(&mut self, forward: bool) {
        match &mut self.fields[self.focus].kind {
            FieldKind::Toggle(b) => *b = !*b,
            FieldKind::Choice { options, idx } => {
                let n = options.len();
                *idx = if forward {
                    (*idx + 1) % n
                } else {
                    (*idx + n - 1) % n
                };
            }
            FieldKind::Text(_) => {}
        }
    }
}

/// Apply a form to `doc`, returning the task name. Validates field consistency;
/// full config validation is the caller's job.
fn apply_form(doc: &mut DocumentMut, form: &FormState) -> std::result::Result<String, String> {
    let (name, table) = build_task_table(form)?;

    if doc.get("tasks").and_then(Item::as_table).is_none() {
        let mut parent = Table::new();
        parent.set_implicit(true);
        doc["tasks"] = Item::Table(parent);
    }
    let tasks = doc["tasks"]
        .as_table_mut()
        .ok_or_else(|| "`tasks` is not a table".to_string())?;
    tasks.set_implicit(true);

    // On rename, drop the old key.
    if let Some(old) = &form.original
        && old != &name
    {
        tasks.remove(old);
    }
    tasks.insert(&name, Item::Table(table));
    Ok(name)
}

/// Serialize a form into a task name and a `toml_edit` table.
fn build_task_table(form: &FormState) -> std::result::Result<(String, Table), String> {
    let name = form.text(F_NAME).trim().to_string();
    if name.is_empty() {
        return Err("name is required".into());
    }
    config::validate_task_name(&name).map_err(|e| strip_banner(&e))?;

    let mut t = Table::new();
    match form.choice(F_TYPE) {
        0 => {
            let run = form.text(F_RUN).trim();
            if run.is_empty() {
                return Err("run command is required".into());
            }
            t.insert("run", str_item(run));
        }
        1 => {
            let bin = form.text(F_DBIN).trim();
            if bin.is_empty() {
                return Err("delegate bin is required".into());
            }
            t.insert("delegate", str_item(bin));
        }
        2 => {
            let bin = form.text(F_DBIN).trim();
            if bin.is_empty() {
                return Err("delegate bin is required".into());
            }
            let mut it = InlineTable::new();
            it.insert("bin", Value::from(bin));
            let args = split_ws(form.text(F_DARGS));
            if !args.is_empty() {
                it.insert("args", Value::Array(to_array(&args)));
            }
            t.insert("delegate", Item::Value(Value::InlineTable(it)));
        }
        _ => {} // auto-detect: no run/delegate
    }

    match form.choice(F_LOC) {
        1 => {
            let dir = form.text(F_DIR).trim();
            if !dir.is_empty() {
                t.insert("dir", str_item(dir));
            }
        }
        2 => {
            let pkgs = split_csv(form.text(F_PKGS));
            if !pkgs.is_empty() {
                t.insert("packages", array_item(&pkgs));
            }
        }
        _ => {}
    }

    let deps = split_csv(form.text(F_DEPS));
    if !deps.is_empty() {
        t.insert("deps", array_item(&deps));
    }
    let args = split_ws(form.text(F_ARGS));
    if !args.is_empty() {
        t.insert("args", array_item(&args));
    }
    if form.toggle(F_PARALLEL) {
        t.insert("parallel", Item::Value(Value::from(true)));
    }
    let env = parse_env(form.text(F_ENV))?;
    if !env.is_empty() {
        let mut it = InlineTable::new();
        for (k, v) in env {
            it.insert(&k, Value::from(v));
        }
        t.insert("env", Item::Value(Value::InlineTable(it)));
    }

    Ok((name, t))
}

// --- small toml/string helpers ---

fn str_item(s: &str) -> Item {
    Item::Value(Value::from(s))
}

fn to_array(items: &[String]) -> Array {
    let mut a = Array::new();
    for s in items {
        a.push(s.as_str());
    }
    a
}

fn array_item(items: &[String]) -> Item {
    Item::Value(Value::Array(to_array(items)))
}

fn split_csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(String::from)
        .collect()
}

fn split_ws(s: &str) -> Vec<String> {
    s.split_whitespace().map(String::from).collect()
}

fn parse_env(s: &str) -> std::result::Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for part in s.split(',') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        let (k, v) = p
            .split_once('=')
            .ok_or_else(|| format!("env entry '{p}' must be KEY=VALUE"))?;
        let k = k.trim();
        if k.is_empty() {
            return Err(format!("env entry '{p}' has an empty key"));
        }
        out.push((k.to_string(), v.trim().to_string()));
    }
    Ok(out)
}

fn join_csv(item: Option<&Item>) -> String {
    array_strings(item).join(", ")
}
fn join_ws(item: Option<&Item>) -> String {
    array_strings(item).join(" ")
}
fn array_strings(item: Option<&Item>) -> Vec<String> {
    item.and_then(Item::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}
fn join_env(item: Option<&Item>) -> String {
    let Some(tbl) = item.and_then(Item::as_table_like) else {
        return String::new();
    };
    tbl.iter()
        .filter_map(|(k, v)| v.as_str().map(|s| format!("{k}={s}")))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Task keys in sorted order.
fn task_keys(doc: &DocumentMut) -> Vec<String> {
    let mut keys: Vec<String> = doc
        .get("tasks")
        .and_then(Item::as_table_like)
        .map(|t| t.iter().map(|(k, _)| k.to_string()).collect())
        .unwrap_or_default();
    keys.sort();
    keys
}

/// A one-line descriptor of a task's form, for the list view.
fn describe(doc: &DocumentMut, key: &str) -> String {
    let Some(t) = doc
        .get("tasks")
        .and_then(|x| x.get(key))
        .and_then(Item::as_table_like)
    else {
        return String::new();
    };
    let mut parts = Vec::new();
    if let Some(run) = t.get("run").and_then(Item::as_str) {
        parts.push(format!("run: {run}"));
    } else if let Some(del) = t.get("delegate") {
        if let Some(s) = del.as_str() {
            parts.push(format!("delegate: {s}"));
        } else {
            parts.push("delegate (custom)".to_string());
        }
    }
    if let Some(pkgs) = t.get("packages") {
        parts.push(format!("packages: {}", join_csv(Some(pkgs))));
    }
    if t.get("deps").is_some() {
        parts.push(format!("deps: {}", join_csv(t.get("deps"))));
    }
    if t.get("parallel").and_then(Item::as_bool) == Some(true) {
        parts.push("parallel".to_string());
    }
    if parts.is_empty() {
        parts.push("auto".to_string());
    }
    parts.join("  ·  ")
}

/// Strip the `Display` banner from a `TsrError` for inline form messages.
fn strip_banner(e: &TsrError) -> String {
    let s = e.to_string();
    s.strip_prefix("✗ config error: ")
        .or_else(|| s.strip_prefix("✗ error: "))
        .map(str::to_string)
        .unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form_with(fields: &[(usize, &str)], choices: &[(usize, usize)]) -> FormState {
        let mut f = FormState::new_task();
        for (i, v) in fields {
            f.set_text(*i, v);
        }
        for (i, v) in choices {
            f.set_choice(*i, *v);
        }
        f
    }

    #[test]
    fn builds_run_task_and_validates() {
        let f = form_with(&[(F_NAME, "dev"), (F_RUN, "vite --host")], &[(F_TYPE, 0)]);
        let mut doc = DocumentMut::new();
        let name = apply_form(&mut doc, &f).unwrap();
        assert_eq!(name, "dev");
        config::validate_str(&doc.to_string()).unwrap();
        assert!(doc.to_string().contains("[tasks.dev]"));
        assert!(doc.to_string().contains("run = \"vite --host\""));
    }

    #[test]
    fn builds_delegate_table_with_args() {
        let f = form_with(
            &[
                (F_NAME, "bundle"),
                (F_DBIN, "make"),
                (F_DARGS, "bundle prod"),
            ],
            &[(F_TYPE, 2)],
        );
        let mut doc = DocumentMut::new();
        apply_form(&mut doc, &f).unwrap();
        let s = doc.to_string();
        assert!(
            s.contains("delegate = { bin = \"make\", args = [\"bundle\", \"prod\"] }"),
            "{s}"
        );
    }

    #[test]
    fn builds_packages_deps_parallel_env() {
        let mut f = form_with(
            &[
                (F_NAME, "test"),
                (F_PKGS, "apps/*, packages/ui"),
                (F_DEPS, "lint, build"),
                (F_ENV, "CI=true, LOG=debug"),
            ],
            &[(F_TYPE, 3), (F_LOC, 2)],
        );
        f.set_toggle(F_PARALLEL, true);
        let mut doc = DocumentMut::new();
        apply_form(&mut doc, &f).unwrap();
        config::validate_str(&doc.to_string()).unwrap();
        let s = doc.to_string();
        assert!(
            s.contains("packages = [\"apps/*\", \"packages/ui\"]"),
            "{s}"
        );
        assert!(s.contains("deps = [\"lint\", \"build\"]"), "{s}");
        assert!(s.contains("parallel = true"), "{s}");
        assert!(s.contains("CI = \"true\""), "{s}");
    }

    #[test]
    fn preserves_comments_and_other_tasks() {
        let src = "# top comment\n[tasks.keep]\nrun = \"echo hi\" # inline\n";
        let mut doc = src.parse::<DocumentMut>().unwrap();
        let f = form_with(&[(F_NAME, "added"), (F_RUN, "true")], &[(F_TYPE, 0)]);
        apply_form(&mut doc, &f).unwrap();
        let s = doc.to_string();
        assert!(s.contains("# top comment"));
        assert!(s.contains("# inline"));
        assert!(s.contains("[tasks.keep]") && s.contains("[tasks.added]"));
    }

    #[test]
    fn rejects_dir_and_packages_conflict_via_validation() {
        // Location choice makes dir/packages mutually exclusive by construction,
        // so a well-formed form always validates.
        let f = form_with(
            &[(F_NAME, "x"), (F_RUN, "true"), (F_DIR, "apps/web")],
            &[(F_TYPE, 0), (F_LOC, 1)],
        );
        let mut doc = DocumentMut::new();
        apply_form(&mut doc, &f).unwrap();
        assert!(config::validate_str(&doc.to_string()).is_ok());
    }

    #[test]
    fn rejects_bad_name() {
        let f = form_with(&[(F_NAME, "bad name"), (F_RUN, "true")], &[(F_TYPE, 0)]);
        let mut doc = DocumentMut::new();
        assert!(apply_form(&mut doc, &f).is_err());
    }

    #[test]
    fn round_trips_existing_task_into_form() {
        let src = "[tasks.\"web#build\"]\ndelegate = { bin = \"make\", args = [\"b\"] }\ndeps = [\"ui#build\"]\nparallel = true\n";
        let doc = src.parse::<DocumentMut>().unwrap();
        let form = FormState::from_doc(&doc, "web#build");
        assert_eq!(form.text(F_NAME), "web#build");
        assert_eq!(form.choice(F_TYPE), 2);
        assert_eq!(form.text(F_DBIN), "make");
        assert_eq!(form.text(F_DARGS), "b");
        assert_eq!(form.text(F_DEPS), "ui#build");
        assert!(form.toggle(F_PARALLEL));
    }
}
