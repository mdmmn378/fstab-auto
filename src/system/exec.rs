//! Running external commands, with dry-run support.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::SysCtx;

/// A command to run. Environment values may be secrets and are never shown.
#[derive(Clone)]
pub struct CmdSpec {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl CmdSpec {
    pub fn new(program: &str, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            program: program.to_string(),
            args: args.into_iter().map(Into::into).collect(),
            env: Vec::new(),
        }
    }

    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.to_string(), value.to_string()));
        self
    }

    /// Shell-style rendering for previews and logs.
    pub fn display(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .map(shell_quote)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

pub fn shell_quote(arg: &str) -> String {
    let safe = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_=+,./:@%".contains(c));
    if safe {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// Runs the command and returns its trimmed output, or a readable error built
/// from stderr. In dry-run mode nothing is executed.
pub fn run(ctx: &SysCtx, spec: &CmdSpec) -> Result<String, String> {
    if ctx.dry_run {
        return Ok(format!("dry run: {}", spec.display()));
    }
    let program = find_program(&spec.program).unwrap_or_else(|| PathBuf::from(&spec.program));
    let output = Command::new(&program)
        .args(&spec.args)
        .envs(spec.env.iter().map(|(k, v)| (k, v)))
        // Stable, parseable messages regardless of the user's locale.
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("could not run {}: {e}", spec.program))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.success() {
        return Ok(stdout);
    }
    let message = match (stderr.is_empty(), stdout.is_empty()) {
        (false, _) => stderr,
        (true, false) => stdout,
        (true, true) => match output.status.code() {
            Some(code) => format!("{} exited with status {code}", spec.program),
            None => format!("{} was killed by a signal", spec.program),
        },
    };
    Err(message)
}

/// Looks the program up in PATH plus the sbin directories, which are often
/// missing from an unprivileged user's PATH.
pub fn find_program(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        return Path::new(name).is_file().then(|| PathBuf::from(name));
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(["/usr/local/sbin", "/usr/sbin", "/sbin"].map(PathBuf::from))
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_quotes_only_when_needed() {
        let spec = CmdSpec::new(
            "mount",
            [
                "-t",
                "cifs",
                "-o",
                "username=alice,uid=1001",
                "//nas/my share",
                "/mnt/it's",
            ],
        )
        .env("PASSWD", "secret");
        assert_eq!(
            spec.display(),
            r"mount -t cifs -o username=alice,uid=1001 '//nas/my share' '/mnt/it'\''s'"
        );
        assert!(!spec.display().contains("secret"));
    }

    #[test]
    fn dry_run_executes_nothing() {
        let ctx = SysCtx::new("/nonexistent".into(), "/nonexistent".into(), true);
        let out = run(&ctx, &CmdSpec::new("/definitely/not/a/program", ["x"])).unwrap();
        assert!(out.starts_with("dry run:"));
    }
}
