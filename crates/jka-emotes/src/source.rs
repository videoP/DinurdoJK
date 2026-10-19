//! Where a bundle's bytes come from.
//!
//! A `.djk_fnemote` bundle is a ZIP. [`Bundle`](crate::Bundle) never assumes it
//! is a file: it reads through a [`BundleSource`], which only has to hand out
//! independent seekable readers. Two things are shipped here:
//!
//! * [`FileSource`] - a loose file, or a *stored* member of a larger archive
//!   (the packed library), read in place without extracting anything;
//! * the trait itself, which is the seam for other backends. A streaming store
//!   would implement `open` with HTTP range requests (or a sparse on-disk
//!   cache) and `key` from the remote object's identity; catalogue code only
//!   reads the ZIP directory and manifest ([`Bundle::open_index`]), so a cold
//!   bundle costs a few small ranged reads until it is actually played.
//!
//! [`Bundle::open_index`]: crate::Bundle::open_index
use std::{
    fmt::Debug,
    fs::File,
    hash::{Hash, Hasher},
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

/// A seekable byte stream that can move between threads.
pub trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

/// Random access to the bytes of one bundle.
pub trait BundleSource: Send + Sync + Debug {
    /// Total size of the bundle in bytes.
    fn len(&self) -> u64;
    /// A fresh, independent reader positioned at the start. Called once per
    /// operation so concurrent readers never share a cursor.
    fn open(&self) -> io::Result<Box<dyn ReadSeek>>;
    /// Identifies *where* the bytes live. It is hashed together with every ZIP
    /// member's CRC into the bundle's cache identity, so two locations never
    /// share cached audio/poses and a changed bundle never reuses stale ones.
    fn key(&self) -> u64;
    /// Human-readable location for error messages.
    fn describe(&self) -> String;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A file on disk, or a byte range inside one (a stored member of a PK3).
#[derive(Clone, Debug)]
pub struct FileSource {
    path: PathBuf,
    /// `(member name, offset, length)` when the bundle is nested in a larger file.
    packed: Option<(String, u64, u64)>,
    len: u64,
}

impl FileSource {
    /// A whole file.
    pub fn file(path: &Path) -> io::Result<Self> {
        let len = File::open(path)?.metadata()?.len();
        Ok(Self {
            path: path.to_owned(),
            packed: None,
            len,
        })
    }

    /// `length` bytes starting at `offset` in `path`; `member` is the nested
    /// bundle's name inside the archive (only used for identity and messages).
    pub fn packed(path: &Path, member: &str, offset: u64, length: u64) -> io::Result<Self> {
        let size = File::open(path)?.metadata()?.len();
        if offset.checked_add(length).is_none_or(|end| end > size) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "packed bundle outside archive",
            ));
        }
        Ok(Self {
            path: path.to_owned(),
            packed: Some((member.to_owned(), offset, length)),
            len: length,
        })
    }
}

impl BundleSource for FileSource {
    fn len(&self) -> u64 {
        self.len
    }

    fn open(&self) -> io::Result<Box<dyn ReadSeek>> {
        let file = File::open(&self.path)?;
        let (start, length) = self
            .packed
            .as_ref()
            .map_or((0, self.len), |(_, start, length)| (*start, *length));
        let size = file.metadata()?.len();
        if start.checked_add(length).is_none_or(|end| end > size) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "packed bundle outside archive",
            ));
        }
        Ok(Box::new(SliceReader {
            file,
            start,
            length,
            position: 0,
        }))
    }

    fn key(&self) -> u64 {
        let mut hasher = std::hash::DefaultHasher::new();
        self.path.hash(&mut hasher);
        self.packed.hash(&mut hasher);
        hasher.finish()
    }

    fn describe(&self) -> String {
        match &self.packed {
            Some((member, ..)) => format!("{}:{member}", self.path.display()),
            None => self.path.display().to_string(),
        }
    }
}

/// Reads a window of a file as if it were a file of its own.
struct SliceReader {
    file: File,
    start: u64,
    length: u64,
    position: u64,
}

impl Read for SliceReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = (self.length - self.position).min(buffer.len() as u64) as usize;
        self.file
            .seek(SeekFrom::Start(self.start + self.position))?;
        let count = self.file.read(&mut buffer[..count])?;
        self.position += count as u64;
        Ok(count)
    }
}

impl Seek for SliceReader {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let next = match from {
            SeekFrom::Start(n) => n as i128,
            SeekFrom::Current(n) => self.position as i128 + n as i128,
            SeekFrom::End(n) => self.length as i128 + n as i128,
        };
        if next < 0 || next > self.length as i128 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek outside packed bundle",
            ));
        }
        self.position = next as u64;
        Ok(self.position)
    }
}
