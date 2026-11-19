//! CIFS / SMB: option building and input validation.

use std::path::{Path, PathBuf};

use super::Auth;
use crate::model::{MountRequest, Persistence};

pub const FS_TYPE: &str = "cifs";
pub const HELPER: &str = "mount.cifs";

/// Options that have a dedicated wizard field (or would leak a secret) and so
/// are rejected in the free-form "extra options" field.
const RESERVED_OPTIONS: &[&str] = &[
    "username",
    "user",
    "password",
    "pass",
    "credentials",
    "cred",
    "domain",
    "dom",
    "workgroup",
    "uid",
    "gid",
    "guest",
];

pub fn source(host: &str, share: &str) -> String {
    format!("//{host}/{share}")
}

pub fn options(req: &MountRequest, auth: Auth<'_>) -> Vec<String> {
    let mut opts = Vec::new();
    if req.is_guest() {
        opts.push("guest".to_string());
    } else {
        match auth {
            Auth::Inline => {
                opts.push(format!("username={}", req.username));
                if !req.domain.is_empty() {
                    opts.push(format!("domain={}", req.domain));
                }
            }
            Auth::CredentialsFile(path) => {
                opts.push(format!("credentials={}", path.display()));
            }
        }
    }
    if let Some(uid) = req.uid {
        opts.push(format!("uid={uid}"));
    }
    if let Some(gid) = req.gid {
        opts.push(format!("gid={gid}"));
    }
    opts.extend(req.extra_options.iter().cloned());
    if req.persistence == Persistence::Permanent {
        // Wait for the network at boot, and never block booting when the NAS
        // is unreachable.
        for default in ["_netdev", "nofail"] {
            if !opts.iter().any(|o| o == default) {
                opts.push(default.to_string());
            }
        }
    }
    opts
}

/// Contents of a `credentials=` file as understood by mount.cifs.
pub fn credentials_file(req: &MountRequest) -> String {
    let mut out = format!("username={}\npassword={}\n", req.username, req.password);
    if !req.domain.is_empty() {
        out.push_str(&format!("domain={}\n", req.domain));
    }
    out
}

/// File name for the credentials of the mount at `mount_dir`,
/// e.g. `/mnt/nas` -> `mnt-nas.cred`.
pub fn credentials_file_name(mount_dir: &Path) -> String {
    let slug: String = mount_dir
        .to_string_lossy()
        .trim_matches('/')
        .chars()
        .map(|c| match c {
            '/' => '-',
            c if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' => c,
            _ => '_',
        })
        .collect();
    format!("{slug}.cred")
}

/// Accepts `nas`, `//nas`, `\\nas` and `//nas/share`. Returns the host and,
/// when the user pasted a full UNC path, the share part.
pub fn parse_host(input: &str) -> Result<(String, Option<String>), String> {
    let cleaned = input.trim().replace('\\', "/");
    let cleaned = cleaned
        .strip_prefix("smb:")
        .unwrap_or(&cleaned)
        .trim_start_matches('/');
    let (host, share) = match cleaned.split_once('/') {
        Some((h, s)) => (h, Some(s.trim_matches('/').to_string())),
        None => (cleaned, None),
    };
    if host.is_empty() {
        return Err("Enter the NAS hostname or IP address".into());
    }
    if host.chars().any(|c| c.is_whitespace() || c == ',') {
        return Err("A hostname cannot contain spaces or commas".into());
    }
    Ok((host.to_string(), share.filter(|s| !s.is_empty())))
}

pub fn parse_share(input: &str) -> Result<String, String> {
    let share = input.trim().replace('\\', "/");
    let share = share.trim_matches('/');
    if share.is_empty() {
        return Err("Enter the share name, e.g. projects".into());
    }
    if share.contains("//") {
        return Err("The share path contains an empty segment".into());
    }
    Ok(share.to_string())
}

pub fn parse_username(input: &str) -> Result<String, String> {
    let user = input.trim();
    if user.contains(',') {
        return Err("A username cannot contain commas".into());
    }
    Ok(user.to_string())
}

pub fn parse_domain(input: &str) -> Result<String, String> {
    let domain = input.trim();
    if domain.chars().any(|c| c.is_whitespace() || c == ',') {
        return Err("A domain cannot contain spaces or commas".into());
    }
    Ok(domain.to_string())
}

pub fn parse_mount_dir(input: &str) -> Result<PathBuf, String> {
    let dir = input.trim();
    if dir.is_empty() {
        return Err("Enter the directory to mount on, e.g. /mnt/nas".into());
    }
    if !dir.starts_with('/') {
        return Err("Use an absolute path starting with /".into());
    }
    if dir.contains(',') {
        return Err("A mount directory cannot contain commas".into());
    }
    let trimmed = dir.trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("Refusing to mount over the root directory".into());
    }
    if trimmed.split('/').any(|part| part == "." || part == "..") {
        return Err("Use a path without . or .. segments".into());
    }
    Ok(PathBuf::from(trimmed))
}

