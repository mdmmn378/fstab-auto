//! Top-level application state: tabs, the Mounts list, confirmations.

use std::path::PathBuf;
use std::sync::Arc;

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::wizard::{Signal, Wizard};
use crate::system::mounts::{self, ShareStatus};
use crate::system::{InvokingUser, SysCtx, ops};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Wizard,
    Mounts,
    Help,
}

pub const TABS: [(Tab, &str); 3] = [
    (Tab::Wizard, "New mount"),
    (Tab::Mounts, "Mounts"),
    (Tab::Help, "Help"),
];

/// What a confirmed dialog does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfirmAction {
    CreateDir(PathBuf),
    Unmount(String),
    RemovePermanent(String),
}

pub struct Confirm {
    pub title: String,
    pub message: String,
    pub yes_selected: bool,
    pub action: ConfirmAction,
}

/// Outcome of the last action in the Mounts tab.
pub struct Status {
    pub ok: bool,
    pub text: String,
}

pub struct App {
    pub ctx: Arc<SysCtx>,
    pub tab: Tab,
    pub wizard: Wizard,
    pub shares: Vec<ShareStatus>,
    pub selected: usize,
    pub status: Option<Status>,
    pub confirm: Option<Confirm>,
    pub should_quit: bool,
    /// Advances on every idle tick; drives the spinner.
    pub ticks: usize,
}

impl App {
    pub fn new(ctx: SysCtx, user: InvokingUser) -> Self {
        let ctx = Arc::new(ctx);
        let mut app = Self {
            wizard: Wizard::new(Arc::clone(&ctx), user),
            ctx,
            tab: Tab::Wizard,
            shares: Vec::new(),
            selected: 0,
            status: None,
            confirm: None,
            should_quit: false,
            ticks: 0,
        };
        app.refresh();
        app.status = None;
        app
    }

    /// Called when no input arrived for a tick.
    pub fn tick(&mut self) {
        self.ticks = self.ticks.wrapping_add(1);
        if self.wizard.poll() {
            self.refresh();
        }
    }

    pub fn refresh(&mut self) {
        match mounts::inventory(&self.ctx) {
            Ok(shares) => self.shares = shares,
            Err(text) => self.status = Some(Status { ok: false, text }),
        }
        self.selected = self.selected.min(self.shares.len().saturating_sub(1));
    }

