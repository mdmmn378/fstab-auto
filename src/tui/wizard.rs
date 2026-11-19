//! State and logic of the step-by-step mount wizard (no rendering).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::input::TextInput;
use crate::model::{MountRequest, PROTOCOL_CHOICES, Persistence, ProtocolKind};
use crate::protocol::cifs;
use crate::system::ops::{self, Event};
use crate::system::{InvokingUser, SysCtx};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    Protocol,
    Host,
    Share,
    Username,
    Password,
    MountDir,
    Options,
    Mode,
    Review,
    Apply,
}

pub const STEPS: [Step; 10] = [
    Step::Protocol,
    Step::Host,
    Step::Share,
    Step::Username,
    Step::Password,
    Step::MountDir,
    Step::Options,
    Step::Mode,
    Step::Review,
    Step::Apply,
];

impl Step {
    pub fn title(self) -> &'static str {
        match self {
            Step::Protocol => "Protocol",
            Step::Host => "Server",
            Step::Share => "Share",
            Step::Username => "Username",
            Step::Password => "Password",
            Step::MountDir => "Mount directory",
            Step::Options => "Options",
            Step::Mode => "Persistence",
            Step::Review => "Review",
            Step::Apply => "Mount",
        }
    }
}

/// Fields of the Options step, in display order.
pub const OPTION_FIELDS: [&str; 4] = ["Owner UID", "Owner GID", "Domain", "Extra options"];

