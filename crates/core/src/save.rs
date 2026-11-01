//! Verified, atomic saving with backups.
//!
//! Rules (see docs/00-investigation.md §5):
//! - an unedited document is never re-serialized: saving in place is a no-op and
//!   "save as" copies the original bytes;
//! - the container format never changes silently;
//! - new bytes must re-parse (caller-supplied validator) before anything on disk
//!   is touched;
//! - an existing target is backed up first, then replaced by temp file + fsync +
//!   rename, and the result is read back and compared.

use crate::format::{detect, Format};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// What was on disk when we last read or wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DiskStamp {
    len: u64,
    modified: Option<SystemTime>,
}

impl DiskStamp {
    fn of(path: &Path) -> std::io::Result<Self> {
        let m = fs::metadata(path)?;
        Ok(DiskStamp {
            len: m.len(),
            modified: m.modified().ok(),
        })
    }
}

/// A document file as opened from disk. Holds the exact original bytes.
#[derive(Debug)]
pub struct OpenedFile {
    path: PathBuf,
    format: Format,
    original: Vec<u8>,
    stamp: DiskStamp,
}

impl OpenedFile {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SaveError> {
        let path = path.as_ref().to_path_buf();
        let original = fs::read(&path).map_err(|e| SaveError::Io(path.clone(), e))?;
        let format = detect(&original).map_err(SaveError::Detect)?;
        let stamp = DiskStamp::of(&path).map_err(|e| SaveError::Io(path.clone(), e))?;
        Ok(OpenedFile {
            path,
            format,
            original,
            stamp,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn format(&self) -> Format {
        self.format
    }

    /// Bytes exactly as last read from / written to disk.
    pub fn bytes(&self) -> &[u8] {
        &self.original
    }

    /// True if someone else changed the file since we opened or saved it.
    pub fn changed_on_disk(&self) -> bool {
        DiskStamp::of(&self.path)
            .map(|s| s != self.stamp)
            .unwrap_or(true)
    }
}

#[derive(Debug, Clone)]
pub struct SaveOptions {
    pub backup_dir: PathBuf,
    /// Permit writing a different container format than the original (HWP <-> HWPX).
    pub allow_format_change: bool,
    /// Overwrite even if the target changed on disk since it was opened.
    pub overwrite_external_changes: bool,
}

impl Default for SaveOptions {
    fn default() -> Self {
        SaveOptions {
            backup_dir: default_backup_dir(),
            allow_format_change: false,
            overwrite_external_changes: false,
        }
    }
}

pub fn default_backup_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("hwpedit/backups")
}

#[derive(Debug, PartialEq, Eq)]
pub enum SaveOutcome {
    /// Saving in place with no edits: nothing was written.
    Unchanged,
    /// "Save as" with no edits: the original bytes were copied verbatim.
    CopiedOriginal { backup: Option<PathBuf> },
    /// New serialized bytes were written.
    Written { backup: Option<PathBuf> },
}

#[derive(Debug)]
pub enum SaveError {
    Io(PathBuf, std::io::Error),
    Detect(crate::format::DetectError),
    Serialize(String),
    /// Serializer output is not the format it should be.
    FormatMismatch {
        expected: Format,
        got: Option<Format>,
    },
    /// Re-parsing the new bytes failed; nothing was written.
    Validation(String),
    ChangedOnDisk(PathBuf),
    /// Read-back after rename differs from what we wrote.
    ReadBackMismatch(PathBuf),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Io(p, e) => write!(f, "{}: {}", p.display(), e),
            SaveError::Detect(e) => write!(f, "{e}"),
            SaveError::Serialize(e) => write!(f, "직렬화 실패: {e}"),
            SaveError::FormatMismatch { expected, got } => {
                write!(
                    f,
                    "저장 형식이 맞지 않습니다 (기대 {expected:?}, 결과 {got:?})"
                )
            }
            SaveError::Validation(e) => write!(f, "저장 결과 검증 실패, 저장하지 않았습니다: {e}"),
            SaveError::ChangedOnDisk(p) => {
                write!(
                    f,
                    "{}: 연 뒤에 다른 프로그램이 파일을 바꿨습니다",
                    p.display()
                )
            }
            SaveError::ReadBackMismatch(p) => {
                write!(f, "{}: 저장 후 다시 읽은 내용이 다릅니다", p.display())
            }
        }
    }
}

impl std::error::Error for SaveError {}

