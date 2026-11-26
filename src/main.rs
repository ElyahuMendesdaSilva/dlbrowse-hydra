use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use ratatui::{
    crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, List, ListItem, ListState, Paragraph, Wrap},
    DefaultTerminal, Frame,
};
use serde::{Deserialize, Serialize};

// ───────────────────────── Modelo do JSON ─────────────────────────

#[derive(Deserialize)]
struct Source {
    #[serde(default)]
    name: String,
    #[serde(default)]
    downloads: Vec<Download>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Download {
    #[serde(default)]
    title: String,
    #[serde(default)]
    uris: Vec<String>,
    #[serde(default)]
    uri_groups: Vec<Vec<String>>,
    #[serde(default)]
    upload_date: Option<String>,
    #[serde(default)]
    file_size: Option<String>,
}

// ───────────────────────── Entrada normalizada ─────────────────────────

struct Entry {
    title: String,
    title_lc: String,
    source: String,
    source_lc: String,
    uris: Vec<String>,
    groups: Vec<Vec<String>>,
    date: Option<DateTime<Utc>>,
    date_raw: String,
    size_raw: String,
    size_bytes: u64,
}

impl Entry {
    fn from_download(d: Download, source: &str) -> Self {
        let date_raw = d.upload_date.unwrap_or_default();
        // Aceita "2026-11-20T03:33:52.000Z" e "2026-11-19T17:39:50+00:00"
        let date = DateTime::parse_from_rfc3339(&date_raw)
            .ok()
            .map(|x| x.with_timezone(&Utc));
        let size_raw = d.file_size.unwrap_or_default();
        Entry {
            title_lc: d.title.to_lowercase(),
            title: d.title,
            source_lc: source.to_lowercase(),
            source: source.to_string(),
            uris: d.uris,
            groups: d.uri_groups,
            date,
            size_bytes: parse_size(&size_raw),
            date_raw,
            size_raw,
        }
    }

    /// Todos os links (uris + uriGroups), sem vazios e sem duplicados.
    fn links(&self) -> Vec<(String, String)> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for (i, u) in self.uris.iter().enumerate() {
            let u = u.trim();
            if !u.is_empty() && seen.insert(u.to_string()) {
                out.push((format!("URI {}", i + 1), u.to_string()));
            }
        }
        for (g, grp) in self.groups.iter().enumerate() {
            for (i, u) in grp.iter().enumerate() {
                let u = u.trim();
                if !u.is_empty() && seen.insert(u.to_string()) {
                    out.push((format!("Grupo {} · {}", g + 1, i + 1), u.to_string()));
                }
            }
        }
        out
    }

    fn date_short(&self) -> String {
        self.date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "----------".into())
    }
}

fn parse_size(s: &str) -> u64 {
    let s = s.trim();
    if s.is_empty() {
        return 0;
    }
    let mut parts = s.split_whitespace();
    let num: f64 = parts
        .next()
        .map(|n| n.replace(',', "."))
        .and_then(|n| n.parse().ok())
        .unwrap_or(0.0);
    let mult = match parts.next().map(|u| u.to_uppercase()).as_deref() {
        Some("KB") | Some("KIB") => 1024.0,
        Some("MB") | Some("MIB") => 1024.0 * 1024.0,
        Some("GB") | Some("GIB") => 1024.0 * 1024.0 * 1024.0,
        Some("TB") | Some("TIB") => 1024.0_f64.powi(4),
        _ => 1.0,
    };
    (num * mult) as u64
}

// ───────────────────────── Categorias ─────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum CatMode {
    Provider,
    Month,
    Size,
    Host,
}

impl CatMode {
    fn next(self) -> Self {
        match self {
            CatMode::Provider => CatMode::Month,
            CatMode::Month => CatMode::Size,
            CatMode::Size => CatMode::Host,
            CatMode::Host => CatMode::Provider,
        }
    }
    fn label(self) -> &'static str {
        match self {
            CatMode::Provider => "Fornecedor",
            CatMode::Month => "Mês de upload",
            CatMode::Size => "Tamanho",
            CatMode::Host => "Host do link",
        }
    }
}

fn host_of(url: &str) -> String {
    if url.starts_with("magnet:") {
        return "magnet".into();
    }
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = host.rsplit('@').next().unwrap_or(host);
    host.trim_start_matches("www.").to_lowercase()
}

