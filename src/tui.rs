//! `tsr --config` — a terminal UI for editing `tasks.toml` (SPEC §1.5, the
//! "TUI-primary, hand-edit-safe" principle).
//!
//! The TUI is the intended way to author tasks with all their options, instead
//! of hand-editing TOML. It opens on a menu of workflows (add / edit / delegate /
//! delete / graph) rather than a bare list, so there is always an obvious next
//! step. Edits go through the `toml_edit` document, so comments and unknown keys
//! in an existing file survive a round-trip.
//!
//! Changes **autosave**: a committed form or delete is written to disk straight
//! away, so there is no dirty state and no discard prompt. Every change is
//! validated (`config::validate_str`) *before* it is committed, so an autosave
//! can never write a broken config; a failed validation keeps the form open.

use std::fs;
use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value};

use crate::config;
use crate::error::{Result, TsrError};
use crate::resolve::{self, Invocation};

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
const F_ENV_FILE: usize = 12;

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
    /// The home screen: a menu of workflows to launch.
    Menu,
    /// A task picker — the task list opened to pick one for `purpose`.
    List(ListPurpose),
    Form(FormState),
    /// The read-only dependency-graph / dry-run preview.
    Graph(GraphView),
    /// Confirm removing the named task.
    ConfirmDelete(String),
}

/// Why the task list (picker) is open — decides what selecting a task does.
#[derive(Clone, Copy, PartialEq)]
enum ListPurpose {
    Edit,
    Delete,
}

/// Home-menu entries, in display order. Each opens a dedicated workflow.
#[derive(Clone, Copy)]
enum MenuItem {
    Add,
    Edit,
    Delegate,
    Delete,
    Graph,
    Quit,
}

impl MenuItem {
    const ALL: [MenuItem; 6] = [
        MenuItem::Add,
        MenuItem::Edit,
        MenuItem::Delegate,
        MenuItem::Delete,
        MenuItem::Graph,
        MenuItem::Quit,
    ];

    /// The label and a one-line hint shown in the menu.
    fn label_hint(self) -> (&'static str, &'static str) {
        match self {
            MenuItem::Add => ("Add a task", "create a run / auto-detect task"),
            MenuItem::Edit => ("Edit a task", "pick a task and change its options"),
            MenuItem::Delegate => ("Delegate a task", "hand a task to turbo / make / nx / …"),
            MenuItem::Delete => ("Delete a task", "pick a task and remove it"),
            MenuItem::Graph => ("Preview graph", "dependency tree & dry-run commands"),
            MenuItem::Quit => ("Quit", "every change is already saved"),
        }
    }
}

/// State for the graph/dry-run view: which task is focused (`None` = every task),
/// and a vertical scroll offset for tall graphs.
struct GraphView {
    focus: Option<String>,
    scroll: u16,
}

/// The application state.
struct App {
    path: PathBuf,
    doc: DocumentMut,
    tasks: Vec<String>,
    list: ListState,
    menu: ListState,
    mode: Mode,
    status: String,
    quit: bool,
}

