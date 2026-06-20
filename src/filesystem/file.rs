//! Adapter implementing [`virtual_fs::VirtualFile`] on top of the blocking
//! `s3` client.
//!
//! The adapter only supports two ways of opening a file (see [`design.md`]):
//!
//! * *reading* an existing file — backed by S3 ranged GET requests, with
//!   [`AsyncSeek`] support;
//! * *creating* a brand new file — backed by an S3 multipart upload that is
//!   only made visible (and registered in its parent directory) once the file
//!   is closed.
//!
//! Every other combination of open flags is rejected by the opener, so seeking
//! is, by construction, only ever available for reads.
//!
//! [`design.md`]: ../../doc/design.md

use std::io::{self, SeekFrom};
use std::pin::Pin;
use std::task::{Context, Poll};

use s3::BlockingClient;
use tokio::io::{AsyncRead, AsyncSeek, AsyncWrite, ReadBuf};
use virtual_fs::{FsError, Result as FsResult, VirtualFile};

use super::store::{self, ObjectStore};
use super::tree::{ObjName, S3FsDirEntry};

/// Minimum size of a non-final S3 multipart part (5 MiB). Buffered writes are
/// flushed as a part once this much data has accumulated.
const PART_SIZE: usize = 5 * 1024 * 1024;

/// A [`VirtualFile`] backed by a single S3 object.
///
/// Only the two open modes described in the module docs are representable; the
/// opener returns an error for anything else.
pub enum S3VirtualFile {
    /// An existing object opened for reading (with seek support).
    ReadOperation(ReadOp),
    /// A new object being created through a multipart upload.
    WriteCreateOperation(WriteCreateOp),
}

impl S3VirtualFile {
    /// Opens an existing object for reading.
    pub fn new_read(
        client: BlockingClient,
        bucket: String,
        obj_name: ObjName,
        len: u64,
        created: u64,
    ) -> Self {
        S3VirtualFile::ReadOperation(ReadOp {
            client,
            bucket,
            key: obj_name.to_string(),
            len,
            created,
            pos: 0,
        })
    }

    /// Starts creating a new object backed by the multipart upload `upload_id`.
    ///
    /// On close the object is finalized and an entry named `name` pointing at
    /// `obj_name` is inserted into the `parent` directory object.
    pub fn new_write(
        store: ObjectStore,
        obj_name: ObjName,
        upload_id: String,
        parent: ObjName,
        name: String,
        created: u64,
    ) -> Self {
        S3VirtualFile::WriteCreateOperation(WriteCreateOp {
            store,
            obj_name,
            upload_id,
            parent,
            name,
            created,
            buffer: Vec::new(),
            parts: Vec::new(),
            written: 0,
            terminal: false,
        })
    }
}

/// State of an object opened for reading.
pub struct ReadOp {
    client: BlockingClient,
    bucket: String,
    /// The S3 key (the stringified object name).
    key: String,
    /// Total object size, learned from the directory entry at open time.
    len: u64,
    /// Creation time, in seconds since the epoch.
    created: u64,
    /// Current read/seek cursor.
    pos: u64,
}

impl ReadOp {
    /// Fills `buf` with bytes starting at the current cursor via a ranged GET.
    fn read_into(&mut self, buf: &mut ReadBuf<'_>) -> io::Result<()> {
        if self.pos >= self.len || buf.remaining() == 0 {
            return Ok(()); // EOF (or no room): leave `buf` untouched.
        }

        // Inclusive range, capped at the last byte of the object.
        let want = buf.remaining() as u64;
        let end = (self.pos + want - 1).min(self.len - 1);

        let bytes = self
            .client
            .objects()
            .get(&self.bucket, &self.key)
            .range_bytes(self.pos, end)
            .map_err(to_io)?
            .send()
            .map_err(to_io)?
            .bytes()
            .map_err(to_io)?;

        buf.put_slice(&bytes);
        self.pos += bytes.len() as u64;
        Ok(())
    }

    /// Moves the cursor; the resolved absolute position is returned.
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let target = match position {
            SeekFrom::Start(n) => n as i128,
            SeekFrom::End(n) => self.len as i128 + n as i128,
            SeekFrom::Current(n) => self.pos as i128 + n as i128,
        };
        if target < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot seek before the start of the file",
            ));
        }
        self.pos = target as u64;
        Ok(self.pos)
    }
}