/// Save `file` to `target`.
///
/// `edited` is false when the document model has no user changes; then the
/// serializer is never called. `serialize` produces bytes in `format`;
/// `validate` re-parses them and returns an error message on failure.
pub fn save(
    file: &mut OpenedFile,
    target: &Path,
    format: Format,
    edited: bool,
    serialize: impl FnOnce(Format) -> Result<Vec<u8>, String>,
    validate: impl Fn(&[u8]) -> Result<(), String>,
    opts: &SaveOptions,
) -> Result<SaveOutcome, SaveError> {
    if format != file.format && !opts.allow_format_change {
        return Err(SaveError::FormatMismatch {
            expected: file.format,
            got: Some(format),
        });
    }
    let in_place = same_file(target, &file.path);
    if in_place && !opts.overwrite_external_changes && file.changed_on_disk() {
        return Err(SaveError::ChangedOnDisk(target.to_path_buf()));
    }

    let unedited_same_format = !edited && format == file.format;
    if unedited_same_format && in_place {
        return Ok(SaveOutcome::Unchanged);
    }

    let bytes = if unedited_same_format {
        file.original.clone()
    } else {
        let b = serialize(format).map_err(SaveError::Serialize)?;
        let got = detect(&b).ok();
        if got != Some(format) {
            return Err(SaveError::FormatMismatch {
                expected: format,
                got,
            });
        }
        validate(&b).map_err(SaveError::Validation)?;
        b
    };

    let backup = if target.exists() {
        Some(backup(target, &opts.backup_dir)?)
    } else {
        None
    };
    atomic_write(target, &bytes)?;
    let back = fs::read(target).map_err(|e| SaveError::Io(target.to_path_buf(), e))?;
    if back != bytes {
        return Err(SaveError::ReadBackMismatch(target.to_path_buf()));
    }

    let stamp = DiskStamp::of(target).map_err(|e| SaveError::Io(target.to_path_buf(), e))?;
    *file = OpenedFile {
        path: target.to_path_buf(),
        format,
        original: bytes,
        stamp,
    };
    Ok(if unedited_same_format {
        SaveOutcome::CopiedOriginal { backup }
    } else {
        SaveOutcome::Written { backup }
    })
}