fn cat_key(e: &Entry, mode: CatMode) -> String {
    match mode {
        CatMode::Provider => {
            if e.source.is_empty() {
                "(sem nome)".into()
            } else {
                e.source.clone()
            }
        }
        CatMode::Month => e
            .date
            .map(|d| d.format("%Y-%m").to_string())
            .unwrap_or_else(|| "Sem data".into()),
        CatMode::Size => {
            const MB: u64 = 1024 * 1024;
            const GB: u64 = 1024 * MB;
            match e.size_bytes {
                0 => "5. Desconhecido",
                b if b < 100 * MB => "1. Menos de 100 MB",
                b if b < GB => "2. 100 MB – 1 GB",
                b if b < 10 * GB => "3. 1 – 10 GB",
                _ => "4. Mais de 10 GB",
            }
            .into()
        }
        CatMode::Host => e
            .links()
            .first()
            .map(|(_, u)| host_of(u))
            .unwrap_or_else(|| "(sem link)".into()),
    }
}

// ───────────────────────── Estado do app ─────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Categories,
    List,
    Links,
}

#[derive(Clone, Copy)]
enum Sort {
    Date,
    Title,
    Size,
}

impl Sort {
    fn next(self) -> Self {
        match self {
            Sort::Date => Sort::Title,
            Sort::Title => Sort::Size,
            Sort::Size => Sort::Date,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Sort::Date => "data ↓",
            Sort::Title => "título A-Z",
            Sort::Size => "tamanho ↓",
        }
    }
}

struct App {
    entries: Vec<Entry>,
    keys: Vec<String>, // categoria de cada entrada (no modo atual)
    mode: CatMode,
    cats: Vec<(String, usize)>,
    cat_sel: usize,
    filtered: Vec<usize>,
    sel: usize,
    link_sel: usize,
    query: String,
    searching: bool,
    focus: Focus,
    sort: Sort,
    status: String,
    quit: bool,
    want_picker: bool,
    errors: Vec<String>,
    selection: Vec<PathBuf>,
}

fn step(cur: usize, len: usize, d: isize) -> usize {
    if len == 0 {
        return 0;
    }
    (cur as isize).saturating_add(d).clamp(0, len as isize - 1) as usize
}

impl App {
    fn new(entries: Vec<Entry>, errors: Vec<String>, selection: Vec<PathBuf>) -> Self {
        let mut app = App {
            entries,
            keys: vec![],
            mode: CatMode::Provider,
            cats: vec![],
            cat_sel: 0,
            filtered: vec![],
            sel: 0,
            link_sel: 0,
            query: String::new(),
            searching: false,
            focus: Focus::List,
            sort: Sort::Date,
            status: String::new(),
            quit: false,
            want_picker: false,
            errors,
            selection,
        };
        app.rebuild_categories();
        app
    }

    fn set_entries(&mut self, entries: Vec<Entry>, errors: Vec<String>) {
        self.entries = entries;
        self.errors = errors;
        self.query.clear();
        self.focus = Focus::List;
        self.rebuild_categories();
    }

