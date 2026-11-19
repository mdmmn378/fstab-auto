//! Protocol-specific knowledge: how to name a source, which mount helper is
//! needed and which options to pass. Adding a protocol means adding a module
//! here and a variant to [`ProtocolKind`].

pub mod cifs;

use std::path::Path;

use crate::model::{MountRequest, ProtocolKind};

/// Where the mount helper gets its credentials from.
pub enum Auth<'a> {
    /// Username in the option string; the password travels out of band.
    Inline,
    /// A root-only credentials file referenced from the option string.
    CredentialsFile(&'a Path),
}

pub fn fs_type(kind: ProtocolKind) -> &'static str {
    match kind {
        ProtocolKind::Cifs => cifs::FS_TYPE,
    }
}

/// Binary that `mount -t <type>` delegates to.
pub fn helper(kind: ProtocolKind) -> &'static str {
    match kind {
        ProtocolKind::Cifs => cifs::HELPER,
    }
}

/// Distribution package that ships the helper, for error hints.
pub fn helper_package(kind: ProtocolKind) -> &'static str {
    match kind {
        ProtocolKind::Cifs => "cifs-utils",
    }
}

pub fn source(req: &MountRequest) -> String {
    match req.protocol {
        ProtocolKind::Cifs => cifs::source(&req.host, &req.share),
    }
}

pub fn options(req: &MountRequest, auth: Auth<'_>) -> Vec<String> {
    match req.protocol {
        ProtocolKind::Cifs => cifs::options(req, auth),
    }
}

/// Filesystem types this tool lists and manages in the Mounts tab.
pub fn is_managed_fs_type(fstype: &str) -> bool {
    matches!(fstype, "cifs" | "smb3")
}