/// Empty input means "do not set"; otherwise a numeric id.
pub fn parse_id(input: &str, what: &str) -> Result<Option<u32>, String> {
    let value = input.trim();
    if value.is_empty() {
        return Ok(None);
    }
    value
        .parse()
        .map(Some)
        .map_err(|_| format!("{what} must be a number, e.g. 1000"))
}

pub fn parse_extra_options(input: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for raw in input.split(',') {
        let opt = raw.trim();
        if opt.is_empty() {
            continue;
        }
        if opt.chars().any(char::is_whitespace) {
            return Err(format!(
                "Option \"{opt}\" contains a space; separate options with commas"
            ));
        }
        let key = opt.split('=').next().unwrap_or(opt);
        if RESERVED_OPTIONS.contains(&key) {
            return Err(format!(
                "\"{key}\" is set by its own field; remove it from extra options"
            ));
        }
        out.push(opt.to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProtocolKind;

    fn request() -> MountRequest {
        MountRequest {
            protocol: ProtocolKind::Cifs,
            host: "nas01".into(),
            share: "projects".into(),
            username: "alice".into(),
            password: "s3cret".into(),
            domain: String::new(),
            mount_dir: "/mnt/nas".into(),
            uid: Some(1001),
            gid: None,
            extra_options: vec![],
            persistence: Persistence::Temporary,
        }
    }

    #[test]
    fn temporary_options_match_the_manual_command() {
        let req = request();
        assert_eq!(source(&req.host, &req.share), "//nas01/projects");
        assert_eq!(
            options(&req, Auth::Inline).join(","),
            "username=alice,uid=1001"
        );
    }

    #[test]
    fn permanent_options_use_credentials_file_and_boot_safe_defaults() {
        let mut req = request();
        req.persistence = Persistence::Permanent;
        req.gid = Some(1001);
        req.extra_options = vec!["vers=3.0".into(), "nofail".into()];
        let cred = PathBuf::from("/etc/fstab-auto/credentials/mnt-nas.cred");
        assert_eq!(
            options(&req, Auth::CredentialsFile(&cred)).join(","),
            "credentials=/etc/fstab-auto/credentials/mnt-nas.cred,uid=1001,gid=1001,vers=3.0,nofail,_netdev"
        );
    }

    #[test]
    fn guest_never_references_credentials() {
        let mut req = request();
        req.username.clear();
        let cred = PathBuf::from("/x.cred");
        assert_eq!(
            options(&req, Auth::CredentialsFile(&cred)).join(","),
            "guest,uid=1001"
        );
    }

    #[test]
    fn credentials_file_contents() {
        let mut req = request();
        req.domain = "CORP".into();
        assert_eq!(
            credentials_file(&req),
            "username=alice\npassword=s3cret\ndomain=CORP\n"
        );
        assert_eq!(credentials_file_name(Path::new("/mnt/nas")), "mnt-nas.cred");
        assert_eq!(
            credentials_file_name(Path::new("/mnt/my share/")),
            "mnt-my_share.cred"
        );
    }

    #[test]
    fn host_accepts_unc_forms() {
        assert_eq!(parse_host(" nas01 ").unwrap(), ("nas01".into(), None));
        assert_eq!(parse_host("//nas01").unwrap(), ("nas01".into(), None));
        assert_eq!(
            parse_host(r"\\nas01\data").unwrap(),
            ("nas01".into(), Some("data".into()))
        );
        assert_eq!(
            parse_host("smb://10.0.0.5/a/b/").unwrap(),
            ("10.0.0.5".into(), Some("a/b".into()))
        );
        assert!(parse_host("").is_err());
        assert!(parse_host("my nas").is_err());
    }

    #[test]
    fn share_and_mount_dir_validation() {
        assert_eq!(parse_share("/projects/").unwrap(), "projects");
        assert!(parse_share(" / ").is_err());
        assert_eq!(
            parse_mount_dir("/mnt/nas/").unwrap(),
            PathBuf::from("/mnt/nas")
        );
        assert!(parse_mount_dir("mnt/nas").is_err());
        assert!(parse_mount_dir("/").is_err());
        assert!(parse_mount_dir("/mnt/../etc").is_err());
    }

    #[test]
    fn ids_and_extra_options() {
        assert_eq!(parse_id("", "UID").unwrap(), None);
        assert_eq!(parse_id(" 1001 ", "UID").unwrap(), Some(1001));
        assert!(parse_id("alice", "UID").is_err());
        assert_eq!(
            parse_extra_options(" vers=3.0, ro ,,").unwrap(),
            vec!["vers=3.0".to_string(), "ro".to_string()]
        );
        assert!(parse_extra_options("password=x").is_err());
        assert!(parse_extra_options("vers=3.0 ro").is_err());
    }
}
