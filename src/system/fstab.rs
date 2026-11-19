//! Reading and safely editing fstab.
//!
//! Edits are line based: every line this tool did not write is preserved
//! byte for byte, a backup is taken first, and the new file replaces the old
//! one atomically.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Comment written above every entry this tool adds.
pub const MARKER: &str = "# Added by fstab-auto";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FstabEntry {
    pub source: String,
    pub target: String,
    pub fstype: String,
    pub options: String,
    /// Preceded by [`MARKER`], i.e. written by this tool.
    pub managed: bool,
}

/// fstab encodes whitespace inside a field as an octal escape.
pub fn escape(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    for c in field.chars() {
        match c {
            ' ' => out.push_str("\\040"),
            '\t' => out.push_str("\\011"),
            '\n' => out.push_str("\\012"),
            '\\' => out.push_str("\\134"),
            c => out.push(c),
        }
    }
    out
}

/// Reverses [`escape`]; also used for /proc/self/mounts, which shares the
/// encoding.
pub fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 4 <= bytes.len() {
            let digits = &bytes[i + 1..i + 4];
            let value = digits
                .iter()
                .fold(0u32, |acc, b| acc * 8 + u32::from(b.wrapping_sub(b'0')));
            if digits.iter().all(|b| (b'0'..=b'7').contains(b)) && value <= 0xff {
                out.push(value as u8);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_line(line: &str) -> Option<(String, String, String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let mut fields = line.split_whitespace();
    let source = fields.next()?;
    let target = fields.next()?;
    let fstype = fields.next()?;
    let options = fields.next().unwrap_or("defaults");
    Some((
        unescape(source),
        unescape(target),
        fstype.to_string(),
        options.to_string(),
    ))
}

pub fn parse(content: &str) -> Vec<FstabEntry> {
    let mut entries = Vec::new();
    let mut previous = "";
    for line in content.lines() {
        if let Some((source, target, fstype, options)) = parse_line(line) {
            entries.push(FstabEntry {
                source,
                target,
                fstype,
                options,
                managed: previous.trim() == MARKER,
            });
        }
        previous = line;
    }
    entries
}

/// Reads and parses the file; a missing file is an empty table.
pub fn read(path: &Path) -> Result<Vec<FstabEntry>, String> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(parse(&content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

pub fn render_line(source: &str, target: &str, fstype: &str, options: &[String]) -> String {
    let options = if options.is_empty() {
        "defaults".to_string()
    } else {
        escape(&options.join(","))
    };
    format!(
        "{} {} {fstype} {options} 0 0",
        escape(source),
        escape(target)
    )
}

/// Appends a marked entry, leaving existing content untouched.
pub fn add_entry(content: &str, line: &str) -> String {
    let mut out = content.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(MARKER);
    out.push('\n');
    out.push_str(line);
    out.push('\n');
    out
}

/// Removes every entry mounted on `target`, together with its marker comment.
/// Returns the new content and how many entries were removed.
pub fn remove_entry(content: &str, target: &str) -> (String, usize) {
    let mut kept: Vec<&str> = Vec::new();
    let mut removed = 0;
    for line in content.lines() {
        let matches = parse_line(line).is_some_and(|(_, t, _, _)| same_path(&t, target));
        if matches {
            removed += 1;
            if kept.last().is_some_and(|prev| prev.trim() == MARKER) {
                kept.pop();
            }
        } else {
            kept.push(line);
        }
    }
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    (out, removed)
}

pub fn same_path(a: &str, b: &str) -> bool {
    let norm = |p: &str| {
        let trimmed = p.trim_end_matches('/');
        if trimmed.is_empty() { "/" } else { trimmed }.to_string()
    };
    norm(a) == norm(b)
}

/// The `credentials=` file referenced by an option string, if any.
pub fn credentials_path(options: &str) -> Option<PathBuf> {
    unescape(options).split(',').find_map(|opt| {
        opt.strip_prefix("credentials=")
            .or_else(|| opt.strip_prefix("cred="))
            .map(PathBuf::from)
    })
}

pub fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".fstab-auto.bak");
    path.with_file_name(name)
}

/// Copies the current file next to itself; returns `None` when there is
/// nothing to back up yet.
pub fn backup(path: &Path) -> Result<Option<PathBuf>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let dest = backup_path(path);
    fs::copy(path, &dest).map_err(|e| {
        format!(
            "cannot back up {} to {}: {e}",
            path.display(),
            dest.display()
        )
    })?;
    Ok(Some(dest))
}

/// Writes via a temporary file in the same directory and renames it into
/// place, so a crash can never leave a half-written fstab behind.
pub fn write_atomic(path: &Path, content: &str, mode: u32) -> Result<(), String> {
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".fstab-auto.tmp");
    let tmp = path.with_file_name(tmp_name);
    let result = (|| -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)?;
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    result.map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("cannot write {}: {e}", path.display())
    })
}

