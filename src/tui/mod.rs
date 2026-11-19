//! Terminal setup and the event loop.

mod app;
mod input;
mod ui;
mod wizard;

use std::io::stdout;
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste};
use ratatui::crossterm::execute;

use crate::system::{InvokingUser, SysCtx};
use app::App;

const TICK: Duration = Duration::from_millis(100);

pub fn run(ctx: SysCtx, user: InvokingUser) -> Result<()> {
    // `init` also installs a panic hook that restores the terminal.
    let mut terminal = ratatui::init();
    // Without bracketed paste a pasted password would arrive as key presses,
    // and a newline in it would submit the step.
    let _ = execute!(stdout(), EnableBracketedPaste);
    let mut app = App::new(ctx, user);
    let result = (|| -> Result<()> {
        while !app.should_quit {
            terminal.draw(|frame| ui::render(frame, &app))?;
            if event::poll(TICK)? {
                app.handle_event(event::read()?);
            }
            app.tick();
        }
        Ok(())
    })();
    let _ = execute!(stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}
