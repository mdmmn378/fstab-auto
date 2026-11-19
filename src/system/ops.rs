//! The operations the UI triggers: mount (temporary or permanent), unmount,
//! and removing an fstab entry. Each reports progress step by step so the UI
//! can show exactly what was done to the system.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use super::exec::{self, CmdSpec};
use super::{SysCtx, fstab, mounts};
use crate::model::{MountRequest, Persistence};
use crate::protocol::{self, Auth, cifs};

/// Progress of [`apply`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A step started.
    Begin(String),
    /// The current step finished; the string is an optional detail line.
    Done(String),
    /// The current step failed.
    Failed(String),
    /// The current step failed but the operation carries on.
    Warned(String),
    /// Extra information not tied to a step.
    Note(String),
    /// The operation is over.
    Finished { success: bool },
}

pub fn credentials_path(ctx: &SysCtx, req: &MountRequest) -> PathBuf {
    ctx.credentials_dir
        .join(cifs::credentials_file_name(&req.mount_dir))
}

/// The command used for a temporary mount. The password is handed to the
/// mount helper through the environment, so it never shows up in `ps`.
pub fn temporary_mount_command(req: &MountRequest) -> CmdSpec {
    let mut args = vec![
        "-t".to_string(),
        protocol::fs_type(req.protocol).to_string(),
    ];
    let options = protocol::options(req, Auth::Inline);
    if !options.is_empty() {
        args.push("-o".into());
        args.push(options.join(","));
    }
    args.push(protocol::source(req));
    args.push(req.mount_dir.to_string_lossy().into_owned());
    let spec = CmdSpec::new("mount", args);
    if req.is_guest() {
        spec
    } else {
        spec.env("PASSWD", &req.password)
    }
}

/// The line a permanent mount adds to fstab.
pub fn fstab_line(ctx: &SysCtx, req: &MountRequest) -> String {
    let cred = credentials_path(ctx, req);
    fstab::render_line(
        &protocol::source(req),
        &req.mount_dir.to_string_lossy(),
        protocol::fs_type(req.protocol),
        &protocol::options(req, Auth::CredentialsFile(&cred)),
    )
}

/// What the Review screen shows as "this is what will run".
pub fn preview(ctx: &SysCtx, req: &MountRequest) -> String {
    match req.persistence {
        Persistence::Temporary => temporary_mount_command(req).display(),
        Persistence::Permanent => fstab_line(ctx, req),
    }
}

fn mount_from_fstab_command(ctx: &SysCtx, dir: &Path) -> CmdSpec {
    let dir = dir.to_string_lossy().into_owned();
    if ctx.uses_default_fstab() {
        CmdSpec::new("mount", [dir])
    } else {
        CmdSpec::new(
            "mount",
            [
                "--fstab".to_string(),
                ctx.fstab.to_string_lossy().into_owned(),
                dir,
            ],
        )
    }
}

/// Turns the terse kernel errors from mount.cifs into something actionable.
pub fn explain(error: &str) -> String {
    let hint = if error.contains("error(13)") {
        Some("The NAS rejected the login. Check the username, password and domain.")
    } else if error.contains("error(2)") {
        Some("The NAS has no share with that name. Check the share name.")
    } else if error.contains("error(111)") {
        Some("The host refused the connection. Is file sharing (SMB) enabled on it?")
    } else if error.contains("error(112)")
        || error.contains("error(113)")
        || error.contains("error(115)")
        || error.contains("error(110)")
    {
        Some("The host did not respond. Check that it is powered on and reachable.")
    } else if error.contains("could not resolve address") || error.contains("error(6)") {
        Some("The hostname could not be resolved. Try the IP address instead.")
    } else if error.contains("error(95)") {
        Some("Protocol version mismatch. Try adding vers=3.0 or vers=2.1 to the extra options.")
    } else if error.contains("error(16)") {
        Some("The mount point is busy or already in use.")
    } else if error.contains("error(1)") || error.contains("Operation not permitted") {
        Some(
            "Mounting was not permitted. Root privileges (and, in containers, --privileged) are required.",
        )
    } else {
        None
    };
    match hint {
        Some(hint) => format!("{error}\n{hint}"),
        None => error.to_string(),
    }
}

fn systemd_running() -> bool {
    Path::new("/run/systemd/system").is_dir()
}

fn write_private_file(path: &Path, content: &str) -> Result<(), String> {
    let err = |e: std::io::Error| format!("cannot write {}: {e}", path.display());
    if let Some(dir) = path.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    // Remove first so a pre-existing file with loose permissions is never
    // reused for the secret.
    let _ = fs::remove_file(path);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(err)?;
    file.write_all(content.as_bytes()).map_err(err)?;
    file.sync_all().map_err(err)
}