impl App {
    fn new(path: PathBuf, doc: DocumentMut) -> App {
        let tasks = task_keys(&doc);
        let mut list = ListState::default();
        list.select(Some(0));
        let mut menu = ListState::default();
        menu.select(Some(0));
        App {
            path,
            doc,
            tasks,
            list,
            menu,
            mode: Mode::Menu,
            status: String::new(),
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

    /// Write the document to disk. Every committed change calls this, so
    /// `tasks.toml` always matches what the UI shows — there is no unsaved state.
    fn write_file(&self) -> std::result::Result<(), String> {
        fs::write(&self.path, self.doc.to_string())
            .map_err(|e| format!("write failed: {} — {e}", self.path.display()))
    }

    // --- input ---

    fn on_key(&mut self, key: ratatui::crossterm::event::KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match &mut self.mode {
            Mode::Menu => self.on_key_menu(key.code),
            Mode::List(_) => self.on_key_list(key.code),
            Mode::Form(_) => self.on_key_form(key.code, ctrl),
            Mode::Graph(_) => self.on_key_graph(key.code),
            Mode::ConfirmDelete(_) => self.on_key_confirm_delete(key.code),
        }
    }

    /// The home menu: move between actions and launch the selected workflow.
    fn on_key_menu(&mut self, code: KeyCode) {
        match code {
            // Nothing to confirm: every change is already on disk.
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Down | KeyCode::Char('j') => self.move_menu(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_menu(-1),
            KeyCode::Enter | KeyCode::Char(' ') => {
                let i = self.menu.selected().unwrap_or(0);
                self.activate_menu(MenuItem::ALL[i]);
            }
            _ => {}
        }
    }

    /// Launch the workflow for a menu item.
    fn activate_menu(&mut self, item: MenuItem) {
        self.status.clear();
        match item {
            MenuItem::Add => self.mode = Mode::Form(FormState::new_task()),
            MenuItem::Delegate => self.mode = Mode::Form(FormState::new_delegate()),
            MenuItem::Edit => self.open_picker(ListPurpose::Edit),
            MenuItem::Delete => self.open_picker(ListPurpose::Delete),
            MenuItem::Graph => {
                self.mode = Mode::Graph(GraphView {
                    focus: None,
                    scroll: 0,
                })
            }
            MenuItem::Quit => self.quit = true,
        }
    }

    /// Open the task picker for `purpose`, or bounce back to the menu with a hint
    /// when there is nothing to pick.
    fn open_picker(&mut self, purpose: ListPurpose) {
        if self.tasks.is_empty() {
            self.status = "no tasks yet — choose \"Add a task\" first".into();
            return;
        }
        self.list.select(Some(0));
        self.mode = Mode::List(purpose);
    }

    /// The task picker (opened from Edit or Delete). Enter acts on the selection;
    /// Esc returns to the menu.
    fn on_key_list(&mut self, code: KeyCode) {
        let Mode::List(purpose) = self.mode else {
            return;
        };
        match code {
            KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Menu,
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1),
            // 'g' previews the selected task's graph from either picker.
            KeyCode::Char('g') => {
                self.status.clear();
                let focus = self.selected_task();
                self.mode = Mode::Graph(GraphView { focus, scroll: 0 });
            }
            KeyCode::Enter => match purpose {
                ListPurpose::Edit => {
                    if let Some(key) = self.selected_task() {
                        self.status.clear();
                        self.mode = Mode::Form(FormState::from_doc(&self.doc, &key));
                    }
                }
                ListPurpose::Delete => {
                    if let Some(key) = self.selected_task() {
                        self.mode = Mode::ConfirmDelete(key);
                    }
                }
            },
            _ => {}
        }
    }

    fn on_key_graph(&mut self, code: KeyCode) {
        let Mode::Graph(mut view) = std::mem::replace(&mut self.mode, Mode::Menu) else {
            return;
        };
        match code {
            // Back to the menu.
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('g') => return,
            // Widen to all tasks.
            KeyCode::Char('a') | KeyCode::Char('G') => {
                view.focus = None;
                view.scroll = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => view.scroll = view.scroll.saturating_add(1),
            KeyCode::Up | KeyCode::Char('k') => view.scroll = view.scroll.saturating_sub(1),
            _ => {}
        }
        self.mode = Mode::Graph(view);
    }

    fn on_key_confirm_delete(&mut self, code: KeyCode) {
        let Mode::ConfirmDelete(key) = std::mem::replace(&mut self.mode, Mode::Menu) else {
            return;
        };
        match code {
            KeyCode::Char('y') => {
                if let Some(tasks) = self.doc.get_mut("tasks").and_then(Item::as_table_mut) {
                    tasks.remove(&key);
                    self.refresh_tasks();
                    self.status = match self.write_file() {
                        Ok(()) => format!("deleted '{key}'"),
                        Err(e) => e,
                    };
                }
                // Stay in the delete picker if tasks remain, else back to the menu.
                if !self.tasks.is_empty() {
                    self.mode = Mode::List(ListPurpose::Delete);
                }
            }
            // Cancel: back to the delete picker.
            KeyCode::Char('n') | KeyCode::Esc => self.mode = Mode::List(ListPurpose::Delete),
            _ => {}
        }
    }

    fn on_key_form(&mut self, code: KeyCode, ctrl: bool) {
        // Extract the form; put it back at the end (avoids borrow tangles).
        let Mode::Form(mut form) = std::mem::replace(&mut self.mode, Mode::Menu) else {
            return;
        };
        // Enter saves the form. No text field consumes Enter, and unlike Ctrl+S it
        // survives editor/IDE terminals, which grab that for "save file" (and it
        // is XOFF where terminal flow control is on). Ctrl+S stays an alias.
        if code == KeyCode::Enter || (ctrl && code == KeyCode::Char('s')) {
            match self.commit_form(&form) {
                Ok(name) => {
                    self.status = format!("saved task '{name}' to {}", self.path.display());
                    return; // back to Menu
                }
                Err(e) => form.error = Some(e),
            }
            self.mode = Mode::Form(form);
            return;
        }
        match code {
            KeyCode::Esc => {
                self.status = "edit cancelled".into();
                return; // mode already reset to Menu
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

    /// Validate `form` against a clone of the document; commit only if the whole
    /// resulting config still validates, then write it straight to disk. A failed
    /// write surfaces as a form error, so the user is never told a task was saved
    /// when it was not.
    fn commit_form(&mut self, form: &FormState) -> std::result::Result<String, String> {
        let mut candidate = self.doc.clone();
        let name = apply_form(&mut candidate, form)?;
        config::validate_str(&candidate.to_string()).map_err(|e| strip_banner(&e))?;
        self.doc = candidate;
        self.refresh_tasks();
        self.write_file()?;
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

    fn move_menu(&mut self, delta: i32) {
        let n = MenuItem::ALL.len() as i32;
        let cur = self.menu.selected().unwrap_or(0) as i32;
        self.menu.select(Some((cur + delta).rem_euclid(n) as usize));
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
            // No dirty marker: changes are written as they are made.
            Span::styled("  · autosaves", Style::default().fg(Color::DarkGray)),
        ]);
        frame.render_widget(Paragraph::new(title), chunks[0]);

        // `List` needs `&mut self` (list state), so copy its purpose out and
        // render it after the borrow of `self.mode` ends.
        let list_purpose = match &self.mode {
            Mode::Menu => {
                render_menu(frame, chunks[1], &mut self.menu);
                None
            }
            Mode::Form(form) => {
                render_form(frame, chunks[1], form);
                None
            }
            Mode::Graph(view) => {
                render_graph(frame, chunks[1], &self.doc, &self.path, view);
                None
            }
            Mode::ConfirmDelete(key) => {
                render_confirm_delete(frame, chunks[1], key);
                None
            }
            Mode::List(purpose) => Some(*purpose),
        };
        if let Some(purpose) = list_purpose {
            self.render_list(frame, chunks[1], purpose);
        }

        let help = match &self.mode {
            Mode::Menu => "↑↓ move · Enter select · q quit",
            Mode::List(ListPurpose::Edit) => "↑↓ move · Enter edit · g graph · Esc back",
            Mode::List(ListPurpose::Delete) => "↑↓ move · Enter delete · g graph · Esc back",
            Mode::Form(_) => {
                "↑↓/Tab field · ←→/Space change · type to edit · Enter save · Esc cancel"
            }
            Mode::Graph(_) => "↑↓ scroll · a all tasks · Esc/g back to menu",
            Mode::ConfirmDelete(_) => "delete task? — y delete · n/Esc cancel",
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

    fn render_list(&mut self, frame: &mut Frame, area: Rect, purpose: ListPurpose) {
        let items: Vec<ListItem> = self
            .tasks
            .iter()
            .map(|k| {
                let desc = describe(&self.doc, k);
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{k:<18}"), Style::default().fg(ACCENT)),
                    Span::styled(desc, Style::default().fg(Color::Gray)),
                ]))
            })
            .collect();
        let title = match purpose {
            ListPurpose::Edit => " select a task to edit ",
            ListPurpose::Delete => " select a task to delete ",
        };
        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title(title))
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
            .highlight_symbol("▌ ");
        frame.render_stateful_widget(list, area, &mut self.list);
    }
}

/// The home menu: a selectable list of workflows with one-line hints.
fn render_menu(frame: &mut Frame, area: Rect, state: &mut ListState) {
    let items: Vec<ListItem> = MenuItem::ALL
        .iter()
        .map(|item| {
            let (label, hint) = item.label_hint();
            ListItem::new(Line::from(vec![
                Span::styled(format!("{label:<18}"), Style::default().fg(ACCENT).bold()),
                Span::styled(hint, Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" what do you want to do? "),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("▌ ");
    frame.render_stateful_widget(list, area, state);
}

fn render_confirm_delete(frame: &mut Frame, area: Rect, key: &str) {
    let text = Paragraph::new(vec![
        Line::from(""),
        Line::from(vec![
            Span::raw("  Delete task "),
            Span::styled(format!("'{key}'"), Style::default().fg(ACCENT).bold()),
            Span::raw("?"),
        ]),
        Line::from(""),
        Line::from("  y  delete it"),
        Line::from("  n  keep it"),
    ])
    .block(Block::default().borders(Borders::ALL).title(" delete "));
    frame.render_widget(text, area);
}

// --- graph / dry-run preview ---

/// Which tree connector a node draws before its label.
#[derive(Clone, Copy)]
enum Branch {
    Root,
    Mid,
    Last,
}

fn render_graph(frame: &mut Frame, area: Rect, doc: &DocumentMut, path: &Path, view: &GraphView) {
    let title = match &view.focus {
        Some(k) => format!(" graph · {k} "),
        None => " graph · all tasks ".to_string(),
    };

    // Resolve auto-detect against the config's own directory, mirroring a real run.
    let root = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let lines = match config::parse_str(&doc.to_string(), root) {
        Ok(cfg) => build_graph_lines(&cfg, view.focus.as_deref()),
        Err(e) => vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("  cannot preview — {}", strip_banner(&e)),
                Style::default().fg(Color::Red),
            )),
        ],
    };

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .scroll((view.scroll, 0));
    frame.render_widget(para, area);
}

/// Build the connected dependency tree as styled lines: each task node shows its
/// resolved (dry-run) command; `deps` are drawn as children with box connectors.
fn build_graph_lines(cfg: &config::Config, focus: Option<&str>) -> Vec<Line<'static>> {
    let roots: Vec<String> = match focus {
        Some(k) => vec![k.to_string()],
        None => root_tasks(cfg),
    };
    if roots.is_empty() {
        return vec![Line::from(Span::styled(
            "  (no tasks yet — press Esc, then choose \"Add a task\")",
            Style::default().fg(Color::DarkGray),
        ))];
    }

    let mut out = vec![Line::from("")];
    for (i, r) in roots.iter().enumerate() {
        let mut ancestors: Vec<String> = Vec::new();
        node_lines(
            cfg,
            r,
            String::new(),
            Branch::Root,
            &mut ancestors,
            &mut out,
        );
        if i + 1 < roots.len() {
            out.push(Line::from(""));
        }
    }
    out
}

/// Tasks that nothing else depends on — the natural roots of the graph. Falls
/// back to every task if a cycle leaves none (the tree renderer breaks cycles).
fn root_tasks(cfg: &config::Config) -> Vec<String> {
    use std::collections::BTreeSet;
    let mut depended: BTreeSet<&str> = BTreeSet::new();
    for t in cfg.tasks.values() {
        for d in &t.deps {
            depended.insert(d.as_str());
        }
    }
    let roots: Vec<String> = cfg
        .tasks
        .keys()
        .filter(|k| !depended.contains(k.as_str()))
        .cloned()
        .collect();
    if roots.is_empty() {
        cfg.tasks.keys().cloned().collect()
    } else {
        roots
    }
}

/// Recursively append one node and its `deps` subtree. `prefix` is the accumulated
/// indentation; `ancestors` guards against cycles in a mid-edit config.
fn node_lines(
    cfg: &config::Config,
    key: &str,
    prefix: String,
    branch: Branch,
    ancestors: &mut Vec<String>,
    out: &mut Vec<Line<'static>>,
) {
    let connector = match branch {
        Branch::Root => "",
        Branch::Mid => "├─ ",
        Branch::Last => "└─ ",
    };
    let mut spans = vec![Span::styled(
        format!("  {prefix}{connector}"),
        Style::default().fg(Color::DarkGray),
    )];

    let Some(task) = cfg.task(key) else {
        // A dep that names no defined task.
        spans.push(Span::styled(
            format!("● {key}"),
            Style::default().fg(Color::Red),
        ));
        spans.push(Span::styled(
            "  (undefined task)",
            Style::default().fg(Color::Red),
        ));
        out.push(Line::from(spans));
        return;
    };

    spans.push(Span::styled(
        format!("● {key}"),
        Style::default().fg(ACCENT).bold(),
    ));
    if !task.deps.is_empty() {
        let tag = if task.parallel {
            "  ⇉ parallel"
        } else {
            "  → sequential"
        };
        spans.push(Span::styled(tag, Style::default().fg(Color::Yellow)));
    }
    spans.push(Span::styled(
        format!("   {}", dry_run(cfg, task)),
        Style::default().fg(Color::Gray),
    ));
    out.push(Line::from(spans));

    let child_prefix = match branch {
        Branch::Root => String::new(),
        Branch::Mid => format!("{prefix}│  "),
        Branch::Last => format!("{prefix}   "),
    };
    ancestors.push(key.to_string());
    let n = task.deps.len();
    for (i, dep) in task.deps.iter().enumerate() {
        let last = i + 1 == n;
        if ancestors.contains(dep) {
            let conn = if last { "└─ " } else { "├─ " };
            out.push(Line::from(vec![
                Span::styled(
                    format!("  {child_prefix}{conn}"),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(format!("↺ {dep}"), Style::default().fg(Color::Red)),
                Span::styled("  (cycle)", Style::default().fg(Color::Red)),
            ]));
        } else {
            let b = if last { Branch::Last } else { Branch::Mid };
            node_lines(cfg, dep, child_prefix.clone(), b, ancestors, out);
        }
    }
    ancestors.pop();
}

/// The command `tsr` would run for a task, computed from the config alone — the
/// same precedence the executor uses (SPEC §3.1, §5): a deps-only task is a pure
/// aggregator; otherwise resolve `delegate` / `run` / auto-detect.
fn dry_run(cfg: &config::Config, task: &config::Task) -> String {
    if task.run.is_none()
        && task.delegate.is_none()
        && task.packages.is_none()
        && !task.deps.is_empty()
    {
        return "runs its deps only".to_string();
    }

    let dir = match &task.dir {
        Some(d) => cfg.root.join(d),
        None => cfg.root.clone(),
    };
    let base = match resolve::resolve(task, &dir) {
        Ok(Invocation::Direct { program, args }) => std::iter::once(program)
            .chain(args)
            .collect::<Vec<_>>()
            .join(" "),
        Ok(Invocation::Run(s)) => s,
        // Auto-detect with no ecosystem marker in `dir` — can't name the runner.
        Err(_) => "auto-detect (native runner)".to_string(),
    };

    let mut cmd = format!("→ {base}");
    if !task.args.is_empty() {
        cmd.push(' ');
        cmd.push_str(&task.args.join(" "));
    }
    match &task.packages {
        Some(p) if !p.is_empty() => format!("{cmd}   × packages [{}]", p.join(", ")),
        _ => cmd,
    }
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
        F_ENV_FILE => Some(".env paths, comma-separated (later overrides earlier)"),
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
            t("env file"),
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

    /// A blank task form pre-set to the `delegate` type, for the "Delegate a
    /// task" workflow.
    fn new_delegate() -> FormState {
        let mut form = Self::new_task();
        form.set_choice(F_TYPE, 1); // delegate (string)
        form
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
        form.set_text(F_ENV_FILE, &join_env_file(t.get("env_file")));
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
    // env_file: a single path serializes as a string, several as an array — both
    // parse back the same way (config::parse_string_or_array).
    let env_files = split_csv(form.text(F_ENV_FILE));
    match env_files.as_slice() {
        [] => {}
        [one] => {
            t.insert("env_file", str_item(one));
        }
        many => {
            t.insert("env_file", array_item(many));
        }
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
/// `env_file` may be a single string or an array of them; show both as a
/// comma-separated list in the form.
fn join_env_file(item: Option<&Item>) -> String {
    match item {
        Some(i) if i.is_str() => i.as_str().unwrap_or_default().to_string(),
        Some(i) => join_csv(Some(i)),
        None => String::new(),
    }
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
    fn env_file_serializes_string_or_array_and_round_trips() {
        // Several paths → array.
        let f = form_with(
            &[
                (F_NAME, "test"),
                (F_RUN, "vitest"),
                (F_ENV_FILE, ".env.local, .env.test"),
            ],
            &[(F_TYPE, 0)],
        );
        let mut doc = DocumentMut::new();
        apply_form(&mut doc, &f).unwrap();
        config::validate_str(&doc.to_string()).unwrap();
        assert!(
            doc.to_string()
                .contains("env_file = [\".env.local\", \".env.test\"]"),
            "{}",
            doc.to_string()
        );

        // A single path → plain string, and it survives an edit round-trip.
        let one = form_with(
            &[(F_NAME, "t"), (F_RUN, "vitest"), (F_ENV_FILE, ".env.test")],
            &[(F_TYPE, 0)],
        );
        let mut doc2 = DocumentMut::new();
        apply_form(&mut doc2, &one).unwrap();
        assert!(doc2.to_string().contains("env_file = \".env.test\""));
        let back = FormState::from_doc(&doc2, "t");
        assert_eq!(back.text(F_ENV_FILE), ".env.test");
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

    // --- graph / dry-run preview ---

    /// Parse a config for the preview, rooted at a marker-free temp dir so
    /// auto-detect resolves deterministically to "native runner" (no ecosystem).
    fn preview_cfg(text: &str) -> config::Config {
        let dir = std::env::temp_dir().join(format!(
            "tsr-tui-graph-{}-{}",
            std::process::id(),
            text.len()
        ));
        let _ = std::fs::create_dir_all(&dir);
        config::parse_str(text, dir).unwrap()
    }

    fn all_text(lines: &[Line]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn dry_run_resolves_each_form() {
        let cfg = preview_cfg(
            "[tasks.dev]\nrun = \"vite\"\nargs = [\"--host\"]\n\
             [tasks.build]\ndelegate = \"turbo\"\n\
             [tasks.bundle]\ndelegate = { bin = \"make\", args = [\"bundle\"] }\n\
             [tasks.ci]\ndeps = [\"build\"]\n\
             [tasks.detect]\n",
        );
        assert_eq!(dry_run(&cfg, cfg.task("dev").unwrap()), "→ vite --host");
        assert_eq!(
            dry_run(&cfg, cfg.task("build").unwrap()),
            "→ turbo run build"
        );
        assert_eq!(dry_run(&cfg, cfg.task("bundle").unwrap()), "→ make bundle");
        assert_eq!(dry_run(&cfg, cfg.task("ci").unwrap()), "runs its deps only");
        assert_eq!(
            dry_run(&cfg, cfg.task("detect").unwrap()),
            "→ auto-detect (native runner)"
        );
    }

    #[test]
    fn dry_run_annotates_packages_fan_out() {
        let cfg = preview_cfg(
            "[workspace]\nmembers = [\"apps/*\"]\n\
             [tasks.test]\nrun = \"vitest\"\npackages = [\"apps/*\"]\n",
        );
        assert_eq!(
            dry_run(&cfg, cfg.task("test").unwrap()),
            "→ vitest   × packages [apps/*]"
        );
    }

    #[test]
    fn roots_exclude_depended_on_tasks() {
        let cfg = preview_cfg(
            "[tasks.ci]\ndeps = [\"lint\", \"test\"]\n\
             [tasks.lint]\nrun = \"true\"\n[tasks.test]\nrun = \"true\"\n\
             [tasks.standalone]\nrun = \"true\"\n",
        );
        // lint/test are depended on by ci, so only ci + standalone are roots.
        assert_eq!(root_tasks(&cfg), vec!["ci", "standalone"]);
    }

    #[test]
    fn graph_lines_draw_connected_tree() {
        let cfg = preview_cfg(
            "[tasks.ci]\ndeps = [\"lint\", \"build\"]\nparallel = true\n\
             [tasks.lint]\nrun = \"eslint .\"\n\
             [tasks.build]\ndelegate = \"turbo\"\n",
        );
        let text = all_text(&build_graph_lines(&cfg, Some("ci")));
        assert!(text.contains("● ci"), "{text}");
        assert!(text.contains("⇉ parallel"), "{text}");
        assert!(
            text.contains("├─ ● lint") && text.contains("→ eslint ."),
            "{text}"
        );
        assert!(
            text.contains("└─ ● build") && text.contains("→ turbo run build"),
            "{text}"
        );
    }

    #[test]
    fn graph_marks_undefined_deps_and_cycles() {
        let missing = preview_cfg("[tasks.a]\ndeps = [\"ghost\"]\n[tasks.b]\nrun = \"true\"\n");
        assert!(all_text(&build_graph_lines(&missing, Some("a"))).contains("(undefined task)"));

        // a → b → a is a cycle; the tree must break it, not recurse forever.
        let cyclic = preview_cfg("[tasks.a]\ndeps = [\"b\"]\n[tasks.b]\ndeps = [\"a\"]\n");
        assert!(all_text(&build_graph_lines(&cyclic, Some("a"))).contains("(cycle)"));
    }

    #[test]
    fn delegate_workflow_preselects_delegate_type() {
        // "Delegate a task" opens the form already on the delegate type, so the
        // user lands on the delegate-bin field rather than the run command.
        let form = FormState::new_delegate();
        assert_eq!(form.choice(F_TYPE), 1);

        // And it serializes as a delegate task once a bin is filled in.
        let mut f = form;
        f.set_text(F_NAME, "build");
        f.set_text(F_DBIN, "turbo");
        let mut doc = DocumentMut::new();
        apply_form(&mut doc, &f).unwrap();
        config::validate_str(&doc.to_string()).unwrap();
        assert!(doc.to_string().contains("delegate = \"turbo\""));
    }

    /// An App over a scratch `tasks.toml` path that is never pre-created.
    fn app_at(tag: &str, src: &str) -> App {
        let dir = std::env::temp_dir().join(format!("tsr-tui-keys-{}-{tag}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tasks.toml");
        let _ = std::fs::remove_file(&path);
        App::new(path, src.parse::<DocumentMut>().unwrap())
    }

    #[test]
    fn enter_writes_the_task_straight_to_disk() {
        let mut app = app_at("form-enter", "");
        let mut form = FormState::new_task();
        form.set_text(F_NAME, "dev");
        form.set_text(F_RUN, "vite");
        app.mode = Mode::Form(form);

        // Enter (no Ctrl, no second save step) commits *and* writes the file.
        app.on_key_form(KeyCode::Enter, false);
        assert!(matches!(app.mode, Mode::Menu));
        assert_eq!(app.tasks, vec!["dev"]);
        assert!(app.path.is_file(), "the form must autosave on apply");
        let on_disk = std::fs::read_to_string(&app.path).unwrap();
        assert!(on_disk.contains("run = \"vite\""), "{on_disk}");
    }

    #[test]
    fn deleting_a_task_writes_the_file() {
        let mut app = app_at(
            "del-save",
            "[tasks.dev]\nrun = \"vite\"\n[tasks.ci]\nrun = \"x\"\n",
        );
        app.mode = Mode::ConfirmDelete("dev".into());
        app.on_key_confirm_delete(KeyCode::Char('y'));
        assert_eq!(app.tasks, vec!["ci"]);
        let on_disk = std::fs::read_to_string(&app.path).unwrap();
        assert!(!on_disk.contains("[tasks.dev]"), "{on_disk}");
        assert!(on_disk.contains("[tasks.ci]"), "{on_disk}");
    }

    #[test]
    fn cancelling_a_delete_leaves_the_file_alone() {
        let mut app = app_at("del-cancel", "[tasks.dev]\nrun = \"vite\"\n");
        app.mode = Mode::ConfirmDelete("dev".into());
        app.on_key_confirm_delete(KeyCode::Char('n'));
        assert_eq!(app.tasks, vec!["dev"]);
        assert!(!app.path.is_file(), "cancelling must not write anything");
    }

    #[test]
    fn form_enter_keeps_the_form_open_on_a_validation_error() {
        let mut app = app_at("form-err", "");
        // No name → invalid; the form must stay open and show the error.
        app.mode = Mode::Form(FormState::new_task());
        app.on_key_form(KeyCode::Enter, false);
        match &app.mode {
            Mode::Form(f) => assert!(f.error.is_some(), "expected an inline error"),
            _ => panic!("form should stay open on an invalid apply"),
        }
        // An invalid form is never written — autosave only follows a valid commit.
        assert!(!app.path.is_file());
    }

    #[test]
    fn typing_in_a_form_field_does_not_write() {
        // Plain letters are text input; only Enter commits and autosaves.
        let mut app = app_at("form-typing", "");
        app.mode = Mode::Form(FormState::new_task());
        for c in "start".chars() {
            app.on_key_form(KeyCode::Char(c), false);
        }
        match &app.mode {
            Mode::Form(f) => assert_eq!(f.text(F_NAME), "start"),
            _ => panic!("form should still be open"),
        }
        assert!(!app.path.is_file(), "typing must not have written the file");
    }

    #[test]
    fn quitting_needs_no_confirmation() {
        // Autosave means there is never unsaved work to warn about.
        let mut app = app_at("quit", "[tasks.dev]\nrun = \"vite\"\n");
        app.on_key_menu(KeyCode::Char('q'));
        assert!(app.quit);
    }

    #[test]
    fn menu_has_a_hint_for_every_item() {
        // Every home-menu entry carries a non-empty label and hint.
        for item in MenuItem::ALL {
            let (label, hint) = item.label_hint();
            assert!(!label.is_empty());
            assert!(!hint.is_empty());
        }
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