    fn rebuild_categories(&mut self) {
        self.keys = self.entries.iter().map(|e| cat_key(e, self.mode)).collect();
        let mut map: BTreeMap<String, usize> = BTreeMap::new();
        for k in &self.keys {
            *map.entry(k.clone()).or_default() += 1;
        }
        let mut v: Vec<(String, usize)> = map.into_iter().collect();
        match self.mode {
            CatMode::Month => v.reverse(),
            CatMode::Host => v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0))),
            _ => {}
        }
        let mut cats = vec![("Todos".to_string(), self.entries.len())];
        cats.extend(v);
        self.cats = cats;
        self.cat_sel = 0;
        self.refilter();
    }

    fn refilter(&mut self) {
        let terms: Vec<String> = self
            .query
            .to_lowercase()
            .split_whitespace()
            .map(String::from)
            .collect();
        let cat = if self.cat_sel == 0 {
            None
        } else {
            Some(self.cats[self.cat_sel].0.as_str())
        };
        let mut f: Vec<usize> = (0..self.entries.len())
            .filter(|&i| {
                cat.map_or(true, |c| self.keys[i] == c)
                    && terms.iter().all(|t| {
                        self.entries[i].title_lc.contains(t.as_str())
                            || self.entries[i].source_lc.contains(t.as_str())
                    })
            })
            .collect();
        let en = &self.entries;
        match self.sort {
            Sort::Date => f.sort_by(|&a, &b| en[b].date.cmp(&en[a].date)),
            Sort::Title => f.sort_by(|&a, &b| en[a].title_lc.cmp(&en[b].title_lc)),
            Sort::Size => f.sort_by(|&a, &b| en[b].size_bytes.cmp(&en[a].size_bytes)),
        }
        self.filtered = f;
        self.sel = 0;
        self.link_sel = 0;
    }

    fn current(&self) -> Option<&Entry> {
        self.filtered.get(self.sel).map(|&i| &self.entries[i])
    }

    fn current_links(&self) -> Vec<(String, String)> {
        self.current().map(|e| e.links()).unwrap_or_default()
    }

    fn open_link(&mut self, url: &str) {
        self.status = match open::that_detached(url) {
            Ok(_) => format!("Aberto: {url}"),
            Err(e) => format!("Erro ao abrir: {e}"),
        };
    }

    fn mv(&mut self, d: isize) {
        match self.focus {
            Focus::Categories => {
                let n = step(self.cat_sel, self.cats.len(), d);
                if n != self.cat_sel {
                    self.cat_sel = n;
                    self.refilter();
                }
            }
            Focus::List => {
                self.sel = step(self.sel, self.filtered.len(), d);
                self.link_sel = 0;
            }
            Focus::Links => {
                let len = self.current_links().len();
                self.link_sel = step(self.link_sel, len, d);
            }
        }
    }

    fn focus_next(&mut self) {
        self.focus = match self.focus {
            Focus::Categories => Focus::List,
            Focus::List => Focus::Links,
            Focus::Links => Focus::Categories,
        };
    }

    fn focus_prev(&mut self) {
        self.focus = match self.focus {
            Focus::Categories => Focus::Links,
            Focus::List => Focus::Categories,
            Focus::Links => Focus::List,
        };
    }

    fn on_key(&mut self, key: KeyEvent) {
        self.status.clear();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        if self.searching {
            match key.code {
                KeyCode::Esc => {
                    self.searching = false;
                    self.query.clear();
                    self.refilter();
                }
                KeyCode::Enter => self.searching = false,
                KeyCode::Backspace => {
                    self.query.pop();
                    self.refilter();
                }
                KeyCode::Char('u') if ctrl => {
                    self.query.clear();
                    self.refilter();
                }
                KeyCode::Char(c) if !ctrl => {
                    self.query.push(c);
                    self.refilter();
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Char('c') if ctrl => self.quit = true,
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('/') => self.searching = true,
            KeyCode::Esc => {
                if !self.query.is_empty() {
                    self.query.clear();
                    self.refilter();
                } else if self.focus == Focus::Links {
                    self.focus = Focus::List;
                }
            }
            KeyCode::Tab => self.focus_next(),
            KeyCode::BackTab => self.focus_prev(),
            KeyCode::Left | KeyCode::Char('h') => self.focus_prev(),
            KeyCode::Right | KeyCode::Char('l') => self.focus_next(),
            KeyCode::Down | KeyCode::Char('j') => self.mv(1),
            KeyCode::Up | KeyCode::Char('k') => self.mv(-1),
            KeyCode::PageDown => self.mv(10),
            KeyCode::PageUp => self.mv(-10),
            KeyCode::Home | KeyCode::Char('g') => self.mv(isize::MIN),
            KeyCode::End | KeyCode::Char('G') => self.mv(isize::MAX),
            KeyCode::Char('c') => {
                self.mode = self.mode.next();
                self.rebuild_categories();
            }
            KeyCode::Char('s') => {
                self.sort = self.sort.next();
                self.refilter();
            }
            KeyCode::Char('f') => self.want_picker = true,
            KeyCode::Char('o') => {
                if let Some((_, url)) = self.current_links().first().cloned() {
                    self.open_link(&url);
                } else {
                    self.status = "Esta entrada não tem links.".into();
                }
            }
            KeyCode::Enter => match self.focus {
                Focus::Categories => self.focus = Focus::List,
                Focus::List => {
                    if !self.current_links().is_empty() {
                        self.focus = Focus::Links;
                    } else {
                        self.status = "Esta entrada não tem links.".into();
                    }
                }
                Focus::Links => {
                    let links = self.current_links();
                    if let Some((_, url)) = links.get(self.link_sel) {
                        let url = url.clone();
                        self.open_link(&url);
                    }
                }
            },
            _ => {}
        }
    }
}

// ───────────────────────── Interface ─────────────────────────

fn border(focused: bool) -> Style {
    if focused {
        Style::new().fg(Color::Yellow)
    } else {
        Style::new().fg(Color::DarkGray)
    }
}

fn hl() -> Style {
    Style::new()
        .bg(Color::Blue)
        .fg(Color::White)
        .add_modifier(Modifier::BOLD)
}

/// Janela visível: só construímos os itens que cabem na tela (JSON grande = milhares de itens).
fn window(sel: usize, len: usize, h: usize) -> (usize, usize) {
    let h = h.max(1);
    let start = sel.saturating_sub(h / 2).min(len.saturating_sub(h));
    (start, (start + h).min(len))
}

fn ui(f: &mut Frame, app: &App) {
    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(5),
        Constraint::Length(1),
    ])
    .split(f.area());

    // Pesquisa
    let search = Paragraph::new(app.query.as_str()).block(
        Block::bordered()
            .title(" Pesquisa ( / ) ")
            .border_style(if app.searching {
                Style::new().fg(Color::Yellow)
            } else {
                Style::new().fg(Color::DarkGray)
            }),
    );
    f.render_widget(search, rows[0]);
    if app.searching {
        f.set_cursor_position((
            rows[0].x + 1 + app.query.chars().count() as u16,
            rows[0].y + 1,
        ));
    }

    // Corpo
    let cols = Layout::horizontal([Constraint::Length(30), Constraint::Min(30)]).split(rows[1]);
    draw_categories(f, app, cols[0]);

    let right = Layout::vertical([
        Constraint::Percentage(45),
        Constraint::Length(8),
        Constraint::Min(4),
    ])
    .split(cols[1]);
    draw_list(f, app, right[0]);
    draw_details(f, app, right[1]);
    draw_links(f, app, right[2]);

    // Rodapé
    let footer = if !app.status.is_empty() {
        Line::from(Span::styled(
            app.status.as_str(),
            Style::new().fg(Color::Green),
        ))
    } else {
        Line::from(Span::styled(
            " / buscar │ Tab painel │ jk mover │ Enter abrir │ o 1º link │ c categoria │ s ordem │ f arquivos │ q sair",
            Style::new().fg(Color::DarkGray),
        ))
    };
    f.render_widget(Paragraph::new(footer), rows[2]);
}

