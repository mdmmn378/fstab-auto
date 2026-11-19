//! Protocol-independent description of a mount the user asked for.

use std::path::PathBuf;

/// Network filesystem protocols. Only CIFS is implemented today; the wizard
/// lists the planned ones so the extension point stays visible.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProtocolKind {
    Cifs,
}

/// A row in the wizard's protocol picker.
pub struct ProtocolChoice {
    pub label: &'static str,
    pub description: &'static str,
    pub kind: Option<ProtocolKind>,
}

pub const PROTOCOL_CHOICES: &[ProtocolChoice] = &[
    ProtocolChoice {
        label: "CIFS / SMB",
        description: "Windows shares, Synology, QNAP, TrueNAS, Samba",
        kind: Some(ProtocolKind::Cifs),
    },
    ProtocolChoice {
        label: "NFS",
        description: "coming soon",
        kind: None,
    },
    ProtocolChoice {
        label: "SSHFS",
        description: "coming soon",
        kind: None,
    },
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Persistence {
    /// Mounted now; gone after a reboot or `umount`.
    Temporary,
    /// Written to fstab so it comes back on every boot.
    Permanent,
}

/// Everything needed to perform one mount. Deliberately not `Debug`: it
/// carries the password.
#[derive(Clone)]
pub struct MountRequest {
    pub protocol: ProtocolKind,
    pub host: String,
    pub share: String,
    /// Empty means guest access.
    pub username: String,
    pub password: String,
    pub domain: String,
    pub mount_dir: PathBuf,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub extra_options: Vec<String>,
    pub persistence: Persistence,
}

impl MountRequest {
    pub fn is_guest(&self) -> bool {
        self.username.is_empty()
    }
}