pub const MODES: [(Persistence, &str, &str); 2] = [
    (
        Persistence::Temporary,
        "Temporary",
        "Mounted now. Gone after a reboot or when unmounted. Nothing is saved.",
    ),
    (
        Persistence::Permanent,
        "Permanent",
        "Saved to fstab and mounted on every boot. Credentials are stored root-only.",
    ),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LogState {
    Running,
    Ok,
    Failed,
    Warned,
    Note,
}

#[derive(Clone, Debug)]
pub struct LogEntry {
    pub title: String,
    pub state: LogState,
    pub detail: String,
}

/// What the wizard needs from the surrounding app after a key press.
#[derive(Debug, PartialEq, Eq)]
pub enum Signal {
    None,
    /// The chosen mount directory does not exist; ask whether to create it.
    AskCreateDir(PathBuf),
}

pub struct Wizard {
    ctx: Arc<SysCtx>,
    user: InvokingUser,
    pub step: Step,
    pub protocol_idx: usize,
    pub host: TextInput,
    pub share: TextInput,
    pub username: TextInput,
    pub password: TextInput,
    pub mount_dir: TextInput,
    /// uid, gid, domain, extra options: see [`OPTION_FIELDS`].
    pub options: [TextInput; 4],
    pub options_focus: usize,
    pub mode_idx: usize,
    pub reveal_password: bool,
    /// The user typed their own mount directory; stop suggesting one.
    mount_dir_edited: bool,
    /// The user agreed to create this missing directory.
    create_dir: Option<PathBuf>,
    pub error: Option<String>,
    pub log: Vec<LogEntry>,
    /// `None` while running, then whether the mount succeeded.
    pub outcome: Option<bool>,
    progress: Option<Receiver<Event>>,
}

impl Wizard {
    pub fn new(ctx: Arc<SysCtx>, user: InvokingUser) -> Self {
        let username = if user.name == "root" {
            ""
        } else {
            user.name.as_str()
        };
        let options = [
            TextInput::with_value(&user.uid.to_string()),
            TextInput::with_value(&user.gid.to_string()),
            TextInput::default(),
            TextInput::default(),
        ];
        Self {
            ctx,
            username: TextInput::with_value(username),
            user,
            step: Step::Protocol,
            protocol_idx: 0,
            host: TextInput::default(),
            share: TextInput::default(),
            password: TextInput::default(),
            mount_dir: TextInput::default(),
            options,
            options_focus: 0,
            mode_idx: 0,
            reveal_password: false,
            mount_dir_edited: false,
            create_dir: None,
            error: None,
            log: Vec::new(),
            outcome: None,
            progress: None,
        }
    }

    pub fn user(&self) -> &InvokingUser {
        &self.user
    }

    pub fn is_guest(&self) -> bool {
        self.username.value().trim().is_empty()
    }

    pub fn persistence(&self) -> Persistence {
        MODES[self.mode_idx].0
    }

    pub fn is_running(&self) -> bool {
        self.step == Step::Apply && self.outcome.is_none()
    }

    /// Steps that apply to the current answers (guests have no password).
    pub fn visible_steps(&self) -> Vec<Step> {
        STEPS
            .into_iter()
            .filter(|s| !(*s == Step::Password && self.is_guest()))
            .collect()
    }

    /// True when the focused widget is a text field, so letters are input
    /// rather than shortcuts.
    pub fn is_typing(&self) -> bool {
        self.active_input().is_some()
    }

    fn active_input(&self) -> Option<&TextInput> {
        Some(match self.step {
            Step::Host => &self.host,
            Step::Share => &self.share,
            Step::Username => &self.username,
            Step::Password => &self.password,
            Step::MountDir => &self.mount_dir,
            Step::Options => &self.options[self.options_focus],
            _ => return None,
        })
    }

    fn active_input_mut(&mut self) -> Option<&mut TextInput> {
        Some(match self.step {
            Step::Host => &mut self.host,
            Step::Share => &mut self.share,
            Step::Username => &mut self.username,
            Step::Password => &mut self.password,
            Step::MountDir => &mut self.mount_dir,
            Step::Options => &mut self.options[self.options_focus],
            _ => return None,
        })
    }

    pub fn paste(&mut self, text: &str) {
        let step = self.step;
        if let Some(input) = self.active_input_mut() {
            input.insert_str(text.trim_end_matches(['\r', '\n']));
            self.error = None;
            if step == Step::MountDir {
                self.mount_dir_edited = true;
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Signal {
        if self.is_running() {
            return Signal::None;
        }
        match key.code {
            KeyCode::Enter => return self.advance(),
            KeyCode::Esc => {
                self.back();
                return Signal::None;
            }
            _ => {}
        }
        match self.step {
            Step::Protocol => match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.move_protocol(-1),
                KeyCode::Down | KeyCode::Char('j') => self.move_protocol(1),
                _ => {}
            },
            Step::Mode => match key.code {
                KeyCode::Up | KeyCode::Down | KeyCode::Char('j' | 'k') => {
                    self.mode_idx = 1 - self.mode_idx;
                }
                _ => {}
            },
            Step::Options if matches!(key.code, KeyCode::Up | KeyCode::Down) => {
                let n = OPTION_FIELDS.len();
                self.options_focus = match key.code {
                    KeyCode::Up => (self.options_focus + n - 1) % n,
                    _ => (self.options_focus + 1) % n,
                };
            }
            Step::Password
                if key.code == KeyCode::Char('r')
                    && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.reveal_password = !self.reveal_password;
            }
            step => {
                if let Some(input) = self.active_input_mut()
                    && input.handle_key(key)
                {
                    self.error = None;
                    if step == Step::MountDir {
                        self.mount_dir_edited = true;
                    }
                }
            }
        }
        Signal::None
    }

    fn move_protocol(&mut self, delta: isize) {
        let n = PROTOCOL_CHOICES.len() as isize;
        self.protocol_idx = (self.protocol_idx as isize + delta).rem_euclid(n) as usize;
        self.error = None;
    }

    fn go_to(&mut self, step: Step) {
        self.step = step;
        self.error = None;
        if step == Step::MountDir && !self.mount_dir_edited {
            self.mount_dir.set(&self.suggested_mount_dir());
        }
    }

    fn suggested_mount_dir(&self) -> String {
        let share = self.share.value().trim().trim_matches('/');
        let leaf = share.rsplit('/').next().unwrap_or(share);
        format!("/mnt/{leaf}")
    }

    fn neighbour(&self, delta: isize) -> Option<Step> {
        let steps = self.visible_steps();
        let at = steps.iter().position(|s| *s == self.step)? as isize + delta;
        usize::try_from(at).ok().and_then(|i| steps.get(i).copied())
    }

    fn back(&mut self) {
        if self.step == Step::Apply {
            // Only reachable once finished; let the user fix and retry.
            self.go_to(Step::Review);
        } else if let Some(prev) = self.neighbour(-1) {
            self.go_to(prev);
        }
    }

    /// Validates the current step and moves on.
    fn advance(&mut self) -> Signal {
        match self.validate_current() {
            Err(message) => {
                self.error = Some(message);
                return Signal::None;
            }
            Ok(Some(signal)) => return signal,
            Ok(None) => {}
        }
        match self.step {
            Step::Review => self.start(),
            Step::Apply => match self.outcome {
                Some(true) => self.reset_for_next(),
                _ => self.go_to(Step::Review),
            },
            _ => {
                if let Some(next) = self.neighbour(1) {
                    self.go_to(next);
                }
            }
        }
        Signal::None
    }

    fn validate_current(&mut self) -> Result<Option<Signal>, String> {
        match self.step {
            Step::Protocol => {
                if PROTOCOL_CHOICES[self.protocol_idx].kind.is_none() {
                    return Err(format!(
                        "{} is not supported yet. Only CIFS / SMB is available for now.",
                        PROTOCOL_CHOICES[self.protocol_idx].label
                    ));
                }
            }
            Step::Host => {
                let (host, share) = cifs::parse_host(self.host.value())?;
                self.host.set(&host);
                // A pasted //host/share fills in the next step too.
                if let Some(share) = share
                    && self.share.is_empty()
                {
                    self.share.set(&share);
                }
            }
            Step::Share => {
                let share = cifs::parse_share(self.share.value())?;
                self.share.set(&share);
            }
            Step::Username => {
                let user = cifs::parse_username(self.username.value())?;
                self.username.set(&user);
            }
            Step::MountDir => {
                let dir = cifs::parse_mount_dir(self.mount_dir.value())?;
                self.mount_dir.set(&dir.to_string_lossy());
                if dir.exists() {
                    if !dir.is_dir() {
                        return Err(format!("{} exists but is not a directory", dir.display()));
                    }
                    self.create_dir = None;
                } else if self.create_dir.as_ref() != Some(&dir) {
                    return Ok(Some(Signal::AskCreateDir(dir)));
                }
            }
            Step::Options => {
                let checks = [
                    cifs::parse_id(self.options[0].value(), "UID").map(drop),
                    cifs::parse_id(self.options[1].value(), "GID").map(drop),
                    cifs::parse_domain(self.options[2].value()).map(drop),
                    cifs::parse_extra_options(self.options[3].value()).map(drop),
                ];
                for (i, check) in checks.into_iter().enumerate() {
                    if let Err(message) = check {
                        self.options_focus = i;
                        return Err(message);
                    }
                }
            }
            Step::Password | Step::Mode | Step::Review | Step::Apply => {}
        }
        Ok(None)
    }

    /// Answer to [`Signal::AskCreateDir`].
    pub fn answer_create_dir(&mut self, dir: PathBuf, create: bool) {
        if create {
            self.create_dir = Some(dir);
            if let Some(next) = self.neighbour(1) {
                self.go_to(next);
            }
        } else {
            self.error = Some(format!(
                "{} does not exist. Choose another directory, or press Enter to create it.",
                dir.display()
            ));
        }
    }

    /// Whether the mount directory will be created when applying.
    pub fn will_create_dir(&self) -> bool {
        self.create_dir.is_some()
    }

    /// Things worth knowing before applying, shown on the Review step.
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        let dir = PathBuf::from(self.mount_dir.value());
        if dir
            .read_dir()
            .is_ok_and(|mut entries| entries.next().is_some())
        {
            out.push(format!(
                "{} is not empty; its current contents are hidden while the share is mounted.",
                dir.display()
            ));
        }
        if self.ctx.dry_run {
            out.push("Dry run: nothing on this system will be changed.".into());
        }
        out
    }

    /// The validated request for the current answers, or the step to fix.
    pub fn request(&self) -> Result<MountRequest, (Step, String)> {
        let at = |step: Step| move |message: String| (step, message);
        let (host, _) = cifs::parse_host(self.host.value()).map_err(at(Step::Host))?;
        Ok(MountRequest {
            protocol: PROTOCOL_CHOICES[self.protocol_idx]
                .kind
                .unwrap_or(ProtocolKind::Cifs),
            host,
            share: cifs::parse_share(self.share.value()).map_err(at(Step::Share))?,
            username: cifs::parse_username(self.username.value()).map_err(at(Step::Username))?,
            password: self.password.value().to_string(),
            domain: cifs::parse_domain(self.options[2].value()).map_err(at(Step::Options))?,
            mount_dir: cifs::parse_mount_dir(self.mount_dir.value()).map_err(at(Step::MountDir))?,
            uid: cifs::parse_id(self.options[0].value(), "UID").map_err(at(Step::Options))?,
            gid: cifs::parse_id(self.options[1].value(), "GID").map_err(at(Step::Options))?,
            extra_options: cifs::parse_extra_options(self.options[3].value())
                .map_err(at(Step::Options))?,
            persistence: self.persistence(),
        })
    }

    /// The command or fstab line for the answers so far, once there are
    /// enough of them to be meaningful.
    pub fn preview(&self) -> Option<String> {
        if self.host.is_empty() || self.share.is_empty() || self.mount_dir.is_empty() {
            return None;
        }
        self.request().ok().map(|req| ops::preview(&self.ctx, &req))
    }

    /// Short value shown next to a step in the sidebar.
    pub fn summary(&self, step: Step) -> String {
        match step {
            Step::Protocol => PROTOCOL_CHOICES[self.protocol_idx].label.to_string(),
            Step::Host => self.host.value().to_string(),
            Step::Share => self.share.value().to_string(),
            Step::Username if self.is_guest() => "guest".to_string(),
            Step::Username => self.username.value().to_string(),
            Step::Password => "•".repeat(self.password.len().min(8)),
            Step::MountDir => self.mount_dir.value().to_string(),
            Step::Options => {
                let [uid, gid, ..] = &self.options;
                match (uid.is_empty(), gid.is_empty()) {
                    (false, false) => format!("uid {} gid {}", uid.value(), gid.value()),
                    (false, true) => format!("uid {}", uid.value()),
                    (true, false) => format!("gid {}", gid.value()),
                    (true, true) => "defaults".to_string(),
                }
            }
            Step::Mode => MODES[self.mode_idx].1.to_string(),
            Step::Review | Step::Apply => String::new(),
        }
    }

    fn start(&mut self) {
        let req = match self.request() {
            Ok(req) => req,
            Err((step, message)) => {
                self.go_to(step);
                self.error = Some(message);
                return;
            }
        };
        self.step = Step::Apply;
        self.error = None;
        self.log.clear();
        self.outcome = None;
        let (tx, rx) = mpsc::channel();
        self.progress = Some(rx);
        let ctx = Arc::clone(&self.ctx);
        // Mounting can block for a long time on an unreachable host; keep it
        // off the UI thread.
        thread::spawn(move || {
            ops::apply(&ctx, &req, &mut |event| {
                let _ = tx.send(event);
            });
        });
    }

    /// Drains progress from the worker. Returns true when the mount finished
    /// during this call.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.progress else {
            return false;
        };
        let events: Vec<Event> = rx.try_iter().collect();
        let mut finished = false;
        for event in events {
            finished |= self.record(event);
        }
        if finished {
            self.progress = None;
        }
        finished
    }

    fn record(&mut self, event: Event) -> bool {
        fn close(log: &mut [LogEntry], state: LogState, detail: String) {
            if let Some(entry) = log.last_mut() {
                entry.state = state;
                entry.detail = detail;
            }
        }
        match event {
            Event::Begin(title) => self.log.push(LogEntry {
                title,
                state: LogState::Running,
                detail: String::new(),
            }),
            Event::Done(detail) => close(&mut self.log, LogState::Ok, detail),
            Event::Failed(detail) => close(&mut self.log, LogState::Failed, detail),
            Event::Warned(detail) => close(&mut self.log, LogState::Warned, detail),
            Event::Note(text) => self.log.push(LogEntry {
                title: text,
                state: LogState::Note,
                detail: String::new(),
            }),
            Event::Finished { success } => {
                self.outcome = Some(success);
                // The secret is no longer needed once the mount exists.
                if success {
                    self.password.clear();
                }
                return true;
            }
        }
        false
    }

    /// After a success: keep the server and identity (mounting several
    /// shares from one NAS is the common case), clear the rest.
    fn reset_for_next(&mut self) {
        self.share.clear();
        self.password.clear();
        self.mount_dir.clear();
        self.mount_dir_edited = false;
        self.create_dir = None;
        self.reveal_password = false;
        self.log.clear();
        self.outcome = None;
        self.go_to(Step::Share);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wizard() -> Wizard {
        let dir = std::env::temp_dir().join(format!("fstab-auto-wiz-{}", std::process::id()));
        let mut ctx = SysCtx::new(dir.join("fstab"), dir.join("creds"), true);
        ctx.mounts_file = dir.join("mounts");
        let user = InvokingUser {
            name: "alice".into(),
            uid: 1001,
            gid: 1001,
        };
        Wizard::new(Arc::new(ctx), user)
    }

    fn press(w: &mut Wizard, code: KeyCode) -> Signal {
        w.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn type_text(w: &mut Wizard, text: &str) {
        for c in text.chars() {
            press(w, KeyCode::Char(c));
        }
    }

    fn wait(w: &mut Wizard) {
        for _ in 0..500 {
            if w.poll() {
                return;
            }
            thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("apply did not finish");
    }

    #[test]
    fn full_walkthrough_builds_the_expected_command() {
        let mut w = wizard();
        assert_eq!(w.step, Step::Protocol);
        press(&mut w, KeyCode::Enter);
        type_text(&mut w, "nas01");
        press(&mut w, KeyCode::Enter);
        type_text(&mut w, "projects");
        press(&mut w, KeyCode::Enter);
        assert_eq!(w.step, Step::Username);
        assert_eq!(w.username.value(), "alice");
        press(&mut w, KeyCode::Enter);
        type_text(&mut w, "hunter2");
        press(&mut w, KeyCode::Enter);

        assert_eq!(w.step, Step::MountDir);
        assert_eq!(w.mount_dir.value(), "/mnt/projects");
        press(&mut w, KeyCode::Char('u'));
        w.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        type_text(&mut w, "/nonexistent-fstab-auto/nas");
        let signal = press(&mut w, KeyCode::Enter);
        let dir = PathBuf::from("/nonexistent-fstab-auto/nas");
        assert_eq!(signal, Signal::AskCreateDir(dir.clone()));
        assert_eq!(w.step, Step::MountDir);

        w.answer_create_dir(dir.clone(), false);
        assert_eq!(w.step, Step::MountDir);
        assert!(w.error.is_some());
        w.answer_create_dir(dir, true);
        assert_eq!(w.step, Step::Options);
        assert!(w.will_create_dir());

        press(&mut w, KeyCode::Down);
        w.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        press(&mut w, KeyCode::Enter);
        assert_eq!(w.step, Step::Mode);
        press(&mut w, KeyCode::Enter);
        assert_eq!(w.step, Step::Review);
        assert_eq!(
            w.preview().unwrap(),
            "mount -t cifs -o username=alice,uid=1001 //nas01/projects /nonexistent-fstab-auto/nas"
        );

        press(&mut w, KeyCode::Enter);
        assert!(w.is_running());
        wait(&mut w);
        assert_eq!(w.outcome, Some(true));
        assert!(w.log.iter().all(|e| e.state == LogState::Ok));
        assert!(w.password.is_empty());

        press(&mut w, KeyCode::Enter);
        assert_eq!(w.step, Step::Share);
        assert_eq!(w.host.value(), "nas01");
        assert!(w.share.is_empty());
    }

    #[test]
    fn guest_skips_password_and_esc_goes_back() {
        let mut w = wizard();
        press(&mut w, KeyCode::Enter);
        type_text(&mut w, "//nas/public");
        press(&mut w, KeyCode::Enter);
        assert_eq!(w.host.value(), "nas");
        assert_eq!(w.share.value(), "public");
        press(&mut w, KeyCode::Enter);
        w.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        press(&mut w, KeyCode::Enter);
        assert_eq!(w.step, Step::MountDir);
        assert!(!w.visible_steps().contains(&Step::Password));
        press(&mut w, KeyCode::Esc);
        assert_eq!(w.step, Step::Username);
    }

    #[test]
    fn invalid_answers_keep_the_step_and_explain() {
        let mut w = wizard();
        press(&mut w, KeyCode::Down);
        press(&mut w, KeyCode::Enter);
        assert_eq!(w.step, Step::Protocol);
        assert!(w.error.as_deref().unwrap().contains("not supported yet"));
        press(&mut w, KeyCode::Up);
        press(&mut w, KeyCode::Enter);
        press(&mut w, KeyCode::Enter);
        assert_eq!(w.step, Step::Host);
        assert!(w.error.is_some());
        press(&mut w, KeyCode::Char('n'));
        assert!(w.error.is_none());
    }
}