/// State of a new object being created via a multipart upload.
pub struct WriteCreateOp {
    store: ObjectStore,
    /// Object name (S3 key) of the file being written.
    obj_name: ObjName,
    /// Multipart upload id obtained when the file was opened.
    upload_id: String,
    /// Object name of the parent directory the new file is registered in.
    parent: ObjName,
    /// Name of the new entry within its parent directory.
    name: String,
    /// Creation time, in seconds since the epoch.
    created: u64,
    /// Bytes received but not yet flushed as a part.
    buffer: Vec<u8>,
    /// `(part_number, etag)` of every part uploaded so far.
    parts: Vec<(u32, String)>,
    /// Total number of bytes written.
    written: u64,
    /// Set once the upload has been completed or aborted; further writes and
    /// `Drop`-time finalization are then no-ops.
    terminal: bool,
}

impl WriteCreateOp {
    /// Buffers `data`, flushing full parts as the buffer fills.
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.terminal {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "write to a closed file",
            ));
        }

        self.buffer.extend_from_slice(data);
        self.written += data.len() as u64;

        while self.buffer.len() >= PART_SIZE {
            let part: Vec<u8> = self.buffer.drain(..PART_SIZE).collect();
            self.upload_part(part)?;
        }

        Ok(data.len())
    }

    /// Uploads `body` as the next multipart part.
    fn upload_part(&mut self, body: Vec<u8>) -> io::Result<()> {
        let part_number = self.parts.len() as u32 + 1;
        let out = self
            .store
            .client()
            .objects()
            .upload_part(
                self.store.bucket(),
                self.obj_name.to_string(),
                &self.upload_id,
                part_number,
            )
            .body_bytes(body)
            .send()
            .map_err(to_io)?;
        let etag = out
            .etag
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "upload part returned no etag"))?;
        self.parts.push((part_number, etag));
        Ok(())
    }

    /// Flushes the remaining buffer, completes the upload and registers the new
    /// file in its parent directory. Idempotent.
    fn finish(&mut self) -> io::Result<()> {
        if self.terminal {
            return Ok(());
        }
        // Mark terminal up-front so a failure does not leave us retrying the
        // same broken upload on `Drop`.
        self.terminal = true;

        // S3 rejects a zero-length part, so an empty file can't go through the
        // multipart machinery — abort it and PUT an empty object instead.
        if self.parts.is_empty() && self.buffer.is_empty() {
            self.store
                .client()
                .objects()
                .abort_multipart_upload(
                    self.store.bucket(),
                    self.obj_name.to_string(),
                    &self.upload_id,
                )
                .send()
                .map_err(to_io)?;
            self.store
                .client()
                .objects()
                .put(self.store.bucket(), self.obj_name.to_string())
                .body_bytes(Vec::new())
                .send()
                .map_err(to_io)?;
        } else {
            if !self.buffer.is_empty() {
                let body = std::mem::take(&mut self.buffer);
                self.upload_part(body)?;
            }

            let mut req = self.store.client().objects().complete_multipart_upload(
                self.store.bucket(),
                self.obj_name.to_string(),
                &self.upload_id,
            );
            for (number, etag) in &self.parts {
                req = req.part(*number, etag).map_err(to_io)?;
            }
            req.send().map_err(to_io)?;
        }

        self.register_in_parent()
    }

    /// Aborts the upload, leaving nothing behind in the bucket or the tree.
    fn abort(&mut self) -> io::Result<()> {
        if self.terminal {
            return Ok(());
        }
        self.terminal = true;
        self.store
            .client()
            .objects()
            .abort_multipart_upload(self.store.bucket(), self.obj_name.to_string(), &self.upload_id)
            .send()
            .map_err(to_io)?;
        Ok(())
    }

    /// Inserts the finished file into its parent directory object.
    ///
    /// Goes through the shared `update_dir` CAS loop, so it does not clobber
    /// concurrent changes to the parent and fails (`EntryNotFound`) if the
    /// parent is being deleted (its tombstone is observed on reload).
    fn register_in_parent(&self) -> io::Result<()> {
        let entry = S3FsDirEntry {
            obj_name: self.obj_name.clone(),
            ctime: self.created,
            len: self.written,
        };
        store::update_dir(&self.store, &self.parent, |old_dir| {
            let mut dir = old_dir.clone();
            dir.children.insert(self.name.clone(), entry.clone());
            Ok((dir, ()))
        })
        .map_err(fs_to_io)
    }
}