/// Runs one step and reports its outcome. `Err` aborts the operation.
fn step(
    emit: &mut dyn FnMut(Event),
    title: impl Into<String>,
    run: impl FnOnce() -> Result<String, String>,
) -> Result<(), ()> {
    emit(Event::Begin(title.into()));
    match run() {
        Ok(detail) => {
            emit(Event::Done(detail));
            Ok(())
        }
        Err(error) => {
            emit(Event::Failed(error));
            Err(())
        }
    }
}

fn preflight(ctx: &SysCtx, req: &MountRequest) -> Result<String, String> {
    let dir = &req.mount_dir;
    if dir.exists() && !dir.is_dir() {
        return Err(format!("{} exists but is not a directory", dir.display()));
    }
    if mounts::is_mounted(ctx, dir) {
        return Err(format!(
            "Something is already mounted on {}. Unmount it first (Mounts tab).",
            dir.display()
        ));
    }
    if req.persistence == Persistence::Permanent {
        let target = dir.to_string_lossy();
        if fstab::read(&ctx.fstab)?
            .iter()
            .any(|e| fstab::same_path(&e.target, &target))
        {
            return Err(format!(
                "{} already has an entry for {}. Remove it first (Mounts tab).",
                ctx.fstab.display(),
                dir.display()
            ));
        }
    }
    let helper = protocol::helper(req.protocol);
    match exec::find_program(helper) {
        Some(path) => Ok(format!("{helper} found at {}", path.display())),
        None if ctx.dry_run => Ok(format!("dry run: {helper} not found on this machine")),
        None => Err(format!(
            "{helper} is not installed. Install the {pkg} package, e.g.\n  sudo apt install {pkg}",
            pkg = protocol::helper_package(req.protocol)
        )),
    }
}

/// Performs the mount described by `req`. Returns whether it succeeded; the
/// details are delivered through `emit`.
pub fn apply(ctx: &SysCtx, req: &MountRequest, emit: &mut dyn FnMut(Event)) -> bool {
    let success = apply_steps(ctx, req, emit).is_ok();
    emit(Event::Finished { success });
    success
}

fn apply_steps(ctx: &SysCtx, req: &MountRequest, emit: &mut dyn FnMut(Event)) -> Result<(), ()> {
    let dir = req.mount_dir.clone();
    let source = protocol::source(req);

    step(emit, "Check prerequisites", || preflight(ctx, req))?;

    if !dir.exists() {
        step(emit, format!("Create directory {}", dir.display()), || {
            if ctx.dry_run {
                return Ok("dry run: not created".into());
            }
            fs::create_dir_all(&dir)
                .map(|()| String::new())
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))
        })?;
    }

    match req.persistence {
        Persistence::Temporary => {
            let cmd = temporary_mount_command(req);
            step(emit, format!("Mount {source} on {}", dir.display()), || {
                exec::run(ctx, &cmd)
                    .map(|_| cmd.display())
                    .map_err(|e| explain(&e))
            })?;
        }
        Persistence::Permanent => apply_permanent(ctx, req, emit)?,
    }

    step(emit, "Verify the mount", || {
        if ctx.dry_run {
            Ok("dry run: skipped".into())
        } else if mounts::is_mounted(ctx, &dir) {
            Ok(format!("{source} is mounted on {}", dir.display()))
        } else {
            Err(format!(
                "mount reported success but {} is not listed as mounted",
                dir.display()
            ))
        }
    })
}

