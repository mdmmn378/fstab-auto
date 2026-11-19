//! Rendering. Pure functions of the application state.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Padding, Paragraph, Row, Table, TableState, Wrap,
};

use super::app::{App, Confirm, TABS, Tab};
use super::input::TextInput;
use super::wizard::{LogState, MODES, OPTION_FIELDS, Step, Wizard};
use crate::model::{PROTOCOL_CHOICES, Persistence};
use crate::system::{DEFAULT_CREDENTIALS_DIR, DEFAULT_FSTAB};

pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 22;

const SIDEBAR_WIDTH: u16 = 34;
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

// Only the terminal's own 16 colours, so the UI follows the user's theme.
const ACCENT: Color = Color::Cyan;
const OK: Color = Color::Green;
const ERR: Color = Color::Red;
const WARN: Color = Color::Yellow;

fn dim() -> Style {
    Style::new().fg(Color::DarkGray)
}

fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

fn accent() -> Style {
    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
}

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let message = format!(
            "Terminal too small: {}x{}. fstab-auto needs at least {MIN_WIDTH}x{MIN_HEIGHT}.",
            area.width, area.height
        );
        frame.render_widget(Paragraph::new(message).wrap(Wrap { trim: true }), area);
        return;
    }
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    render_header(frame, header, app);
    match app.tab {
        Tab::Wizard => render_wizard(frame, body, app),
        Tab::Mounts => render_mounts(frame, body, app),
        Tab::Help => render_help(frame, body, app),
    }
    render_footer(frame, footer, app);
    if let Some(confirm) = &app.confirm {
        render_confirm(frame, area, confirm);
    }
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::new().borders(Borders::BOTTOM).border_style(dim());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut left = vec![Span::styled(" fstab-auto ", accent()), Span::raw(" ")];
    for (tab, title) in TABS {
        let style = if tab == app.tab {
            Style::new()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            dim()
        };
        left.push(Span::styled(format!(" {title} "), style));
        left.push(Span::raw(" "));
    }
    let mut right = Vec::new();
    if app.ctx.dry_run {
        right.push(Span::styled(
            " DRY RUN ",
            Style::new()
                .fg(Color::Black)
                .bg(WARN)
                .add_modifier(Modifier::BOLD),
        ));
        right.push(Span::raw(" "));
    }
    right.push(Span::styled(
        format!("v{} ", env!("CARGO_PKG_VERSION")),
        dim(),
    ));
    let right = Line::from(right);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(right.width() as u16)])
            .areas(inner);
    frame.render_widget(Paragraph::new(Line::from(left)), left_area);
    frame.render_widget(Paragraph::new(right), right_area);
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App) {
    let hints: Vec<(&str, &str)> = if app.confirm.is_some() {
        vec![
            ("←/→", "choose"),
            ("Enter", "confirm"),
            ("y/n", "answer"),
            ("Esc", "cancel"),
        ]
    } else {
        match app.tab {
            Tab::Wizard => wizard_hints(&app.wizard),
            Tab::Mounts => vec![
                ("↑/↓", "select"),
                ("u", "unmount"),
                ("m", "mount"),
                ("d", "remove from fstab"),
                ("r", "refresh"),
                ("Tab", "switch tab"),
                ("q", "quit"),
            ],
            Tab::Help => vec![("Tab", "switch tab"), ("q", "quit")],
        }
    };
    let mut spans = vec![Span::raw(" ")];
    for (i, (key, what)) in hints.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ·  ", dim()));
        }
        spans.push(Span::styled(key, bold()));
        spans.push(Span::styled(format!(" {what}"), dim()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn wizard_hints(wizard: &Wizard) -> Vec<(&'static str, &'static str)> {
    let tail = [("Tab", "switch tab"), ("Ctrl+C", "quit")];
    let mut hints = match wizard.step {
        Step::Protocol | Step::Mode => vec![("↑/↓", "choose"), ("Enter", "next"), ("Esc", "back")],
        Step::Password => vec![("Enter", "next"), ("Esc", "back"), ("Ctrl+R", "show/hide")],
        Step::Options => vec![("↑/↓", "field"), ("Enter", "next"), ("Esc", "back")],
        Step::Review => vec![("Enter", "mount now"), ("Esc", "back")],
        Step::Apply => match wizard.outcome {
            None => vec![],
            Some(true) => vec![("Enter", "mount another share")],
            Some(false) => vec![("Enter", "back to review")],
        },
        _ => vec![("Enter", "next"), ("Esc", "back")],
    };
    hints.extend(tail);
    hints
}

// ---------------------------------------------------------------- wizard --

fn render_wizard(frame: &mut Frame, area: Rect, app: &App) {
    let wizard = &app.wizard;
    let [sidebar, main] =
        Layout::horizontal([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(0)]).areas(area);
    render_sidebar(frame, sidebar, wizard);

    let show_preview = wizard.step != Step::Apply;
    let [content, preview] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(if show_preview { 5 } else { 0 }),
    ])
    .areas(main);

    let steps = wizard.visible_steps();
    let number = steps.iter().position(|s| *s == wizard.step).unwrap_or(0) + 1;
    let block = Block::bordered()
        .border_style(dim())
        .title(Line::from(vec![
            Span::styled(format!(" Step {number} of {} ", steps.len()), dim()),
            Span::styled(format!("{} ", wizard.step.title()), accent()),
        ]))
        .padding(Padding::new(2, 2, 1, 0));
    let inner = block.inner(content);
    frame.render_widget(block, content);

    match wizard.step {
        Step::Protocol => render_protocol(frame, inner, wizard),
        Step::Host => render_text_step(
            frame,
            inner,
            wizard,
            &wizard.host,
            "Which NAS or file server do you want to connect to?",
            &[
                "Hostname or IP address, e.g. nas01 or 192.168.1.20.",
                "You can also paste a full path such as //nas/share.",
            ],
        ),
        Step::Share => render_text_step(
            frame,
            inner,
            wizard,
            &wizard.share,
            &format!("Which share on {} should be mounted?", wizard.host.value()),
            &[
                "The share (endpoint) name, e.g. projects.",
                "A sub-folder works too: share/folder.",
            ],
        ),
        Step::Username => render_text_step(
            frame,
            inner,
            wizard,
            &wizard.username,
            "Which account should be used to log in to the NAS?",
            &["Leave empty to connect as guest (no password)."],
        ),
        Step::Password => render_text_step(
            frame,
            inner,
            wizard,
            &wizard.password,
            &format!(
                "Password for {} on {}",
                wizard.username.value(),
                wizard.host.value()
            ),
            &[
                "Typed characters are hidden. Ctrl+R shows or hides them.",
                "The password is never put on a command line or written to logs.",
            ],
        ),
        Step::MountDir => render_text_step(
            frame,
            inner,
            wizard,
            &wizard.mount_dir,
            "Where on this machine should the share appear?",
            &[
                "An absolute directory path, e.g. /mnt/nas.",
                "If it does not exist you will be asked whether to create it.",
            ],
        ),
        Step::Options => render_options(frame, inner, wizard),
        Step::Mode => render_mode(frame, inner, wizard),
        Step::Review => render_review(frame, inner, app),
        Step::Apply => render_apply(frame, inner, app),
    }

    if show_preview {
        let persistent = wizard.persistence() == Persistence::Permanent;
        let title = if persistent {
            " fstab entry "
        } else {
            " Equivalent command "
        };
        let text = match wizard.preview() {
            Some(preview) => Span::raw(preview),
            None => Span::styled(
                "Appears once server, share and mount directory are set.",
                dim(),
            ),
        };
        let block = Block::bordered()
            .border_style(dim())
            .title(Span::styled(title, dim()))
            .padding(Padding::horizontal(1));
        frame.render_widget(
            Paragraph::new(Line::from(text))
                .wrap(Wrap { trim: false })
                .block(block),
            preview,
        );
    }
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        text.to_string()
    } else {
        let head: String = text.chars().take(width.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

fn render_sidebar(frame: &mut Frame, area: Rect, wizard: &Wizard) {
    let block = Block::bordered()
        .border_style(dim())
        .title(Span::styled(" Steps ", dim()))
        .padding(Padding::new(1, 1, 1, 0));
    let inner = block.inner(area);
    let steps = wizard.visible_steps();
    let current = steps.iter().position(|s| *s == wizard.step).unwrap_or(0);
    let value_width = (inner.width as usize).saturating_sub(2 + 13);
    let lines: Vec<Line> = steps
        .iter()
        .enumerate()
        .map(|(i, step)| {
            let title = format!("{:<13}", sidebar_title(*step));
            if i == current {
                Line::from(vec![
                    Span::styled("▸ ", accent()),
                    Span::styled(title, accent()),
                ])
            } else if i < current {
                Line::from(vec![
                    Span::styled("✓ ", Style::new().fg(OK)),
                    Span::raw(title),
                    Span::styled(truncate(&wizard.summary(*step), value_width), dim()),
                ])
            } else {
                Line::from(Span::styled(format!("· {title}"), dim()))
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn sidebar_title(step: Step) -> &'static str {
    match step {
        Step::MountDir => "Mount point",
        other => other.title(),
    }
}

/// Draws a bordered single-line field and places the terminal cursor in it.
fn render_field(frame: &mut Frame, area: Rect, input: &TextInput, masked: bool) {
    let block = Block::bordered()
        .border_style(Style::new().fg(ACCENT))
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    let (text, column) = input.visible(inner.width as usize);
    let text = if masked {
        "•".repeat(text.chars().count())
    } else {
        text
    };
    frame.render_widget(Paragraph::new(text).block(block), area);
    frame.set_cursor_position((inner.x + column as u16, inner.y));
}

fn error_line(wizard: &Wizard) -> Line<'static> {
    match &wizard.error {
        Some(error) => Line::from(Span::styled(format!("✗ {error}"), Style::new().fg(ERR))),
        None => Line::default(),
    }
}

fn render_text_step(
    frame: &mut Frame,
    area: Rect,
    wizard: &Wizard,
    input: &TextInput,
    question: &str,
    hints: &[&str],
) {
    let [question_area, field_area, error_area, hint_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Length(2),
        Constraint::Min(0),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(Span::styled(question.to_string(), bold())).wrap(Wrap { trim: true }),
        question_area,
    );
    let field_area = Rect {
        width: field_area.width.min(56),
        ..field_area
    };
    let masked = wizard.step == Step::Password && !wizard.reveal_password;
    render_field(frame, field_area, input, masked);
    frame.render_widget(
        Paragraph::new(error_line(wizard)).wrap(Wrap { trim: true }),
        error_area,
    );
    let hints: Vec<Line> = hints
        .iter()
        .map(|h| Line::from(Span::styled((*h).to_string(), dim())))
        .collect();
    frame.render_widget(Paragraph::new(hints).wrap(Wrap { trim: true }), hint_area);
}

fn choice_lines(
    selected: bool,
    enabled: bool,
    label: &str,
    description: &str,
) -> [Line<'static>; 2] {
    let (marker, style) = match (selected, enabled) {
        (true, true) => ("▸ ", accent()),
        (true, false) => ("▸ ", dim().add_modifier(Modifier::BOLD)),
        (false, true) => ("  ", Style::new()),
        (false, false) => ("  ", dim()),
    };
    [
        Line::from(vec![
            Span::styled(marker, style),
            Span::styled(label.to_string(), style),
        ]),
        Line::from(Span::styled(format!("    {description}"), dim())),
    ]
}

fn render_protocol(frame: &mut Frame, area: Rect, wizard: &Wizard) {
    let mut lines = vec![
        Line::from(Span::styled("How does the NAS share its files?", bold())),
        Line::default(),
    ];
    for (i, choice) in PROTOCOL_CHOICES.iter().enumerate() {
        lines.extend(choice_lines(
            i == wizard.protocol_idx,
            choice.kind.is_some(),
            choice.label,
            choice.description,
        ));
    }
    lines.push(Line::default());
    lines.push(error_line(wizard));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn render_mode(frame: &mut Frame, area: Rect, wizard: &Wizard) {
    let mut lines = vec![
        Line::from(Span::styled(
            "How long should the share stay mounted?",
            bold(),
        )),
        Line::default(),
    ];
    for (i, (_, label, description)) in MODES.iter().enumerate() {
        lines.extend(choice_lines(i == wizard.mode_idx, true, label, description));
        lines.push(Line::default());
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn render_options(frame: &mut Frame, area: Rect, wizard: &Wizard) {
    const LABEL_WIDTH: u16 = 16;
    let user = wizard.user();
    let who = if user.name.is_empty() {
        format!("uid {}", user.uid)
    } else {
        format!("{}, uid {} gid {}", user.name, user.uid, user.gid)
    };
    let hints = [
        format!(
            "Files on the share appear owned by this user id. Default: you ({who}). Empty: root."
        ),
        format!(
            "Files on the share appear owned by this group id. Default: you ({who}). Empty: root."
        ),
        "Windows / Active Directory domain or workgroup. Usually left empty.".to_string(),
        "Extra mount.cifs options, comma separated, e.g. vers=3.0,ro,file_mode=0664,dir_mode=0775"
            .to_string(),
    ];

    let mut lines = vec![
        Line::from(Span::styled(
            "Fine-tune the mount. The defaults suit most setups.",
            bold(),
        )),
        Line::default(),
    ];
    let first_field_row = lines.len() as u16;
    let value_width = area.width.saturating_sub(2 + LABEL_WIDTH) as usize;
    let mut cursor_column = 0;
    for (i, label) in OPTION_FIELDS.iter().enumerate() {
        let focused = i == wizard.options_focus;
        let (text, column) = wizard.options[i].visible(value_width.saturating_sub(1));
        if focused {
            cursor_column = column as u16;
        }
        let shown = if text.is_empty() && !focused {
            "—".to_string()
        } else {
            text
        };
        lines.push(Line::from(vec![
            Span::styled(if focused { "▸ " } else { "  " }, accent()),
            Span::styled(
                format!("{label:<width$}", width = LABEL_WIDTH as usize),
                if focused { accent() } else { Style::new() },
            ),
            Span::styled(shown, if focused { Style::new() } else { dim() }),
        ]));
    }
    lines.push(Line::default());
    lines.push(error_line(wizard));
    lines.push(Line::from(Span::styled(
        hints[wizard.options_focus].clone(),
        dim(),
    )));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
    frame.set_cursor_position((
        area.x + 2 + LABEL_WIDTH + cursor_column,
        area.y + first_field_row + wizard.options_focus as u16,
    ));
}

fn render_review(frame: &mut Frame, area: Rect, app: &App) {
    let wizard = &app.wizard;
    let row = |label: &str, value: String, note: &str| {
        Line::from(vec![
            Span::styled(format!("{label:<13}"), dim()),
            Span::styled(value, bold()),
            Span::styled(format!("  {note}"), dim()),
        ])
    };
    let [uid, gid, domain, extra] = &wizard.options;
    let or_root = |input: &TextInput| {
        if input.is_empty() {
            "root".to_string()
        } else {
            input.value().to_string()
        }
    };
    let mut lines = vec![
        row("Server", wizard.host.value().to_string(), ""),
        row("Share", wizard.share.value().to_string(), ""),
    ];
    if wizard.is_guest() {
        lines.push(row("Login", "guest".into(), "no password"));
    } else {
        let login = if domain.is_empty() {
            wizard.username.value().to_string()
        } else {
            format!("{}\\{}", domain.value(), wizard.username.value())
        };
        lines.push(row("Login", login, "password hidden"));
    }
    lines.push(row(
        "Mount point",
        wizard.mount_dir.value().to_string(),
        if wizard.will_create_dir() {
            "will be created"
        } else {
            ""
        },
    ));
    lines.push(row(
        "Ownership",
        format!("uid {}, gid {}", or_root(uid), or_root(gid)),
        "",
    ));
    if !extra.is_empty() {
        lines.push(row("Options", extra.value().to_string(), ""));
    }
    lines.push(match wizard.persistence() {
        Persistence::Temporary => row("Persistence", "Temporary".into(), "until reboot or unmount"),
        Persistence::Permanent => row("Persistence", "Permanent".into(), "mounted on every boot"),
    });
    lines.push(Line::default());
    for warning in wizard.warnings() {
        lines.push(Line::from(Span::styled(
            format!("! {warning}"),
            Style::new().fg(WARN),
        )));
    }
    let call_to_action = Line::from(vec![
        Span::raw("Press "),
        Span::styled("Enter", accent()),
        Span::raw(" to mount, or "),
        Span::styled("Esc", bold()),
        Span::raw(" to go back and change an answer."),
    ]);
    // The call to action keeps its own row so long values can never push it
    // out of view.
    let [summary_area, action_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        summary_area,
    );
    frame.render_widget(Paragraph::new(call_to_action), action_area);
}

/// Word-wraps `text` into rows of at most `width` characters, splitting
/// words only when a single one is longer than a row. Doing the wrapping here
/// (instead of in the widget) makes the row count exact, which the log needs
/// in order to keep its newest rows in view.
fn wrap_rows(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = vec![String::new()];
    let mut used = 0;
    for word in text.split(' ') {
        let mut rest: Vec<char> = word.chars().collect();
        if used > 0 && used + 1 + rest.len() > width {
            rows.push(String::new());
            used = 0;
        }
        if used > 0 {
            rows.last_mut().unwrap().push(' ');
            used += 1;
        }
        while used + rest.len() > width {
            let tail = rest.split_off(width - used);
            rows.last_mut().unwrap().extend(rest);
            rows.push(String::new());
            used = 0;
            rest = tail;
        }
        used += rest.len();
        rows.last_mut().unwrap().extend(rest);
    }
    rows
}

fn render_apply(frame: &mut Frame, area: Rect, app: &App) {
    let wizard = &app.wizard;
    let text_width = (area.width as usize).saturating_sub(2).max(1);
    let mut lines: Vec<Line> = Vec::new();
    // First row carries the marker, continuation rows are indented under it.
    let mut push = |marker: &'static str, marker_style: Style, text: &str, style: Style| {
        for (i, row) in wrap_rows(text, text_width).into_iter().enumerate() {
            let lead = if i == 0 { marker } else { " " };
            lines.push(Line::from(vec![
                Span::styled(format!("{lead} "), marker_style),
                Span::styled(row, style),
            ]));
        }
    };
    for entry in &wizard.log {
        let (marker, style) = match entry.state {
            LogState::Running => (SPINNER[app.ticks % SPINNER.len()], accent()),
            LogState::Ok => ("✓", Style::new().fg(OK)),
            LogState::Failed => ("✗", Style::new().fg(ERR)),
            LogState::Warned => ("!", Style::new().fg(WARN)),
            LogState::Note => ("•", dim()),
        };
        push(marker, style, &entry.title, Style::new());
        let detail_style = match entry.state {
            LogState::Failed => Style::new().fg(ERR),
            LogState::Warned => Style::new().fg(WARN),
            _ => dim(),
        };
        for detail in entry.detail.lines().filter(|l| !l.trim().is_empty()) {
            push(" ", style, detail, detail_style);
        }
    }
    let share = format!("//{}/{}", wizard.host.value(), wizard.share.value());
    let verdict = match (wizard.outcome, app.ctx.dry_run) {
        (None, _) => None,
        (Some(true), true) => Some((
            "✓",
            OK,
            "Dry run complete. Nothing on this system was changed.".to_string(),
            "Run without --dry-run to mount the share for real.",
        )),
        (Some(true), false) => Some((
            "✓",
            OK,
            format!("{share} is mounted on {}", wizard.mount_dir.value()),
            match wizard.persistence() {
                Persistence::Temporary => {
                    "It stays mounted until the next reboot or until you unmount it."
                }
                Persistence::Permanent => "It will be mounted again automatically on every boot.",
            },
        )),
        (Some(false), _) => Some((
            "✗",
            ERR,
            "The share was not mounted.".to_string(),
            "Press Enter to go back, adjust your answers and try again.",
        )),
    };
    if let Some((marker, color, headline, advice)) = verdict {
        let strong = Style::new().fg(color).add_modifier(Modifier::BOLD);
        push(" ", strong, "", strong);
        push(marker, strong, &headline, strong);
        push(" ", strong, advice, dim());
    }
    let scroll = lines.len().saturating_sub(area.height as usize) as u16;
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), area);
}

// ---------------------------------------------------------------- mounts --

fn render_mounts(frame: &mut Frame, area: Rect, app: &App) {
    let [table_area, status_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(4)]).areas(area);
    let block = Block::bordered()
        .border_style(dim())
        .title(Span::styled(" Network shares ", accent()))
        .padding(Padding::horizontal(1));

    if app.shares.is_empty() {
        let lines = vec![
            Line::default(),
            Line::from("No CIFS / SMB shares are mounted or configured in fstab."),
            Line::from(Span::styled(
                "Switch to the New mount tab (Tab) to add one.",
                dim(),
            )),
        ];
        frame.render_widget(Paragraph::new(lines).centered().block(block), table_area);
    } else {
        let rows = app.shares.iter().map(|share| {
            let state = if share.mounted {
                Cell::from(Span::styled("● mounted", Style::new().fg(OK)))
            } else {
                Cell::from(Span::styled("○ not mounted", dim()))
            };
            let boot = match (share.in_fstab, share.managed) {
                (true, true) => "permanent",
                (true, false) => "permanent (manual entry)",
                (false, _) => "temporary",
            };
            Row::new(vec![
                Cell::from(share.source.clone()),
                Cell::from(share.target.clone()),
                state,
                Cell::from(boot),
            ])
        });
        let table = Table::new(
            rows,
            [
                Constraint::Fill(3),
                Constraint::Fill(2),
                Constraint::Length(14),
                Constraint::Length(24),
            ],
        )
        .header(Row::new(["Share", "Mount point", "State", "Persistence"]).style(dim()))
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol("▸ ")
        .column_spacing(2)
        .block(block);
        let mut state = TableState::default().with_selected(Some(app.selected));
        frame.render_stateful_widget(table, table_area, &mut state);
    }

    let status_block = Block::bordered()
        .border_style(dim())
        .padding(Padding::horizontal(1));
    let status = match &app.status {
        Some(status) => {
            let (marker, color) = if status.ok { ("✓", OK) } else { ("✗", ERR) };
            Paragraph::new(Span::styled(
                format!("{marker} {}", status.text),
                Style::new().fg(color),
            ))
        }
        None => Paragraph::new(Span::styled(
            "Select a share, then press u to unmount, m to mount, or d to stop mounting it at boot.",
            dim(),
        )),
    };
    frame.render_widget(
        status.wrap(Wrap { trim: true }).block(status_block),
        status_area,
    );
}

// ------------------------------------------------------------------ help --

fn render_help(frame: &mut Frame, area: Rect, app: &App) {
    let heading = |text: &str| Line::from(Span::styled(text.to_string(), accent()));
    let item = |key: &str, text: &str| {
        Line::from(vec![
            Span::styled(format!("  {key:<14}"), bold()),
            Span::raw(text.to_string()),
        ])
    };
    let lines = vec![
        heading("What it does"),
        Line::from(
            "  Mounts a NAS share (CIFS / SMB) on a directory, either until the next reboot",
        ),
        Line::from("  or permanently through fstab, and lets you unmount or remove it again."),
        Line::default(),
        heading("Keys"),
        item(
            "Tab / Shift+Tab",
            " switch between New mount, Mounts and Help",
        ),
        item("Enter", "confirm the current step and continue"),
        item("Esc", "go back one step"),
        item("↑ / ↓", "choose an option, field or share"),
        item("Ctrl+R", "show or hide the password while typing"),
        item("Ctrl+U", "clear the current field"),
        item("q / Ctrl+C", "quit (q only outside text fields)"),
        Line::default(),
        heading("Files"),
        item("fstab", &app.ctx.fstab.display().to_string()),
        item(
            "backup",
            "<fstab>.fstab-auto.bak, written before every change",
        ),
        item(
            "credentials",
            &format!("{}  (root only)", app.ctx.credentials_dir.display()),
        ),
        Line::default(),
        Line::from(Span::styled(
            format!(
                "  Defaults: {DEFAULT_FSTAB} and {DEFAULT_CREDENTIALS_DIR}. Try `fstab-auto --dry-run` to explore safely."
            ),
            dim(),
        )),
    ];
    let block = Block::bordered()
        .border_style(dim())
        .title(Span::styled(" Help ", accent()))
        .padding(Padding::new(2, 2, 1, 0));
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(block),
        area,
    );
}

// ----------------------------------------------------------------- modal --

fn render_confirm(frame: &mut Frame, area: Rect, confirm: &Confirm) {
    let width = area.width.saturating_sub(8).min(64);
    let text_width = width.saturating_sub(6).max(1) as usize;
    let message_rows: usize = confirm
        .message
        .lines()
        .map(|l| l.chars().count().max(1).div_ceil(text_width))
        .sum();
    let height = (message_rows as u16 + 6).min(area.height);
    let [popup] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(popup);

    let block = Block::bordered()
        .border_style(Style::new().fg(ACCENT))
        .title(Span::styled(format!(" {} ", confirm.title), accent()))
        .padding(Padding::new(2, 2, 1, 1));
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);

    let [message_area, buttons_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    frame.render_widget(
        Paragraph::new(confirm.message.clone()).wrap(Wrap { trim: true }),
        message_area,
    );
    let button = |label: &str, selected: bool| {
        let style = if selected {
            Style::new()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            dim()
        };
        Span::styled(format!("  {label}  "), style)
    };
    let buttons = Line::from(vec![
        button("Yes", confirm.yes_selected),
        Span::raw("   "),
        button("No", !confirm.yes_selected),
    ]);
    frame.render_widget(Paragraph::new(buttons).centered(), buttons_area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::{InvokingUser, SysCtx};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    fn app() -> App {
        let dir = std::env::temp_dir().join("fstab-auto-ui-test-missing");
        let mut ctx = SysCtx::new(dir.join("fstab"), dir.join("creds"), true);
        ctx.mounts_file = dir.join("mounts");
        let user = InvokingUser {
            name: "alice".into(),
            uid: 1001,
            gid: 1001,
        };
        App::new(ctx, user)
    }

    fn screen(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..height {
            for x in 0..width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    fn type_text(app: &mut App, text: &str) {
        text.chars().for_each(|c| press(app, KeyCode::Char(c)));
    }

    #[test]
    fn every_screen_renders_at_minimum_size() {
        let mut app = app();
        let first = screen(&app, MIN_WIDTH, MIN_HEIGHT);
        assert!(first.contains("fstab-auto"));
        assert!(first.contains("DRY RUN"));
        assert!(first.contains("Step 1 of 10"));
        assert!(first.contains("CIFS / SMB"));

        press(&mut app, KeyCode::Enter);
        type_text(&mut app, "nas01");
        press(&mut app, KeyCode::Enter);
        type_text(&mut app, "projects");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        type_text(&mut app, "topsecret");
        let password = screen(&app, MIN_WIDTH, MIN_HEIGHT);
        assert!(!password.contains("topsecret"));
        assert!(password.contains("•••••••••"));
        press(&mut app, KeyCode::Enter);

        app.handle_event(Event::Key(KeyEvent::new(
            KeyCode::Char('u'),
            KeyModifiers::CONTROL,
        )));
        type_text(&mut app, "/nonexistent-fstab-auto/nas");
        press(&mut app, KeyCode::Enter);
        let modal = screen(&app, MIN_WIDTH, MIN_HEIGHT);
        assert!(modal.contains("Create directory?"));
        assert!(modal.contains("/nonexistent-fstab-auto/nas does not exist yet."));
        press(&mut app, KeyCode::Char('y'));

        assert!(screen(&app, MIN_WIDTH, MIN_HEIGHT).contains("Owner UID"));
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Down);
        assert!(screen(&app, MIN_WIDTH, MIN_HEIGHT).contains("Permanent"));
        press(&mut app, KeyCode::Enter);

        let review = screen(&app, 120, 30);
        assert!(review.contains("will be created"));
        assert!(review.contains("password hidden"));
        assert!(review.contains("//nas01/projects /nonexistent-fstab-auto/nas cifs"));
        assert!(!review.contains("topsecret"));
        let small_review = screen(&app, MIN_WIDTH, MIN_HEIGHT);
        assert!(
            small_review.contains("Press Enter to mount"),
            "{small_review}"
        );

        press(&mut app, KeyCode::Enter);
        for _ in 0..500 {
            app.tick();
            if app.wizard.outcome.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(screen(&app, MIN_WIDTH, MIN_HEIGHT).contains("Dry run complete."));
        let done = screen(&app, 120, 30);
        assert!(done.contains("Verify the mount"), "{done}");
        assert!(
            done.contains("Run without --dry-run to mount the share for real."),
            "{done}"
        );
        assert!(!done.contains("topsecret"));

        press(&mut app, KeyCode::Tab);
        assert!(screen(&app, MIN_WIDTH, MIN_HEIGHT).contains("No CIFS / SMB shares"));
        press(&mut app, KeyCode::Tab);
        assert!(screen(&app, MIN_WIDTH, MIN_HEIGHT).contains("Ctrl+R"));
    }

    #[test]
    fn wrap_rows_prefers_word_boundaries() {
        assert_eq!(wrap_rows("", 5), [""]);
        assert_eq!(wrap_rows("ab cd ef", 5), ["ab cd", "ef"]);
        assert_eq!(wrap_rows("abcdefgh ij", 5), ["abcde", "fgh", "ij"]);
        assert_eq!(wrap_rows("a abcdefgh", 5), ["a", "abcde", "fgh"]);
    }

    #[test]
    fn tiny_terminal_gets_a_message_instead_of_a_panic() {
        let app = app();
        assert!(screen(&app, 40, 10).contains("Terminal too small"));
        screen(&app, 1, 1);
    }
}
