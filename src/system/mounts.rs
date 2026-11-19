//! What is mounted right now, and the combined view shown in the Mounts tab.

use std::fs;
use std::path::Path;

use super::SysCtx;
use super::fstab::{self, same_path};
use crate::protocol::is_managed_fs_type;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveMount {
    pub source: String,
    pub target: String,
    pub fstype: String,
}

/// Parses the /proc/self/mounts format.
pub fn parse(content: &str) -> Vec<ActiveMount> {
    content
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(' ');
            Some(ActiveMount {
                source: fstab::unescape(fields.next()?),
                target: fstab::unescape(fields.next()?),
                fstype: fields.next()?.to_string(),
            })
        })
        .collect()
}

/// Currently mounted filesystems; empty where the mounts file does not exist
/// (non-Linux development machines).
pub fn active(ctx: &SysCtx) -> Vec<ActiveMount> {
    fs::read_to_string(&ctx.mounts_file)
        .map(|c| parse(&c))
        .unwrap_or_default()
}

pub fn is_mounted(ctx: &SysCtx, dir: &Path) -> bool {
    let dir = dir.to_string_lossy();
    active(ctx).iter().any(|m| same_path(&m.target, &dir))
}

/// One network share as shown in the Mounts tab: mounted, in fstab, or both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareStatus {
    pub source: String,
    pub target: String,
    pub fstype: String,
    pub mounted: bool,
    pub in_fstab: bool,
    /// The fstab entry was written by this tool.
    pub managed: bool,
    /// fstab options, when there is an entry.
    pub options: String,
}

/// Merges fstab entries and live mounts of the supported network types.
pub fn inventory(ctx: &SysCtx) -> Result<Vec<ShareStatus>, String> {
    let mut rows: Vec<ShareStatus> = fstab::read(&ctx.fstab)?
        .into_iter()
        .filter(|e| is_managed_fs_type(&e.fstype))
        .map(|e| ShareStatus {
            source: e.source,
            target: e.target,
            fstype: e.fstype,
            mounted: false,
            in_fstab: true,
            managed: e.managed,
            options: e.options,
        })
        .collect();
    for mount in active(ctx) {
        if !is_managed_fs_type(&mount.fstype) {
            continue;
        }
        match rows
            .iter_mut()
            .find(|r| same_path(&r.target, &mount.target))
        {
            Some(row) => row.mounted = true,
            None => rows.push(ShareStatus {
                source: mount.source,
                target: mount.target,
                fstype: mount.fstype,
                mounted: true,
                in_fstab: false,
                managed: false,
                options: String::new(),
            }),
        }
    }
    rows.sort_by(|a, b| a.target.cmp(&b.target));
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_mounts() {
        let mounts = parse(
            "proc /proc proc rw,nosuid 0 0\n\
             //nas3/cv2 /mnt/my\\040nas cifs rw,relatime,vers=3.1.1 0 0\n",
        );
        assert_eq!(mounts.len(), 2);
        assert_eq!(mounts[1].target, "/mnt/my nas");
        assert_eq!(mounts[1].fstype, "cifs");
    }

    #[test]
    fn inventory_merges_fstab_and_live_mounts() {
        let dir = std::env::temp_dir().join(format!("fstab-auto-inv-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let fstab_path = dir.join("fstab");
        let mounts_path = dir.join("mounts");
        fs::write(
            &fstab_path,
            format!(
                "UUID=x / ext4 defaults 0 1\n{}\n//nas/a /mnt/a cifs credentials=/c 0 0\n//nas/b /mnt/b cifs guest 0 0\n",
                fstab::MARKER
            ),
        )
        .unwrap();
        fs::write(
            &mounts_path,
            "//nas/a /mnt/a cifs rw 0 0\n//nas/tmp /mnt/tmp cifs rw 0 0\n/dev/sda1 / ext4 rw 0 0\n",
        )
        .unwrap();
        let mut ctx = SysCtx::new(fstab_path, dir.join("creds"), false);
        ctx.mounts_file = mounts_path;

        let rows = inventory(&ctx).unwrap();
        let summary: Vec<_> = rows
            .iter()
            .map(|r| (r.target.as_str(), r.mounted, r.in_fstab, r.managed))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("/mnt/a", true, true, true),
                ("/mnt/b", false, true, false),
                ("/mnt/tmp", true, false, false),
            ]
        );
        assert!(is_mounted(&ctx, Path::new("/mnt/tmp/")));
        assert!(!is_mounted(&ctx, Path::new("/mnt/b")));
        fs::remove_dir_all(&dir).unwrap();
    }
}