fn apply_permanent(
    ctx: &SysCtx,
    req: &MountRequest,
    emit: &mut dyn FnMut(Event),
) -> Result<(), ()> {
    let dir = req.mount_dir.clone();
    let cred = credentials_path(ctx, req);
    let wrote_credentials = !req.is_guest();

    if wrote_credentials {
        step(
            emit,
            format!("Store credentials in {}", cred.display()),
            || {
                if ctx.dry_run {
                    return Ok("dry run: not written".into());
                }
                write_private_file(&cred, &cifs::credentials_file(req))
                    .map(|()| "readable by root only (mode 600)".into())
            },
        )?;
    }

    let rollback = |emit: &mut dyn FnMut(Event), entry_added: bool| {
        if ctx.dry_run {
            return;
        }
        let mut undone = Vec::new();
        if entry_added {
            let restored = fs::read_to_string(&ctx.fstab)
                .map_err(|e| e.to_string())
                .and_then(|content| {
                    let (content, _) = fstab::remove_entry(&content, &dir.to_string_lossy());
                    fstab::write_atomic(&ctx.fstab, &content, fstab::existing_mode(&ctx.fstab))
                });
            match restored {
                Ok(()) => undone.push(format!("removed the entry from {}", ctx.fstab.display())),
                Err(e) => emit(Event::Note(format!(
                    "Could not undo the fstab change ({e}). Restore it from {}.",
                    fstab::backup_path(&ctx.fstab).display()
                ))),
            }
        }
        if wrote_credentials && fs::remove_file(&cred).is_ok() {
            undone.push("deleted the credentials file".to_string());
        }
        if !undone.is_empty() {
            emit(Event::Note(format!("Rolled back: {}.", undone.join(", "))));
        }
    };

    let line = fstab_line(ctx, req);
    let added = step(
        emit,
        format!("Add entry to {}", ctx.fstab.display()),
        || {
            if ctx.dry_run {
                return Ok(format!("dry run: {line}"));
            }
            let backup = fstab::backup(&ctx.fstab)?;
            let content = match fs::read_to_string(&ctx.fstab) {
                Ok(content) => content,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => return Err(format!("cannot read {}: {e}", ctx.fstab.display())),
            };
            fstab::write_atomic(
                &ctx.fstab,
                &fstab::add_entry(&content, &line),
                fstab::existing_mode(&ctx.fstab),
            )?;
            Ok(match backup {
                Some(path) => format!("{line}\nbackup saved to {}", path.display()),
                None => line.clone(),
            })
        },
    );
    if added.is_err() {
        rollback(emit, false);
        return Err(());
    }

    reload_systemd(ctx, emit);

    let cmd = mount_from_fstab_command(ctx, &dir);
    let mounted = step(emit, format!("Mount {}", dir.display()), || {
        exec::run(ctx, &cmd)
            .map(|_| cmd.display())
            .map_err(|e| explain(&e))
    });
    if mounted.is_err() {
        rollback(emit, true);
        return Err(());
    }
    Ok(())
}

/// systemd turns fstab into mount units; tell it the file changed. Failure is
/// not fatal because `mount` itself reads fstab directly.
fn reload_systemd(ctx: &SysCtx, emit: &mut dyn FnMut(Event)) {
    if !ctx.dry_run && !(systemd_running() && ctx.uses_default_fstab()) {
        return;
    }
    emit(Event::Begin("Reload systemd units".into()));
    match exec::run(ctx, &CmdSpec::new("systemctl", ["daemon-reload"])) {
        Ok(_) => emit(Event::Done(String::new())),
        Err(e) => emit(Event::Warned(e)),
    }
}

/// Unmounts `dir`.
pub fn unmount(ctx: &SysCtx, dir: &str) -> Result<String, String> {
    exec::run(ctx, &CmdSpec::new("umount", [dir])).map_err(|e| {
        if e.contains("busy") {
            format!("{e}\nA program is still using files in {dir}. Close it and try again.")
        } else {
            e
        }
    })?;
    Ok(format!("Unmounted {dir}"))
}

/// Mounts `dir` using its existing fstab entry.
pub fn mount_existing(ctx: &SysCtx, dir: &str) -> Result<String, String> {
    let cmd = mount_from_fstab_command(ctx, Path::new(dir));
    exec::run(ctx, &cmd).map_err(|e| explain(&e))?;
    Ok(format!("Mounted {dir}"))
}