impl Drop for WriteCreateOp {
    fn drop(&mut self) {
        // The `VirtualFile` contract treats going out of scope as closing the
        // file, so finalize a still-open upload on a best-effort basis.
        let _ = self.finish();
    }
}

impl AsyncRead for S3VirtualFile {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            S3VirtualFile::ReadOperation(r) => Poll::Ready(r.read_into(buf)),
            S3VirtualFile::WriteCreateOperation(_) => {
                Poll::Ready(Err(io::ErrorKind::Unsupported.into()))
            }
        }
    }
}

impl AsyncSeek for S3VirtualFile {
    fn start_seek(self: Pin<&mut Self>, position: SeekFrom) -> io::Result<()> {
        match self.get_mut() {
            // Seeking is supported for reads only.
            S3VirtualFile::ReadOperation(r) => r.seek(position).map(|_| ()),
            S3VirtualFile::WriteCreateOperation(_) => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "cannot seek a file opened for writing",
            )),
        }
    }

    fn poll_complete(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
        match self.get_mut() {
            S3VirtualFile::ReadOperation(r) => Poll::Ready(Ok(r.pos)),
            S3VirtualFile::WriteCreateOperation(_) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "cannot seek a file opened for writing",
            ))),
        }
    }
}

impl AsyncWrite for S3VirtualFile {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            S3VirtualFile::WriteCreateOperation(w) => Poll::Ready(w.write(buf)),
            S3VirtualFile::ReadOperation(_) => Poll::Ready(Err(io::ErrorKind::Unsupported.into())),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Parts are flushed lazily; nothing is forced out before close because
        // S3 multipart parts (other than the last) must be at least 5 MiB.
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            S3VirtualFile::WriteCreateOperation(w) => Poll::Ready(w.finish()),
            S3VirtualFile::ReadOperation(_) => Poll::Ready(Ok(())),
        }
    }
}

impl VirtualFile for S3VirtualFile {
    fn last_accessed(&self) -> u64 {
        0 // Access time is not tracked (see design limitations).
    }

    fn last_modified(&self) -> u64 {
        0 // Modification time is not tracked (see design limitations).
    }

    fn created_time(&self) -> u64 {
        match self {
            S3VirtualFile::ReadOperation(r) => r.created,
            S3VirtualFile::WriteCreateOperation(w) => w.created,
        }
    }

    fn size(&self) -> u64 {
        match self {
            S3VirtualFile::ReadOperation(r) => r.len,
            S3VirtualFile::WriteCreateOperation(w) => w.written,
        }
    }

    fn set_len(&mut self, _new_size: u64) -> FsResult<()> {
        // Files are written whole; resizing is not supported.
        Err(FsError::Unsupported)
    }

    fn unlink(&mut self) -> FsResult<()> {
        match self {
            // Aborting a fresh upload simply discards everything we buffered.
            S3VirtualFile::WriteCreateOperation(w) => w.abort().map_err(FsError::from),
            // Deleting through a read handle is not supported.
            S3VirtualFile::ReadOperation(_) => Err(FsError::Unsupported),
        }
    }

    fn poll_read_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            S3VirtualFile::ReadOperation(r) => {
                Poll::Ready(Ok(r.len.saturating_sub(r.pos) as usize))
            }
            S3VirtualFile::WriteCreateOperation(_) => Poll::Ready(Ok(0)),
        }
    }

    fn poll_write_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            S3VirtualFile::WriteCreateOperation(_) => Poll::Ready(Ok(PART_SIZE)),
            S3VirtualFile::ReadOperation(_) => Poll::Ready(Ok(0)),
        }
    }
}

impl std::fmt::Debug for S3VirtualFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            S3VirtualFile::ReadOperation(r) => f
                .debug_struct("S3VirtualFile::ReadOperation")
                .field("key", &r.key)
                .field("len", &r.len)
                .field("pos", &r.pos)
                .finish(),
            S3VirtualFile::WriteCreateOperation(w) => f
                .debug_struct("S3VirtualFile::WriteCreateOperation")
                .field("obj_name", &w.obj_name.to_string())
                .field("written", &w.written)
                .field("parts", &w.parts.len())
                .finish(),
        }
    }
}

/// Maps an `s3` error into an [`io::Error`].
fn to_io(err: s3::Error) -> io::Error {
    io::Error::new(io::ErrorKind::Other, err.to_string())
}

/// Maps a [`FsError`] into an [`io::Error`] (used for (de)serialization paths).
fn fs_to_io(err: FsError) -> io::Error {
    err.into()
}
