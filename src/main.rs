//! fstab-auto: an interactive terminal UI for mounting NAS shares.

mod model;
mod protocol;
mod system;
mod tui;

use std::io::IsTerminal;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;

use system::{DEFAULT_CREDENTIALS_DIR, DEFAULT_FSTAB, SysCtx};

/// Mount NAS shares (CIFS / SMB) step by step, temporarily or permanently.
///
/// Run without arguments to start the interactive wizard. Mounting needs root
/// privileges, so fstab-auto re-runs itself through sudo when necessary.
#[derive(Parser, Debug)]
#[command(version, about, long_about)]
struct Cli {
    /// Walk through everything without changing the system
    #[arg(long)]
    dry_run: bool,

    /// fstab file to manage
    #[arg(long, value_name = "FILE", default_value = DEFAULT_FSTAB)]
    fstab: PathBuf,

    /// Directory for the root-only credential files of permanent mounts
    #[arg(long, value_name = "DIR", default_value = DEFAULT_CREDENTIALS_DIR)]
    credentials_dir: PathBuf,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Print a shell completion script, e.g. `fstab-auto completions bash`
    Completions {
        /// Shell to generate the script for
        shell: Shell,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Some(Commands::Completions { shell }) = cli.command {
        let mut command = Cli::command();
        let name = command.get_name().to_string();
        clap_complete::generate(shell, &mut command, name, &mut std::io::stdout());
        return ExitCode::SUCCESS;
    }
    let mut dry_run = cli.dry_run;

    if !cfg!(target_os = "linux") && !dry_run {
        eprintln!("fstab-auto: only Linux is supported for now; continuing in --dry-run mode.");
        dry_run = true;
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        eprintln!("fstab-auto: this is an interactive tool and needs a terminal.");
        return ExitCode::from(2);
    }
    if !dry_run && !system::is_root() {
        return rerun_with_sudo();
    }

    let ctx = SysCtx::new(cli.fstab, cli.credentials_dir, dry_run);
    match tui::run(ctx, system::invoking_user()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fstab-auto: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Replaces this process with `sudo fstab-auto <same arguments>`. sudo
/// records who called it, which is how the wizard still knows the real user.
fn rerun_with_sudo() -> ExitCode {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("fstab-auto: cannot locate own executable: {error}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("fstab-auto: mounting needs root privileges, asking sudo…");
    let error = Command::new("sudo")
        .arg("--")
        .arg(exe)
        .args(std::env::args_os().skip(1))
        .exec();
    eprintln!("fstab-auto: could not run sudo: {error}");
    eprintln!("Run it as root instead, or try `fstab-auto --dry-run`.");
    ExitCode::FAILURE
}
