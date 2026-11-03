use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// 退避したチャンクのファイル内の位置。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Region {
    pub offset: u64,
    pub len: u64,
}

/// 一時ファイル。所有者だけが読める専用ディレクトリに作り、名前を残さない。
///
/// * Unix: 作成直後に unlink する。以降はこのハンドルからしか辿れず、異常終了しても残らない。
/// * Windows: `FILE_FLAG_DELETE_ON_CLOSE` で、閉じた時点(異常終了を含む)に OS が削除する。
pub(crate) struct SpillFile {
    io: Mutex<Io>,
}

struct Io {
    file: File,
    end: u64,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

impl SpillFile {
    pub fn create(dir: &Path) -> io::Result<Self> {
        create_private_dir(dir)?;
        let name = format!(
            "result-{}-{}.spill",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        );
        let path = dir.join(name);
        let file = open_private(&path)?;
        #[cfg(unix)]
        fs::remove_file(&path)?;
        Ok(Self {
            io: Mutex::new(Io { file, end: 0 }),
        })
    }

    pub fn append(&self, bytes: &[u8]) -> io::Result<Region> {
        let mut io = self.io.lock().unwrap_or_else(|e| e.into_inner());
        let offset = io.end;
        io.file.seek(SeekFrom::Start(offset))?;
        io.file.write_all(bytes)?;
        io.end = offset + bytes.len() as u64;
        Ok(Region {
            offset,
            len: bytes.len() as u64,
        })
    }

    pub fn read(&self, region: Region) -> io::Result<Vec<u8>> {
        let len = usize::try_from(region.len)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "region too large"))?;
        let mut buf = vec![0u8; len];
        let mut io = self.io.lock().unwrap_or_else(|e| e.into_inner());
        io.file.seek(SeekFrom::Start(region.offset))?;
        io.file.read_exact(&mut buf)?;
        Ok(buf)
    }
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    // %LOCALAPPDATA% 配下は既定でユーザーだけが読める
    fs::create_dir_all(dir)
}

#[cfg(unix)]
fn open_private(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(windows)]
fn open_private(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_and_read_back_regions_in_any_order() {
        let dir = tempfile::tempdir().unwrap();
        let f = SpillFile::create(&dir.path().join("spill")).unwrap();
        let a = f.append(b"hello").unwrap();
        let b = f.append(b"").unwrap();
        let c = f.append(b"world!").unwrap();
        assert_eq!(f.read(c).unwrap(), b"world!");
        assert_eq!(f.read(a).unwrap(), b"hello");
        assert_eq!(f.read(b).unwrap(), b"");
    }

    #[cfg(unix)]
    #[test]
    fn file_has_no_name_and_directory_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let spill_dir = dir.path().join("spill");
        let f = SpillFile::create(&spill_dir).unwrap();
        f.append(b"secret").unwrap();
        assert_eq!(fs::read_dir(&spill_dir).unwrap().count(), 0);
        let mode = fs::metadata(&spill_dir).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o077,
            0,
            "directory must not be accessible to others"
        );
    }

    #[test]
    fn reading_past_the_end_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let f = SpillFile::create(dir.path()).unwrap();
        f.append(b"abc").unwrap();
        assert!(f.read(Region { offset: 1, len: 10 }).is_err());
    }
}