fn draw_categories(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .cats
        .iter()
        .map(|(n, c)| ListItem::new(format!("{n} ({c})")))
        .collect();
    let list = List::new(items)
        .block(
            Block::bordered()
                .title(format!(" Categoria: {} ", app.mode.label()))
                .border_style(border(app.focus == Focus::Categories)),
        )
        .highlight_style(hl());
    let mut st = ListState::default().with_selected(Some(app.cat_sel));
    f.render_stateful_widget(list, area, &mut st);
}

fn draw_list(f: &mut Frame, app: &App, area: Rect) {
    let h = area.height.saturating_sub(2) as usize;
    let (start, end) = window(app.sel, app.filtered.len(), h);
    let items: Vec<ListItem> = app.filtered[start..end]
        .iter()
        .map(|&i| {
            let e = &app.entries[i];
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{:>9} ", e.size_raw),
                    Style::new().fg(Color::Cyan),
                ),
                Span::styled(
                    format!("{} ", e.date_short()),
                    Style::new().fg(Color::DarkGray),
                ),
                Span::raw(e.title.as_str()),
            ]))
        })
        .collect();
    let title = format!(
        " Downloads ({}/{}) — ordem: {} ",
        app.filtered.len(),
        app.entries.len(),
        app.sort.label()
    );
    let list = List::new(items)
        .block(
            Block::bordered()
                .title(title)
                .border_style(border(app.focus == Focus::List)),
        )
        .highlight_style(hl());
    let mut st = ListState::default().with_selected(if app.filtered.is_empty() {
        None
    } else {
        Some(app.sel - start)
    });
    f.render_stateful_widget(list, area, &mut st);
}