    pub fn handle_event(&mut self, event: Event) {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => self.handle_key(key),
            Event::Paste(text) if self.confirm.is_none() && self.tab == Tab::Wizard => {
                self.wizard.paste(&text);
            }
            _ => {}
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return;
        }
        if self.confirm.is_some() {
            self.handle_confirm_key(key);
            return;
        }
        match key.code {
            KeyCode::Tab => return self.switch_tab(1),
            KeyCode::BackTab => return self.switch_tab(-1),
            _ => {}
        }
        let typing = self.tab == Tab::Wizard && self.wizard.is_typing();
        if key.code == KeyCode::Char('q') && !typing {
            self.should_quit = true;
            return;
        }
        match self.tab {
            Tab::Wizard => {
                if let Signal::AskCreateDir(dir) = self.wizard.handle_key(key) {
                    self.confirm = Some(Confirm {
                        title: "Create directory?".into(),
                        message: format!(
                            "{} does not exist yet.\nCreate it when mounting?",
                            dir.display()
                        ),
                        yes_selected: true,
                        action: ConfirmAction::CreateDir(dir),
                    });
                }
            }
            Tab::Mounts => self.handle_mounts_key(key),
            Tab::Help => {}
        }
    }

    fn switch_tab(&mut self, delta: isize) {
        let at = TABS.iter().position(|(t, _)| *t == self.tab).unwrap_or(0) as isize;
        let next = (at + delta).rem_euclid(TABS.len() as isize) as usize;
        self.tab = TABS[next].0;
        if self.tab == Tab::Mounts {
            self.refresh();
        }
    }

    fn handle_confirm_key(&mut self, key: KeyEvent) {
        let Some(confirm) = &mut self.confirm else {
            return;
        };
        let answer = match key.code {
            KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
                confirm.yes_selected = !confirm.yes_selected;
                return;
            }
            KeyCode::Char('h') => {
                confirm.yes_selected = true;
                return;
            }
            KeyCode::Char('l') => {
                confirm.yes_selected = false;
                return;
            }
            KeyCode::Enter => confirm.yes_selected,
            KeyCode::Char('y' | 'Y') => true,
            KeyCode::Char('n' | 'N') | KeyCode::Esc => false,
            _ => return,
        };
        let Some(confirm) = self.confirm.take() else {
            return;
        };
        match confirm.action {
            ConfirmAction::CreateDir(dir) => self.wizard.answer_create_dir(dir, answer),
            ConfirmAction::Unmount(dir) if answer => self.run_action(ops::unmount, &dir),
            ConfirmAction::RemovePermanent(dir) if answer => {
                self.run_action(ops::remove_permanent, &dir);
            }
            ConfirmAction::Unmount(_) | ConfirmAction::RemovePermanent(_) => {}
        }
    }

    fn run_action(&mut self, action: fn(&SysCtx, &str) -> Result<String, String>, dir: &str) {
        let result = action(&self.ctx, dir);
        self.refresh();
        self.status = Some(match result {
            Ok(text) => Status { ok: true, text },
            Err(text) => Status { ok: false, text },
        });
    }

    fn handle_mounts_key(&mut self, key: KeyEvent) {
        let count = self.shares.len();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if count > 0 => {
                self.selected = (self.selected + count - 1) % count;
            }
            KeyCode::Down | KeyCode::Char('j') if count > 0 => {
                self.selected = (self.selected + 1) % count;
            }
            KeyCode::Char('r') => {
                self.status = None;
                self.refresh();
            }
            KeyCode::Char('n') => self.tab = Tab::Wizard,
            _ => {}
        }
        let Some(share) = self.shares.get(self.selected).cloned() else {
            return;
        };
        match key.code {
            KeyCode::Char('u') if share.mounted => {
                self.confirm = Some(Confirm {
                    title: "Unmount?".into(),
                    message: format!(
                        "Unmount {} from {}?{}",
                        share.source,
                        share.target,
                        if share.in_fstab {
                            "\nIt stays in fstab and will be mounted again on the next boot."
                        } else {
                            ""
                        }
                    ),
                    yes_selected: true,
                    action: ConfirmAction::Unmount(share.target),
                });
            }
            KeyCode::Char('u') => self.say(false, format!("{} is not mounted", share.target)),
            KeyCode::Char('m') if share.mounted => {
                self.say(true, format!("{} is already mounted", share.target));
            }
            KeyCode::Char('m') if share.in_fstab => {
                self.run_action(ops::mount_existing, &share.target);
            }
            KeyCode::Char('d') if share.in_fstab => {
                self.confirm = Some(Confirm {
                    title: "Remove permanent mount?".into(),
                    message: format!(
                        "Remove {} from {}?\n{}The share will no longer be mounted at boot. \
                         A backup of the file is kept.",
                        share.target,
                        self.ctx.fstab.display(),
                        if share.mounted {
                            "It will be unmounted first. "
                        } else {
                            ""
                        }
                    ),
                    yes_selected: false,
                    action: ConfirmAction::RemovePermanent(share.target),
                });
            }
            KeyCode::Char('d') => self.say(
                false,
                format!(
                    "{} is a temporary mount; press u to unmount it",
                    share.target
                ),
            ),
            _ => {}
        }
    }

    fn say(&mut self, ok: bool, text: String) {
        self.status = Some(Status { ok, text });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::fstab::MARKER;
    use std::fs;

    fn key(app: &mut App, code: KeyCode) {
        app.handle_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    fn app(name: &str) -> (App, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("fstab-auto-app-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("fstab"),
            format!("UUID=x / ext4 defaults 0 1\n{MARKER}\n//nas/a /mnt/a cifs guest 0 0\n"),
        )
        .unwrap();
        let mut ctx = SysCtx::new(dir.join("fstab"), dir.join("creds"), false);
        ctx.mounts_file = dir.join("mounts");
        let user = InvokingUser {
            name: "alice".into(),
            uid: 1001,
            gid: 1001,
        };
        (App::new(ctx, user), dir)
    }

    #[test]
    fn tabs_cycle_and_q_only_quits_outside_text_fields() {
        let (mut app, dir) = app("tabs");
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Char('q'));
        assert!(!app.should_quit);
        assert_eq!(app.wizard.host.value(), "q");
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.tab, Tab::Mounts);
        key(&mut app, KeyCode::BackTab);
        key(&mut app, KeyCode::BackTab);
        assert_eq!(app.tab, Tab::Help);
        key(&mut app, KeyCode::Char('q'));
        assert!(app.should_quit);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn removing_a_permanent_mount_requires_confirmation() {
        let (mut app, dir) = app("remove");
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.shares.len(), 1);
        assert!(app.shares[0].managed);

        key(&mut app, KeyCode::Char('d'));
        assert!(app.confirm.is_some());
        key(&mut app, KeyCode::Enter); // default answer is No
        assert_eq!(app.shares.len(), 1);

        key(&mut app, KeyCode::Char('d'));
        key(&mut app, KeyCode::Char('y'));
        assert!(app.shares.is_empty());
        assert!(app.status.as_ref().unwrap().ok);
        assert_eq!(
            fs::read_to_string(dir.join("fstab")).unwrap(),
            "UUID=x / ext4 defaults 0 1\n"
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