/// Mode of the existing file, or the conventional 0644 for a new one.
pub fn existing_mode(path: &Path) -> u32 {
    fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o7777)
        .unwrap_or(0o644)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# /etc/fstab: static file system information.
UUID=abcd-1234 / ext4 errors=remount-ro 0 1
/swap.img\tnone\tswap\tsw\t0\t0
//old/share /mnt/old\\040share cifs credentials=/root/.cred,uid=1000 0 0
";

    #[test]
    fn parses_entries_and_escapes() {
        let entries = parse(SAMPLE);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[1].fstype, "swap");
        assert_eq!(entries[2].target, "/mnt/old share");
        assert!(!entries[2].managed);
        assert_eq!(
            credentials_path(&entries[2].options),
            Some(PathBuf::from("/root/.cred"))
        );
    }

    #[test]
    fn escape_round_trips() {
        for s in [
            "/mnt/my share",
            "a\\b",
            "tab\there",
            "plain",
            "trailing\\04",
        ] {
            assert_eq!(unescape(&escape(s)), s);
        }
        assert_eq!(unescape("a\\040b"), "a b");
        assert_eq!(unescape("a\\9"), "a\\9");
    }

    #[test]
    fn add_then_remove_restores_original() {
        let line = render_line(
            "//nas01/projects",
            "/mnt/nas",
            "cifs",
            &["credentials=/etc/c.cred".into(), "uid=1001".into()],
        );
        assert_eq!(
            line,
            "//nas01/projects /mnt/nas cifs credentials=/etc/c.cred,uid=1001 0 0"
        );
        let added = add_entry(SAMPLE, &line);
        let entries = parse(&added);
        assert_eq!(entries.len(), 4);
        assert!(entries[3].managed);
        assert_eq!(entries[3].target, "/mnt/nas");

        let (restored, removed) = remove_entry(&added, "/mnt/nas/");
        assert_eq!(removed, 1);
        assert_eq!(restored, SAMPLE);
    }

    #[test]
    fn add_handles_missing_trailing_newline_and_empty_file() {
        assert_eq!(
            add_entry("", "x y z o 0 0"),
            format!("{MARKER}\nx y z o 0 0\n")
        );
        let added = add_entry("a / ext4 defaults 0 1", "x /y cifs o 0 0");
        assert_eq!(parse(&added).len(), 2);
    }

    #[test]
    fn remove_leaves_unrelated_lines_alone() {
        let (out, removed) = remove_entry(SAMPLE, "/mnt/old share");
        assert_eq!(removed, 1);
        assert!(out.contains("UUID=abcd-1234"));
        assert!(out.contains("# /etc/fstab"));
        assert!(!out.contains("//old/share"));
        let (same, none) = remove_entry(SAMPLE, "/mnt/absent");
        assert_eq!(none, 0);
        assert_eq!(same, SAMPLE);
    }

    #[test]
    fn spaces_are_escaped_when_rendering() {
        let line = render_line("//nas/my share", "/mnt/my share", "cifs", &[]);
        assert_eq!(
            line,
            "//nas/my\\040share /mnt/my\\040share cifs defaults 0 0"
        );
        assert_eq!(parse(&line)[0].source, "//nas/my share");
    }

    #[test]
    fn atomic_write_and_backup() {
        let dir = std::env::temp_dir().join(format!("fstab-auto-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fstab");
        assert_eq!(backup(&path).unwrap(), None);
        write_atomic(&path, "one\n", 0o644).unwrap();
        let bak = backup(&path).unwrap().unwrap();
        write_atomic(&path, "two\n", existing_mode(&path)).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "two\n");
        assert_eq!(fs::read_to_string(&bak).unwrap(), "one\n");
        assert_eq!(existing_mode(&path), 0o644);
        fs::remove_dir_all(&dir).unwrap();
    }
}