/// Makes a mount non-permanent: unmounts it if needed, removes its fstab
/// entry (after a backup) and deletes the credentials file when it is one
/// this tool created.
pub fn remove_permanent(ctx: &SysCtx, dir: &str) -> Result<String, String> {
    if mounts::is_mounted(ctx, Path::new(dir)) {
        unmount(ctx, dir)?;
    }
    if ctx.dry_run {
        return Ok(format!(
            "dry run: would remove {dir} from {}",
            ctx.fstab.display()
        ));
    }
    let content = fs::read_to_string(&ctx.fstab)
        .map_err(|e| format!("cannot read {}: {e}", ctx.fstab.display()))?;
    let credentials: Vec<PathBuf> = fstab::parse(&content)
        .iter()
        .filter(|e| fstab::same_path(&e.target, dir))
        .filter_map(|e| fstab::credentials_path(&e.options))
        .collect();
    let (new_content, removed) = fstab::remove_entry(&content, dir);
    if removed == 0 {
        return Err(format!("{} has no entry for {dir}", ctx.fstab.display()));
    }
    fstab::backup(&ctx.fstab)?;
    fstab::write_atomic(&ctx.fstab, &new_content, fstab::existing_mode(&ctx.fstab))?;
    for path in credentials {
        // Only ever delete secrets this tool wrote.
        if path.starts_with(&ctx.credentials_dir) {
            let _ = fs::remove_file(path);
        }
    }
    if systemd_running() && ctx.uses_default_fstab() {
        let _ = exec::run(ctx, &CmdSpec::new("systemctl", ["daemon-reload"]));
    }
    Ok(format!("Removed {dir} from {}", ctx.fstab.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProtocolKind;

    fn request(persistence: Persistence) -> MountRequest {
        MountRequest {
            protocol: ProtocolKind::Cifs,
            host: "nas01".into(),
            share: "projects".into(),
            username: "alice".into(),
            password: "hunter2".into(),
            domain: String::new(),
            mount_dir: "/mnt/nas".into(),
            uid: Some(1001),
            gid: None,
            extra_options: vec![],
            persistence,
        }
    }

    fn ctx(dry_run: bool) -> SysCtx {
        SysCtx::new(
            "/etc/fstab".into(),
            "/etc/fstab-auto/credentials".into(),
            dry_run,
        )
    }

    #[test]
    fn temporary_command_matches_the_manual_one_and_hides_the_password() {
        let cmd = temporary_mount_command(&request(Persistence::Temporary));
        assert_eq!(
            cmd.display(),
            "mount -t cifs -o username=alice,uid=1001 //nas01/projects /mnt/nas"
        );
        assert_eq!(cmd.env, vec![("PASSWD".to_string(), "hunter2".to_string())]);
    }

    #[test]
    fn permanent_preview_is_the_fstab_line() {
        let line = preview(&ctx(true), &request(Persistence::Permanent));
        assert_eq!(
            line,
            "//nas01/projects /mnt/nas cifs credentials=/etc/fstab-auto/credentials/mnt-nas.cred,uid=1001,_netdev,nofail 0 0"
        );
        assert!(!line.contains("hunter2"));
    }

    #[test]
    fn dry_run_reports_every_step_and_changes_nothing() {
        let dir = std::env::temp_dir().join(format!("fstab-auto-ops-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mut ctx = SysCtx::new(dir.join("fstab"), dir.join("creds"), true);
        ctx.mounts_file = dir.join("mounts");
        let mut req = request(Persistence::Permanent);
        req.mount_dir = dir.join("mnt");

        let mut events = Vec::new();
        assert!(apply(&ctx, &req, &mut |e| events.push(e)));
        let titles: Vec<String> = events
            .iter()
            .filter_map(|e| match e {
                Event::Begin(t) => Some(t.split(' ').next().unwrap().to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(
            titles,
            [
                "Check", "Create", "Store", "Add", "Reload", "Mount", "Verify"
            ]
        );
        assert_eq!(events.last(), Some(&Event::Finished { success: true }));
        assert!(!events.iter().any(|e| format!("{e:?}").contains("hunter2")));
        assert!(!dir.join("fstab").exists());
        assert!(!dir.join("creds").exists());
        assert!(!dir.join("mnt").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn failed_permanent_mount_rolls_back_fstab_and_credentials() {
        let dir = std::env::temp_dir().join(format!("fstab-auto-rb-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let original = "UUID=x / ext4 defaults 0 1\n";
        fs::write(dir.join("fstab"), original).unwrap();
        let mut ctx = SysCtx::new(dir.join("fstab"), dir.join("creds"), false);
        ctx.mounts_file = dir.join("mounts");
        let mut req = request(Persistence::Permanent);
        req.mount_dir = dir.join("mnt");

        // Unprivileged test runs cannot mount, and machines without
        // mount.cifs fail the preflight: either way nothing may be left.
        let mut events = Vec::new();
        let ok = apply(&ctx, &req, &mut |e| events.push(e));
        if !ok {
            assert_eq!(fs::read_to_string(dir.join("fstab")).unwrap(), original);
            assert!(!credentials_path(&ctx, &req).exists());
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn remove_permanent_deletes_only_our_credentials() {
        let dir = std::env::temp_dir().join(format!("fstab-auto-rm-{}", std::process::id()));
        let creds = dir.join("creds");
        fs::create_dir_all(&creds).unwrap();
        let ours = creds.join("mnt-a.cred");
        let theirs = dir.join("theirs.cred");
        fs::write(&ours, "x").unwrap();
        fs::write(&theirs, "x").unwrap();
        fs::write(
            dir.join("fstab"),
            format!(
                "UUID=x / ext4 defaults 0 1\n{}\n//n/a /mnt/a cifs credentials={} 0 0\n//n/b /mnt/b cifs credentials={} 0 0\n",
                fstab::MARKER,
                ours.display(),
                theirs.display()
            ),
        )
        .unwrap();
        let mut ctx = SysCtx::new(dir.join("fstab"), creds, false);
        ctx.mounts_file = dir.join("mounts");

        remove_permanent(&ctx, "/mnt/a").unwrap();
        remove_permanent(&ctx, "/mnt/b").unwrap();
        assert!(remove_permanent(&ctx, "/mnt/b").is_err());
        assert_eq!(
            fs::read_to_string(dir.join("fstab")).unwrap(),
            "UUID=x / ext4 defaults 0 1\n"
        );
        assert!(!ours.exists());
        assert!(theirs.exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn explain_adds_hints() {
        assert!(explain("mount error(13): Permission denied").contains("username"));
        assert!(explain("mount error(2): No such file or directory").contains("share"));
        assert_eq!(explain("something odd"), "something odd");
    }
}
