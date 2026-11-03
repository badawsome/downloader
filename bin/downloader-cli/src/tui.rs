//! Full screen search picker (`dc search` in a capable terminal).
//!
//! Keys act immediately and the screen is redrawn in place on the
//! alternate screen, so the scrollback stays clean. Terminals that cannot
//! do this get the line based fallback in `search.rs` (`--plain`).

use std::sync::Arc;
use std::time::Duration;

use bili_service::bili::{Client, SearchItem, SearchOptions, SearchOrder, SearchPage};
use bili_service::naming::format_date;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, HighlightSpacing, Paragraph, Row, Table, TableState};
use ratatui::{DefaultTerminal, Frame};
use tokio::sync::mpsc;

use crate::search::{fmt_count, fmt_secs};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const ORDERS: [SearchOrder; 5] = [
    SearchOrder::Relevance,
    SearchOrder::Plays,
    SearchOrder::Newest,
    SearchOrder::Favorites,
    SearchOrder::Danmaku,
];

fn order_label(o: SearchOrder) -> &'static str {
    match o {
        SearchOrder::Relevance => "relevance",
        SearchOrder::Plays => "most played",
        SearchOrder::Newest => "newest",
        SearchOrder::Favorites => "most favorited",
        SearchOrder::Danmaku => "most danmaku",
    }
}

type Reply = (u64, Result<SearchPage, String>);

struct App {
    client: Arc<Client>,
    keyword: String,
    opts: SearchOptions,
    page: Option<SearchPage>,
    table: TableState,
    /// Picked results, in the order they were picked (kept across pages).
    marked: Vec<SearchItem>,
    /// Some while the query is being edited.
    input: Option<String>,
    /// The pre-filled query is replaced by the first typed character.
    replace_input: bool,
    loading: bool,
    error: Option<String>,
    /// Id of the newest request; older replies are ignored.
    request: u64,
    spinner: usize,
    tx: mpsc::UnboundedSender<Reply>,
}

enum Action {
    Continue,
    Quit,
    Download(Vec<SearchItem>),
}

impl App {
    fn fetch(&mut self) {
        if self.keyword.trim().is_empty() {
            return;
        }
        self.request += 1;
        self.loading = true;
        self.error = None;
        let (id, client, keyword, opts, tx) = (
            self.request,
            self.client.clone(),
            self.keyword.clone(),
            self.opts.clone(),
            self.tx.clone(),
        );
        tokio::spawn(async move {
            let res = client
                .search_videos(&keyword, &opts)
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send((id, res));
        });
    }

