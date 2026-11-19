//! Everything that touches the host: privileges, commands, fstab, mounts.

pub mod exec;
pub mod fstab;
pub mod mounts;
pub mod ops;

use std::path::PathBuf;

pub const DEFAULT_FSTAB: &str = "/etc/fstab";
pub const DEFAULT_CREDENTIALS_DIR: &str = "/etc/fstab-auto/credentials";
const PROC_MOUNTS: &str = "/proc/self/mounts";

/// Host paths and behaviour switches, injected so tests and `--dry-run` never
/// touch the real system files.
#[derive(Clone, Debug)]
pub struct SysCtx {
    pub fstab: PathBuf,
    pub credentials_dir: PathBuf,
    pub mounts_file: PathBuf,
    /// Show what would happen without changing anything.
    pub dry_run: bool,
}

impl SysCtx {
    pub fn new(fstab: PathBuf, credentials_dir: PathBuf, dry_run: bool) -> Self {
        Self {
            fstab,
            credentials_dir,
            mounts_file: PathBuf::from(PROC_MOUNTS),
            dry_run,
        }
    }

    pub fn uses_default_fstab(&self) -> bool {
        self.fstab == std::path::Path::new(DEFAULT_FSTAB)
    }
}

/// The human who started the tool, even when it is running under sudo. Their
/// ids are the default owner of mounted files.
#[derive(Clone, Debug)]
pub struct InvokingUser {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
}

pub fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

pub fn invoking_user() -> InvokingUser {
    let env_id = |key: &str| std::env::var(key).ok().and_then(|v| v.parse::<u32>().ok());
    // SAFETY: getuid/getgid have no preconditions and cannot fail.
    let (real_uid, real_gid) = unsafe { (libc::getuid(), libc::getgid()) };
    match (env_id("SUDO_UID"), env_id("SUDO_GID")) {
        (Some(uid), Some(gid)) if is_root() => InvokingUser {
            name: std::env::var("SUDO_USER").unwrap_or_default(),
            uid,
            gid,
        },
        _ => InvokingUser {
            name: std::env::var("USER")
                .or_else(|_| std::env::var("LOGNAME"))
                .unwrap_or_default(),
            uid: real_uid,
            gid: real_gid,
        },
    }
}