/// First save of a document that has no file yet (a new document).
/// Same checks as [`save`]: format, re-parse, backup of an existing target,
/// atomic write and read-back. Returns the opened file and the backup, if any.
pub fn save_new(
    target: &Path,
    format: Format,
    bytes: Vec<u8>,
    validate: impl Fn(&[u8]) -> Result<(), String>,
    opts: &SaveOptions,
) -> Result<(OpenedFile, Option<PathBuf>), SaveError> {
    let got = detect(&bytes).ok();
    if got != Some(format) {
        return Err(SaveError::FormatMismatch {
            expected: format,
            got,
        });
    }
    validate(&bytes).map_err(SaveError::Validation)?;
    let backup = if target.exists() {
        Some(backup(target, &opts.backup_dir)?)
    } else {
        None
    };
    atomic_write(target, &bytes)?;
    let back = fs::read(target).map_err(|e| SaveError::Io(target.to_path_buf(), e))?;
    if back != bytes {
        return Err(SaveError::ReadBackMismatch(target.to_path_buf()));
    }
    let stamp = DiskStamp::of(target).map_err(|e| SaveError::Io(target.to_path_buf(), e))?;
    Ok((
        OpenedFile {
            path: target.to_path_buf(),
            format,
            original: bytes,
            stamp,
        },
        backup,
    ))
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// Copy `path` into `dir` as `<unix-millis>-<file name>`. Returns the backup path.
pub fn backup(path: &Path, dir: &Path) -> Result<PathBuf, SaveError> {
    fs::create_dir_all(dir).map_err(|e| SaveError::Io(dir.to_path_buf(), e))?;
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut dest = dir.join(format!("{ms}-{name}"));
    let mut n = 1;
    while dest.exists() {
        dest = dir.join(format!("{ms}-{n}-{name}"));
        n += 1;
    }
    fs::copy(path, &dest).map_err(|e| SaveError::Io(dest.clone(), e))?;
    File::open(&dest)
        .and_then(|f| f.sync_all())
        .map_err(|e| SaveError::Io(dest.clone(), e))?;
    Ok(dest)
}

fn atomic_write(target: &Path, bytes: &[u8]) -> Result<(), SaveError> {
    let io = |e| SaveError::Io(target.to_path_buf(), e);
    let dir = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.hwpedit-{}.tmp", std::process::id()));
    let result = (|| {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        if let Ok(m) = fs::metadata(target) {
            f.set_permissions(m.permissions())?;
        }
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, target)?;
        File::open(dir)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hwp_bytes(tag: u8) -> Vec<u8> {
        let mut b = b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1".to_vec();
        b.extend_from_slice(&[tag; 64]);
        b
    }

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(name: &str) -> Self {
            let p =
                std::env::temp_dir().join(format!("hwpedit-test-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
        fn opts(&self) -> SaveOptions {
            SaveOptions {
                backup_dir: self.0.join("backups"),
                ..Default::default()
            }
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn never(_: Format) -> Result<Vec<u8>, String> {
        panic!("serializer must not run for unedited documents")
    }
    fn ok(_: &[u8]) -> Result<(), String> {
        Ok(())
    }

    #[test]
    fn unedited_in_place_writes_nothing() {
        let t = Tmp::new("noop");
        let p = t.0.join("a.hwp");
        fs::write(&p, hwp_bytes(1)).unwrap();
        let mut f = OpenedFile::open(&p).unwrap();
        let r = save(&mut f, &p, Format::Hwp, false, never, ok, &t.opts()).unwrap();
        assert_eq!(r, SaveOutcome::Unchanged);
        assert!(!t.0.join("backups").exists());
    }

    #[test]
    fn unedited_save_as_copies_original_bytes() {
        let t = Tmp::new("copy");
        let p = t.0.join("a.hwp");
        let q = t.0.join("b.hwp");
        fs::write(&p, hwp_bytes(2)).unwrap();
        let mut f = OpenedFile::open(&p).unwrap();
        let r = save(&mut f, &q, Format::Hwp, false, never, ok, &t.opts()).unwrap();
        assert_eq!(r, SaveOutcome::CopiedOriginal { backup: None });
        assert_eq!(fs::read(&q).unwrap(), hwp_bytes(2));
        assert_eq!(f.path(), q.as_path());
    }

    #[test]
    fn edited_save_backs_up_and_replaces() {
        let t = Tmp::new("edit");
        let p = t.0.join("a.hwp");
        fs::write(&p, hwp_bytes(3)).unwrap();
        let mut f = OpenedFile::open(&p).unwrap();
        let r = save(
            &mut f,
            &p,
            Format::Hwp,
            true,
            |_| Ok(hwp_bytes(4)),
            ok,
            &t.opts(),
        )
        .unwrap();
        let SaveOutcome::Written { backup: Some(b) } = r else {
            panic!("{r:?}")
        };
        assert_eq!(fs::read(&b).unwrap(), hwp_bytes(3));
        assert_eq!(fs::read(&p).unwrap(), hwp_bytes(4));
        assert_eq!(f.bytes(), hwp_bytes(4).as_slice());
        // no temp file left behind
        assert_eq!(fs::read_dir(&t.0).unwrap().count(), 2);
    }

    #[test]
    fn validation_failure_leaves_file_untouched() {
        let t = Tmp::new("invalid");
        let p = t.0.join("a.hwp");
        fs::write(&p, hwp_bytes(5)).unwrap();
        let mut f = OpenedFile::open(&p).unwrap();
        let e = save(
            &mut f,
            &p,
            Format::Hwp,
            true,
            |_| Ok(hwp_bytes(6)),
            |_| Err("bad".into()),
            &t.opts(),
        )
        .unwrap_err();
        assert!(matches!(e, SaveError::Validation(_)));
        assert_eq!(fs::read(&p).unwrap(), hwp_bytes(5));
        assert!(!t.0.join("backups").exists());
    }

    #[test]
    fn refuses_silent_format_change() {
        let t = Tmp::new("fmt");
        let p = t.0.join("a.hwp");
        fs::write(&p, hwp_bytes(7)).unwrap();
        let mut f = OpenedFile::open(&p).unwrap();
        let e = save(
            &mut f,
            &p,
            Format::Hwpx,
            true,
            |_| Ok(vec![]),
            ok,
            &t.opts(),
        )
        .unwrap_err();
        assert!(matches!(e, SaveError::FormatMismatch { .. }));
        // serializer output in the wrong format is rejected too
        let e = save(
            &mut f,
            &p,
            Format::Hwp,
            true,
            |_| Ok(b"PK\x03\x04junkjunk".to_vec()),
            ok,
            &t.opts(),
        )
        .unwrap_err();
        assert!(matches!(e, SaveError::FormatMismatch { .. }));
        assert_eq!(fs::read(&p).unwrap(), hwp_bytes(7));
    }

    #[test]
    fn save_new_writes_validates_and_backs_up_existing() {
        let t = Tmp::new("new");
        let p = t.0.join("new.hwp");
        let (f, b) = save_new(&p, Format::Hwp, hwp_bytes(11), ok, &t.opts()).unwrap();
        assert_eq!(b, None);
        assert_eq!(f.bytes(), hwp_bytes(11).as_slice());
        assert_eq!(fs::read(&p).unwrap(), hwp_bytes(11));
        // overwriting an existing file backs it up first
        let (_, b) = save_new(&p, Format::Hwp, hwp_bytes(12), ok, &t.opts()).unwrap();
        assert_eq!(fs::read(b.unwrap()).unwrap(), hwp_bytes(11));
        // wrong format or failed validation write nothing
        assert!(save_new(&p, Format::Hwpx, hwp_bytes(13), ok, &t.opts()).is_err());
        assert!(save_new(
            &p,
            Format::Hwp,
            hwp_bytes(14),
            |_| Err("x".into()),
            &t.opts()
        )
        .is_err());
        assert_eq!(fs::read(&p).unwrap(), hwp_bytes(12));
    }

    #[test]
    fn refuses_to_overwrite_external_change() {
        let t = Tmp::new("ext");
        let p = t.0.join("a.hwp");
        fs::write(&p, hwp_bytes(8)).unwrap();
        let mut f = OpenedFile::open(&p).unwrap();
        let mut longer = hwp_bytes(9);
        longer.push(0);
        fs::write(&p, &longer).unwrap();
        let e = save(
            &mut f,
            &p,
            Format::Hwp,
            true,
            |_| Ok(hwp_bytes(10)),
            ok,
            &t.opts(),
        )
        .unwrap_err();
        assert!(matches!(e, SaveError::ChangedOnDisk(_)));
        assert_eq!(fs::read(&p).unwrap(), longer);
    }
}