fn draw_details(f: &mut Frame, app: &App, area: Rect) {
    let label = Style::new().fg(Color::Yellow);
    let lines = match app.current() {
        Some(e) => vec![
            Line::from(Span::styled(
                e.title.as_str(),
                Style::new().add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![Span::styled("Fornecedor: ", label), Span::raw(e.source.as_str())]),
            Line::from(vec![Span::styled("Tamanho:    ", label), Span::raw(e.size_raw.as_str())]),
            Line::from(vec![Span::styled("Upload:     ", label), Span::raw(e.date_raw.as_str())]),
            Line::from(vec![
                Span::styled("Links:      ", label),
                Span::raw(format!("{}", e.links().len())),
            ]),
        ],
        None => vec![Line::from("Nada encontrado.")],
    };
    let p = Paragraph::new(lines)
        .wrap(Wrap { trim: true })
        .block(Block::bordered().title(" Detalhes ").border_style(border(false)));
    f.render_widget(p, area);
}

fn draw_links(f: &mut Frame, app: &App, area: Rect) {
    let links = app.current_links();
    let items: Vec<ListItem> = links
        .iter()
        .map(|(l, u)| {
            ListItem::new(Line::from(vec![
                Span::styled(format!("{l:<14} "), Style::new().fg(Color::Magenta)),
                Span::raw(u.as_str()),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::bordered()
                .title(" Links (Enter abre) ")
                .border_style(border(app.focus == Focus::Links)),
        )
        .highlight_style(hl());
    let mut st = ListState::default().with_selected(if links.is_empty() {
        None
    } else {
        Some(app.link_sel)
    });
    f.render_stateful_widget(list, area, &mut st);
}

// ───────────────────────── Main ─────────────────────────

fn is_json(p: &Path) -> bool {
    p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json"))
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let path = e.path();
        if path.is_dir() {
            walk(&path, out);
        } else if is_json(&path) {
            out.push(path);
        }
    }
}

/// Expande a seleção: pastas viram todos os .json dentro delas (recursivo).
fn expand(sel: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = vec![];
    for p in sel {
        if p.is_dir() {
            walk(p, &mut out);
        } else {
            out.push(p.clone());
        }
    }
    out.sort();
    out.dedup();
    out
}

fn parse_source(bytes: &[u8]) -> Result<Source> {
    match serde_json::from_slice::<Source>(bytes) {
        Ok(s) => Ok(s),
        // Fallback tolerante: vírgulas sobrando, comentários etc.
        Err(e) => {
            let text = std::str::from_utf8(bytes)?;
            json5::from_str::<Source>(text).map_err(|_| anyhow::anyhow!(e))
        }
    }
}

fn load_file(p: &Path) -> Result<Vec<Entry>> {
    let bytes = std::fs::read(p).with_context(|| format!("lendo {}", p.display()))?;
    let src = parse_source(&bytes)?;
    let name = if src.name.is_empty() {
        p.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        src.name
    };
    Ok(src
        .downloads
        .into_iter()
        .map(|d| Entry::from_download(d, &name))
        .collect())
}

/// Carrega a seleção mostrando uma tela de progresso. Arquivos com erro são ignorados.
/// Retorna (entradas, erros, nº de arquivos).
fn load_with_screen(
    terminal: &mut DefaultTerminal,
    sel: &[PathBuf],
) -> Result<(Vec<Entry>, Vec<String>, usize)> {
    let files = expand(sel);
    let mut entries = vec![];
    let mut errors = vec![];
    for (i, p) in files.iter().enumerate() {
        if i % 10 == 0 || i + 1 == files.len() {
            terminal.draw(|f| {
                let msg = format!("{}/{}\n\n{}", i + 1, files.len(), p.display());
                f.render_widget(
                    Paragraph::new(msg)
                        .alignment(Alignment::Center)
                        .wrap(Wrap { trim: true })
                        .block(Block::bordered().title(" Carregando ")),
                    f.area(),
                );
            })?;
        }
        match load_file(p) {
            Ok(mut v) => entries.append(&mut v),
            Err(e) => errors.push(format!("{}: {e}", p.display())),
        }
    }
    Ok((entries, errors, files.len()))
}

// ───────────────────────── Memória da seleção ─────────────────────────

#[derive(Serialize, Deserialize, Default)]
struct Config {
    #[serde(default)]
    paths: Vec<String>,
}

fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("dlbrowse-hydra").join("selection.json"))
}

fn load_saved() -> Vec<PathBuf> {
    let Some(p) = config_path() else { return vec![] };
    let Ok(bytes) = std::fs::read(p) else { return vec![] };
    let cfg: Config = serde_json::from_slice(&bytes).unwrap_or_default();
    cfg.paths
        .into_iter()
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .collect()
}

fn save_selection(paths: &[PathBuf]) -> Result<()> {
    let p = config_path().context("não achei a pasta de configuração")?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let cfg = Config {
        paths: paths.iter().map(|x| x.to_string_lossy().into_owned()).collect(),
    };
    std::fs::write(p, serde_json::to_string_pretty(&cfg)?)?;
    Ok(())
}

fn clear_saved() -> Result<bool> {
    let Some(p) = config_path() else { return Ok(false) };
    match std::fs::remove_file(&p) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

fn default_start() -> PathBuf {
    std::env::current_dir()
        .ok()
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn start_dir_for(selection: &[PathBuf]) -> PathBuf {
    match selection.first() {
        Some(p) if p.is_dir() => p.clone(),
        Some(p) => p.parent().map(Path::to_path_buf).unwrap_or_else(default_start),
        None => default_start(),
    }
}

// ───────────────────────── Seletor de arquivos ─────────────────────────

struct PItem {
    name: String,
    path: PathBuf,
    is_dir: bool,
    is_parent: bool,
}

struct Picker {
    dir: PathBuf,
    items: Vec<PItem>,
    state: ListState,
    selected: BTreeSet<PathBuf>,
    hidden: bool,
    msg: String,
}

impl Picker {
    fn new(start: PathBuf, pre: Vec<PathBuf>) -> Self {
        let mut p = Picker {
            dir: start,
            items: vec![],
            state: ListState::default(),
            selected: pre.into_iter().collect(),
            hidden: false,
            msg: String::new(),
        };
        p.refresh(None);
        p
    }

    fn refresh(&mut self, focus: Option<&Path>) {
        let mut dirs = vec![];
        let mut files = vec![];
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for e in rd.flatten() {
                let path = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                if !self.hidden && name.starts_with('.') {
                    continue;
                }
                if path.is_dir() {
                    dirs.push(PItem { name: format!("{name}/"), path, is_dir: true, is_parent: false });
                } else if is_json(&path) {
                    files.push(PItem { name, path, is_dir: false, is_parent: false });
                }
            }
        }
        dirs.sort_by_key(|i| i.name.to_lowercase());
        files.sort_by_key(|i| i.name.to_lowercase());

        let mut items = vec![];
        if let Some(parent) = self.dir.parent() {
            items.push(PItem {
                name: "../".into(),
                path: parent.to_path_buf(),
                is_dir: true,
                is_parent: true,
            });
        }
        items.extend(dirs);
        items.extend(files);
        self.items = items;

        let idx = focus
            .and_then(|f| self.items.iter().position(|i| i.path == f))
            .unwrap_or(0);
        *self.state.offset_mut() = 0;
        self.state.select(if self.items.is_empty() { None } else { Some(idx) });
    }

    fn cur(&self) -> Option<(bool, bool, PathBuf)> {
        self.state
            .selected()
            .and_then(|i| self.items.get(i))
            .map(|i| (i.is_parent, i.is_dir, i.path.clone()))
    }

    fn go(&mut self, d: isize) {
        if self.items.is_empty() {
            return;
        }
        let n = step(self.state.selected().unwrap_or(0), self.items.len(), d);
        self.state.select(Some(n));
    }

    fn up(&mut self) {
        if let Some(parent) = self.dir.parent().map(Path::to_path_buf) {
            let from = std::mem::replace(&mut self.dir, parent);
            self.refresh(Some(&from));
        }
    }

    fn enter(&mut self) {
        let Some((is_parent, is_dir, path)) = self.cur() else { return };
        if is_parent {
            self.up();
        } else if is_dir {
            self.dir = path;
            self.refresh(None);
        } else {
            self.toggle();
        }
    }

    fn toggle(&mut self) {
        let Some((is_parent, _, path)) = self.cur() else { return };
        if is_parent {
            return;
        }
        if !self.selected.remove(&path) {
            self.selected.insert(path);
        }
    }

    fn toggle_all_here(&mut self) {
        let files: Vec<PathBuf> = self
            .items
            .iter()
            .filter(|i| !i.is_dir)
            .map(|i| i.path.clone())
            .collect();
        if files.is_empty() {
            return;
        }
        let all = files.iter().all(|f| self.selected.contains(f));
        for f in files {
            if all {
                self.selected.remove(&f);
            } else {
                self.selected.insert(f);
            }
        }
    }

    /// O item está dentro de uma pasta que foi marcada?
    fn covered(&self, p: &Path) -> bool {
        self.selected.iter().any(|s| s != p && p.starts_with(s))
    }
}

fn picker_ui(f: &mut Frame, p: &mut Picker) {
    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .split(f.area());

    f.render_widget(
        Paragraph::new(p.dir.display().to_string()).block(
            Block::bordered()
                .title(" Escolher arquivos / pastas JSON ")
                .border_style(Style::new().fg(Color::Yellow)),
        ),
        rows[0],
    );

    let items: Vec<ListItem> = p
        .items
        .iter()
        .map(|i| {
            let (mark, mark_style) = if i.is_parent {
                ("   ", Style::new())
            } else if p.selected.contains(&i.path) {
                ("[x]", Style::new().fg(Color::Green).add_modifier(Modifier::BOLD))
            } else if p.covered(&i.path) {
                ("[·]", Style::new().fg(Color::Green))
            } else {
                ("[ ]", Style::new().fg(Color::DarkGray))
            };
            let name_style = if i.is_dir {
                Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{mark} "), mark_style),
                Span::styled(i.name.clone(), name_style),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(Block::bordered().border_style(border(true)))
        .highlight_style(hl());
    f.render_stateful_widget(list, rows[1], &mut p.state);

    let help = Line::from(Span::styled(
        " ↑↓ mover │ Enter abrir pasta/marcar │ Espaço marcar │ ←/Backspace subir │ a todos daqui │ x limpar │ . ocultos │ d concluir │ Esc cancelar",
        Style::new().fg(Color::DarkGray),
    ));
    let info = Line::from(vec![
        Span::styled(
            format!(" {} marcado(s)  ", p.selected.len()),
            Style::new().fg(Color::Green),
        ),
        Span::styled(p.msg.as_str(), Style::new().fg(Color::Red)),
    ]);
    f.render_widget(Paragraph::new(vec![help, info]), rows[2]);
}

/// Retorna Some(seleção) ao concluir, None se o usuário cancelar.
fn pick_files(
    terminal: &mut DefaultTerminal,
    start: PathBuf,
    pre: Vec<PathBuf>,
) -> Result<Option<Vec<PathBuf>>> {
    let mut p = Picker::new(start, pre);
    loop {
        terminal.draw(|f| picker_ui(f, &mut p))?;
        let Event::Key(k) = event::read()? else { continue };
        if k.kind != KeyEventKind::Press {
            continue;
        }
        p.msg.clear();
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Char('c') if ctrl => return Ok(None),
            KeyCode::Esc | KeyCode::Char('q') => return Ok(None),
            KeyCode::Down | KeyCode::Char('j') => p.go(1),
            KeyCode::Up | KeyCode::Char('k') => p.go(-1),
            KeyCode::PageDown => p.go(10),
            KeyCode::PageUp => p.go(-10),
            KeyCode::Home | KeyCode::Char('g') => p.go(isize::MIN),
            KeyCode::End | KeyCode::Char('G') => p.go(isize::MAX),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => p.enter(),
            KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') => p.up(),
            KeyCode::Char(' ') => p.toggle(),
            KeyCode::Char('a') => p.toggle_all_here(),
            KeyCode::Char('x') => p.selected.clear(),
            KeyCode::Char('.') => {
                p.hidden = !p.hidden;
                p.refresh(None);
            }
            KeyCode::Char('d') => {
                if p.selected.is_empty() {
                    p.msg = "Marque ao menos um arquivo ou pasta com Espaço.".into();
                } else {
                    return Ok(Some(p.selected.into_iter().collect()));
                }
            }
            _ => {}
        }
    }
}

// ───────────────────────── Loop principal ─────────────────────────

fn summary(entries: usize, files: usize, errors: usize) -> String {
    let mut s = format!("{entries} entradas de {} arquivos", files - errors);
    if errors > 0 {
        s.push_str(&format!(" ({errors} ignorados)"));
    }
    s
}

/// Abre o seletor de dentro do app e recarrega os dados.
fn reselect(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    let start = start_dir_for(&app.selection);
    if let Some(sel) = pick_files(terminal, start, app.selection.clone())? {
        let save_err = save_selection(&sel).err();
        let (entries, errors, nfiles) = load_with_screen(terminal, &sel)?;
        let mut status = summary(entries.len(), nfiles, errors.len());
        if let Some(e) = save_err {
            status.push_str(&format!(" — aviso: não salvei a seleção: {e}"));
        }
        app.selection = sel;
        app.set_entries(entries, errors);
        app.status = status;
    }
    Ok(())
}

fn run(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    while !app.quit {
        if app.want_picker {
            app.want_picker = false;
            reselect(terminal, app)?;
        }
        terminal.draw(|f| ui(f, app))?;
        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    app.on_key(k);
                }
            }
        }
    }
    Ok(())
}

/// Devolve a lista de arquivos ignorados (para imprimir depois que o terminal for restaurado).
fn start(
    terminal: &mut DefaultTerminal,
    mut selection: Vec<PathBuf>,
    need_pick: bool,
    from_args: bool,
) -> Result<Vec<String>> {
    let mut changed = from_args;
    if need_pick {
        match pick_files(terminal, start_dir_for(&selection), selection.clone())? {
            Some(sel) => {
                selection = sel;
                changed = true;
            }
            None if selection.is_empty() => return Ok(vec![]),
            None => {}
        }
    }
    let save_err = if changed { save_selection(&selection).err() } else { None };

    let (entries, errors, nfiles) = load_with_screen(terminal, &selection)?;
    let mut status = summary(entries.len(), nfiles, errors.len());
    if let Some(e) = save_err {
        status.push_str(&format!(" — aviso: não salvei a seleção: {e}"));
    }
    let mut app = App::new(entries, errors, selection);
    app.status = status;
    run(terminal, &mut app)?;
    Ok(app.errors)
}

fn print_help() {
    println!(
        "dlbrowse-hydra — navegador leve de fontes JSON do Hydra no terminal

USO:
  dlbrowse-hydra            abre usando a seleção lembrada (ou o seletor na 1ª vez)
  dlbrowse-hydra <arq|pasta>... usa esses caminhos e passa a lembrar deles
  dlbrowse-hydra --select      abre o seletor de arquivos antes de iniciar
  dlbrowse-hydra --clear       apaga a memória e abre o seletor
  dlbrowse-hydra --help

Dentro do app, a tecla 'f' reabre o seletor.
Memória salva em: {}",
        config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(indisponível)".into())
    );
}

fn main() -> Result<()> {
    let mut clear = false;
    let mut force_pick = false;
    let mut paths: Vec<PathBuf> = vec![];
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--clear" | "--reset" => clear = true,
            "--select" | "-s" => force_pick = true,
            "-h" | "--help" => {
                print_help();
                return Ok(());
            }
            s if s.starts_with('-') => bail!("Opção desconhecida: {s} (use --help)"),
            _ => paths.push(PathBuf::from(a)),
        }
    }

    if clear {
        if clear_saved()? {
            eprintln!("Memória de seleção apagada.");
        } else {
            eprintln!("Não havia nada salvo.");
        }
    }

    let paths: Vec<PathBuf> = paths
        .into_iter()
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .collect();
    for p in &paths {
        if !p.exists() {
            bail!("Não encontrado: {}", p.display());
        }
    }

    let from_args = !paths.is_empty();
    let selection = if from_args {
        paths
    } else if clear {
        vec![]
    } else {
        load_saved()
    };
    let need_pick = selection.is_empty() || force_pick;

    let mut terminal = ratatui::init();
    let res = start(&mut terminal, selection, need_pick, from_args);
    ratatui::restore();

    for e in res? {
        eprintln!("Ignorado -> {e}");
    }
    Ok(())
}