    fn on_reply(&mut self, (id, res): Reply) {
        if id != self.request {
            return;
        }
        self.loading = false;
        match res {
            Ok(page) => {
                self.table.select((!page.items.is_empty()).then_some(0));
                self.page = Some(page);
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn items(&self) -> &[SearchItem] {
        self.page
            .as_ref()
            .map(|p| p.items.as_slice())
            .unwrap_or(&[])
    }

    fn is_marked(&self, item: &SearchItem) -> bool {
        self.marked.iter().any(|m| m.bvid == item.bvid)
    }

    fn toggle(&mut self, item: &SearchItem) {
        match self.marked.iter().position(|m| m.bvid == item.bvid) {
            Some(i) => {
                self.marked.remove(i);
            }
            None => self.marked.push(item.clone()),
        }
    }

    fn move_by(&mut self, delta: isize) {
        let n = self.items().len();
        if n == 0 {
            return;
        }
        let cur = self.table.selected().unwrap_or(0) as isize;
        self.table
            .select(Some((cur + delta).clamp(0, n as isize - 1) as usize));
    }

    fn turn_page(&mut self, delta: i32) {
        let Some(page) = &self.page else { return };
        let target = page.page as i32 + delta;
        if target >= 1 && target as u32 <= page.pages.max(1) {
            self.opts.page = target as u32;
            self.fetch();
        }
    }

    fn on_key(&mut self, key: KeyEvent) -> Action {
        // Windows reports key releases too.
        if key.kind != KeyEventKind::Press {
            return Action::Continue;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            return Action::Quit;
        }

        if let Some(input) = &mut self.input {
            match key.code {
                KeyCode::Enter => {
                    let text = input.trim().to_owned();
                    self.input = None;
                    if !text.is_empty() {
                        self.keyword = text;
                        self.opts.page = 1;
                        self.fetch();
                    }
                }
                KeyCode::Esc => {
                    self.input = None;
                    if self.keyword.is_empty() {
                        return Action::Quit;
                    }
                }
                KeyCode::Backspace => {
                    input.pop();
                    self.replace_input = false;
                }
                KeyCode::Char('u') if ctrl => input.clear(),
                KeyCode::Char(c) if !ctrl => {
                    if std::mem::take(&mut self.replace_input) {
                        input.clear();
                    }
                    input.push(c);
                }
                KeyCode::Left | KeyCode::Right | KeyCode::End => self.replace_input = false,
                _ => {}
            }
            return Action::Continue;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Action::Quit,
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::PageUp => self.move_by(-10),
            KeyCode::PageDown => self.move_by(10),
            KeyCode::Home | KeyCode::Char('g') => self.move_by(isize::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => self.move_by(isize::MAX / 2),
            KeyCode::Right | KeyCode::Char('n') | KeyCode::Char('l') => self.turn_page(1),
            KeyCode::Left | KeyCode::Char('p') | KeyCode::Char('h') => self.turn_page(-1),
            KeyCode::Char(' ') => {
                if let Some(item) = self
                    .table
                    .selected()
                    .and_then(|i| self.items().get(i))
                    .cloned()
                {
                    self.toggle(&item);
                    self.move_by(1);
                }
            }
            KeyCode::Char('a') => {
                let items = self.items().to_vec();
                let all = !items.is_empty() && items.iter().all(|i| self.is_marked(i));
                for item in &items {
                    if all == self.is_marked(item) {
                        self.toggle(item);
                    }
                }
            }
            KeyCode::Char('/') | KeyCode::Char('s') => {
                self.input = Some(self.keyword.clone());
                self.replace_input = !self.keyword.is_empty();
            }
            KeyCode::Char('o') => {
                let i = ORDERS
                    .iter()
                    .position(|o| *o == self.opts.order)
                    .unwrap_or(0);
                self.opts.order = ORDERS[(i + 1) % ORDERS.len()];
                self.opts.page = 1;
                self.fetch();
            }
            KeyCode::Char('m') => {
                self.opts.music_only = !self.opts.music_only;
                self.opts.page = 1;
                self.fetch();
            }
            KeyCode::Char('r') => self.fetch(),
            KeyCode::Enter => {
                if !self.marked.is_empty() {
                    return Action::Download(self.marked.clone());
                }
                if let Some(item) = self.table.selected().and_then(|i| self.items().get(i)) {
                    return Action::Download(vec![item.clone()]);
                }
            }
            _ => {}
        }
        Action::Continue
    }

    // ---------------------------------------------------------------- drawing

    fn draw(&mut self, frame: &mut Frame) {
        let [top, body, status, help] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        self.draw_query(frame, top);
        self.draw_table(frame, body);
        self.draw_status(frame, status);
        self.draw_help(frame, help);
    }

    fn draw_query(&self, frame: &mut Frame, area: Rect) {
        let label = Span::styled(" Search › ", Style::new().cyan().bold());
        let filters = format!(
            "{}{} ",
            order_label(self.opts.order),
            if self.opts.music_only {
                " · music only"
            } else {
                ""
            }
        );
        let line = match &self.input {
            Some(text) => {
                let x = area.x + label.width() as u16 + console::measure_text_width(text) as u16;
                frame.set_cursor_position((x.min(area.right().saturating_sub(1)), area.y));
                let text = if self.replace_input {
                    // pre-filled: typing replaces it
                    Span::styled(text.clone(), Style::new().add_modifier(Modifier::REVERSED))
                } else {
                    Span::raw(text.clone())
                };
                Line::from(vec![label, text])
            }
            None => Line::from(vec![
                label,
                Span::styled(self.keyword.clone(), Style::new().bold()),
            ]),
        };
        frame.render_widget(Paragraph::new(line), area);
        frame.render_widget(
            Paragraph::new(Line::from(filters).dim().right_aligned()),
            Rect { height: 1, ..area },
        );
    }

    fn draw_table(&mut self, frame: &mut Frame, area: Rect) {
        let items = self.items().to_vec();
        if items.is_empty() {
            let msg = if self.loading {
                "searching…"
            } else if self.keyword.is_empty() {
                "type a keyword and press Enter"
            } else if self.error.is_some() {
                "search failed — press r to retry"
            } else {
                "no results"
            };
            frame.render_widget(Paragraph::new(msg).dim().centered(), area);
            return;
        }
        let wide = area.width >= 100;
        let mid = area.width >= 70;
        let mut widths = vec![
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Fill(1),
        ];
        let mut header = vec!["", "#", "title"];
        if mid {
            widths.extend([Constraint::Length(14), Constraint::Length(8)]);
            header.extend(["UP", "  length"]);
        }
        widths.push(Constraint::Length(7));
        header.push("  plays");
        if wide {
            widths.push(Constraint::Length(10));
            header.push("date");
        }
        let rows = items.iter().enumerate().map(|(i, item)| {
            let mark = if self.is_marked(item) {
                Span::styled("●", Style::new().green().bold())
            } else {
                Span::raw(" ")
            };
            let title: Vec<Span> = item
                .title_parts
                .iter()
                .map(|(text, hit)| {
                    if *hit {
                        Span::styled(text.clone(), Style::new().yellow().bold())
                    } else {
                        Span::raw(text.clone())
                    }
                })
                .collect();
            let mut cells = vec![
                Cell::from(mark),
                Cell::from(Line::from(format!("{}", i + 1)).right_aligned().cyan()),
                Cell::from(Line::from(title)),
            ];
            if mid {
                cells.push(Cell::from(item.author.clone()));
                cells.push(Cell::from(
                    Line::from(fmt_secs(item.duration_secs)).right_aligned(),
                ));
            }
            cells.push(Cell::from(
                Line::from(fmt_count(item.plays)).right_aligned(),
            ));
            if wide {
                cells.push(Cell::from(format_date(item.pubdate).unwrap_or_default()).dim());
            }
            Row::new(cells)
        });
        let table = Table::new(rows, widths)
            .header(
                Row::new(header)
                    .style(Style::new().dim().bold())
                    .bottom_margin(0),
            )
            .column_spacing(2)
            .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .highlight_symbol("›")
            .highlight_spacing(HighlightSpacing::Always);
        frame.render_stateful_widget(table, area, &mut self.table);
    }

    fn draw_status(&self, frame: &mut Frame, area: Rect) {
        let mut left = vec![Span::raw(" ")];
        if let Some(p) = &self.page {
            left.push(Span::raw(format!(
                "page {}/{} · {} results",
                p.page,
                p.pages.max(1),
                p.total
            )));
        }
        if !self.marked.is_empty() {
            left.push(Span::raw(" · "));
            left.push(Span::styled(
                format!("{} selected", self.marked.len()),
                Style::new().green().bold(),
            ));
        }
        frame.render_widget(Paragraph::new(Line::from(left)), area);
        let right = if self.loading {
            Line::from(format!(
                "{} searching… ",
                SPINNER[self.spinner % SPINNER.len()]
            ))
            .yellow()
        } else if let Some(e) = &self.error {
            Line::from(format!("{e} ")).red()
        } else {
            Line::default()
        };
        frame.render_widget(Paragraph::new(right.right_aligned()), area);
    }

    fn draw_help(&self, frame: &mut Frame, area: Rect) {
        let keys: &[(&str, &str)] = if self.input.is_some() {
            &[("enter", "search"), ("esc", "cancel"), ("ctrl-u", "clear")]
        } else {
            &[
                ("↑↓", "move"),
                ("space", "select"),
                ("a", "all"),
                ("←→", "page"),
                ("/", "search"),
                ("o", "order"),
                ("m", "music"),
                ("enter", "download"),
                ("q", "quit"),
            ]
        };
        let mut spans = vec![Span::raw(" ")];
        for (k, v) in keys {
            spans.push(Span::styled(*k, Style::new().fg(Color::Cyan).bold()));
            spans.push(Span::styled(format!(" {v}  "), Style::new().dim()));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }
}

async fn run(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    mut rx: mpsc::UnboundedReceiver<Reply>,
) -> anyhow::Result<Vec<SearchItem>> {
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    loop {
        terminal.draw(|f| app.draw(f))?;
        tokio::select! {
            ev = events.next() => match ev {
                Some(Ok(Event::Key(key))) => match app.on_key(key) {
                    Action::Continue => {}
                    Action::Quit => return Ok(Vec::new()),
                    Action::Download(items) => return Ok(items),
                },
                Some(Ok(_)) => {} // resize etc.: redrawn on the next loop
                Some(Err(e)) => return Err(e.into()),
                None => return Ok(Vec::new()),
            },
            Some(reply) = rx.recv() => app.on_reply(reply),
            _ = tick.tick(), if app.loading => app.spinner += 1,
        }
    }
}

/// Runs the picker. Returns `Ok(None)` when the terminal cannot host it
/// (the caller then falls back to the line based mode).
pub async fn pick(
    client: Arc<Client>,
    keyword: &str,
    opts: SearchOptions,
) -> anyhow::Result<Option<Vec<SearchItem>>> {
    let Ok(mut terminal) = ratatui::try_init() else {
        let _ = ratatui::try_restore();
        return Ok(None);
    };
    let (tx, rx) = mpsc::unbounded_channel();
    let mut table = TableState::default();
    table.select(Some(0));
    let mut app = App {
        client,
        keyword: keyword.trim().to_owned(),
        opts,
        page: None,
        table,
        marked: Vec::new(),
        input: keyword.trim().is_empty().then(String::new),
        replace_input: false,
        loading: false,
        error: None,
        request: 0,
        spinner: 0,
        tx,
    };
    app.fetch();
    let res = run(&mut terminal, &mut app, rx).await;
    ratatui::restore();
    res.map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn item(bvid: &str, title: &str) -> SearchItem {
        SearchItem {
            aid: 1,
            bvid: bvid.into(),
            title: title.into(),
            title_parts: vec![(title.into(), false)],
            author: "UP主".into(),
            mid: 1,
            duration_secs: 238,
            plays: 5_369_393,
            pubdate: 1_719_658_800,
            category: String::new(),
        }
    }

    fn app() -> App {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut table = TableState::default();
        table.select(Some(0));
        App {
            client: Arc::new(Client::new(Default::default()).unwrap()),
            keyword: "月光".into(),
            opts: SearchOptions {
                page: 1,
                ..Default::default()
            },
            page: Some(SearchPage {
                page: 1,
                pages: 3,
                total: 60,
                items: vec![
                    item("BV1", "一样的月光"),
                    item("BV2", "第二首"),
                    item("BV3", "third"),
                ],
            }),
            table,
            marked: Vec::new(),
            input: None,
            replace_input: false,
            loading: false,
            error: None,
            request: 0,
            spinner: 0,
            tx,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn marking_and_confirming() {
        let mut a = app();
        assert!(matches!(
            a.on_key(key(KeyCode::Char(' '))),
            Action::Continue
        ));
        assert_eq!(a.table.selected(), Some(1), "space moves down");
        a.on_key(key(KeyCode::Down));
        a.on_key(key(KeyCode::Char(' ')));
        assert_eq!(
            a.marked.iter().map(|m| m.bvid.as_str()).collect::<Vec<_>>(),
            ["BV1", "BV3"]
        );
        match a.on_key(key(KeyCode::Enter)) {
            Action::Download(items) => assert_eq!(items.len(), 2),
            _ => panic!("enter should confirm"),
        }
        // `a` toggles the whole page
        a.on_key(key(KeyCode::Char('a')));
        assert_eq!(a.marked.len(), 3);
        a.on_key(key(KeyCode::Char('a')));
        assert!(a.marked.is_empty());
        // enter without marks downloads the highlighted row
        a.on_key(key(KeyCode::Up));
        match a.on_key(key(KeyCode::Enter)) {
            Action::Download(items) => assert_eq!(items[0].bvid, "BV2"),
            _ => panic!(),
        }
        assert!(matches!(a.on_key(key(KeyCode::Char('q'))), Action::Quit));
    }

    #[test]
    fn query_editing() {
        let mut a = app();
        a.on_key(key(KeyCode::Char('/')));
        assert_eq!(a.input.as_deref(), Some("月光"), "pre-filled");
        for c in "晴天".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        assert_eq!(
            a.input.as_deref(),
            Some("晴天"),
            "typing replaced the old keyword"
        );
        // q is text while editing, not quit
        assert!(matches!(
            a.on_key(key(KeyCode::Char('q'))),
            Action::Continue
        ));
        a.on_key(key(KeyCode::Backspace));
        assert_eq!(a.input.as_deref(), Some("晴天"));
        a.on_key(key(KeyCode::Esc));
        assert!(a.input.is_none());
        assert_eq!(a.keyword, "月光", "esc keeps the previous keyword");
        // backspace edits the pre-filled keyword instead
        a.on_key(key(KeyCode::Char('/')));
        a.on_key(key(KeyCode::Backspace));
        a.on_key(key(KeyCode::Char('亮')));
        assert_eq!(a.input.as_deref(), Some("月亮"));
    }

    #[test]
    fn renders_cjk_rows() {
        let mut a = app();
        a.marked.push(item("BV1", "一样的月光"));
        let mut term = Terminal::new(TestBackend::new(100, 10)).unwrap();
        term.draw(|f| a.draw(f)).unwrap();
        let screen = format!("{:?}", term.backend().buffer());
        assert!(screen.contains("Search › "));
        assert!(screen.contains("536.9万"));
        assert!(screen.contains("1 selected"));
        assert!(screen.contains("page 1/3 · 60 results"));
    }
}
