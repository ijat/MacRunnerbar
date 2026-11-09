//! Safe extraction of the runner release archive.

use std::fs::File;
use std::io::Read;
use std::path::{Component, Path};

use flate2::read::GzDecoder;
use macrunnerbar_core::OsError;
use tar::{Archive, EntryType};

/// Upper bound on the unpacked size (the real runner is a few hundred MB).
const MAX_UNPACKED_BYTES: u64 = 4 * 1024 * 1024 * 1024;

fn err(msg: &str) -> OsError {
    OsError::new(format!("runner archive: {msg}"))
}

/// Whether a symlink at `entry` pointing to `target` stays inside the extraction root (lexically).
fn symlink_stays_inside(entry: &Path, target: &Path) -> bool {
    if target.is_absolute() {
        return false;
    }
    let mut depth: i64 = i64::try_from(entry.components().count()).unwrap_or(0) - 1; // directories above the link
    for c in target.components() {
        match c {
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            _ => return false,
        }
    }
    true
}

/// Extract the `.tar.gz` at `archive` into `dest` (an existing, empty directory). Only regular files,
/// directories and relative in-tree symlinks are accepted; absolute or `..` paths, write-through-symlink
/// attempts, hard links, devices and oversized archives are rejected. Permissions (the exec bit) are kept.
pub(crate) fn extract_tar_gz(archive: &Path, dest: &Path) -> Result<(), OsError> {
    let file = File::open(archive).map_err(|e| err(&format!("cannot open: {:?}", e.kind())))?;
    let mut tar = Archive::new(GzDecoder::new(file));
    tar.set_preserve_permissions(true);
    tar.set_unpack_xattrs(false);
    tar.set_overwrite(false);
    let mut total: u64 = 0;
    let entries = tar
        .entries()
        .map_err(|e| err(&format!("unreadable: {:?}", e.kind())))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| err(&format!("unreadable entry: {:?}", e.kind())))?;
        let path = entry
            .path()
            .map_err(|_| err("entry with an invalid path"))?
            .into_owned();
        match entry.header().entry_type() {
            EntryType::Regular | EntryType::Directory | EntryType::Continuous => {}
            EntryType::Symlink => {
                let target = entry
                    .link_name()
                    .map_err(|_| err("symlink with an invalid target"))?
                    .ok_or_else(|| err("symlink without a target"))?;
                if !symlink_stays_inside(&path, &target) {
                    return Err(err("symlink points outside the archive"));
                }
            }
            // GNU/PAX long-name and extension records are consumed by the tar crate itself.
            EntryType::XGlobalHeader
            | EntryType::XHeader
            | EntryType::GNULongName
            | EntryType::GNULongLink => {
                continue;
            }
            _ => return Err(err("unsupported entry type (link or device)")),
        }
        total = total.saturating_add(entry.size());
        if total > MAX_UNPACKED_BYTES {
            return Err(err("unpacked size limit exceeded"));
        }
        // `unpack_in` refuses `..`, absolute paths and writes that would pass through a symlink out of `dest`.
        match entry.unpack_in(dest) {
            Ok(true) => {}
            Ok(false) => return Err(err("entry escapes the destination")),
            Err(e) => return Err(err(&format!("cannot unpack entry: {:?}", e.kind()))),
        }
    }
    // Drain nothing further; make sure the stream ended cleanly.
    let mut rest = Vec::new();
    tar.into_inner()
        .read_to_end(&mut rest)
        .map_err(|e| err(&format!("trailing data: {:?}", e.kind())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symlink_containment_is_lexical() {
        let p = Path::new;
        assert!(symlink_stays_inside(p("a/b/link"), p("../c")));
        assert!(symlink_stays_inside(p("link"), p("sub/file")));
        assert!(symlink_stays_inside(p("a/link"), p("./x")));
        assert!(!symlink_stays_inside(p("link"), p("../x")));
        assert!(!symlink_stays_inside(p("a/link"), p("../../x")));
        assert!(!symlink_stays_inside(p("a/link"), p("/etc")));
    }
}
