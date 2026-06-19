//! A thin wrapper over the blocking `s3` client, bound to a single bucket.
//!
//! Its only job is to collapse the verbose
//! `client.objects().<op>(&bucket, key)…send()` chains into short, fallible
//! calls. It deals in raw byte objects; typed directory (de)serialization stays
//! in the filesystem layer.
//!
//! Only the directory/tree operations go through this type for now — the file
//! I/O path (ranged reads, multipart writes) still talks to the `s3` client
//! directly in [`super::file`].

use s3::BlockingClient;
use virtual_fs::{FsError, Result as FsResult};

/// Outcome of a conditional (compare-and-swap) write.
///
/// A lost CAS race is an *expected* result, not an error, so it is reported as
/// [`CasOutcome::Conflict`] rather than through `Err`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CasOutcome {
    /// The conditional write succeeded.
    Written,
    /// The precondition failed (`412`): another writer won the race.
    Conflict,
}

pub(crate) struct Versioned<T> {
    inner: T,
    etag: String,
}

impl<T> Versioned<T> {
    pub(crate) fn new(inner: T, etag: String) -> Self {
        Self { inner, etag }
    }

    pub(crate) fn into_inner(self) -> (T, String) {
        (self.inner, self.etag)
    }

    pub(crate) fn etag(&self) -> &str {
        &self.etag
    }
}

impl<T> AsRef<T> for Versioned<T> {
    fn as_ref(&self) -> &T {
        &self.inner
    }
}

pub(crate) struct ObjectStore {
    bucket: String,
    client: BlockingClient,
}

impl ObjectStore {
    pub fn new(bucket: String, client: BlockingClient) -> Self {
        Self { bucket, client }
    }

    /// The underlying client, for the call sites not yet migrated (file I/O).
    pub fn client(&self) -> &BlockingClient {
        &self.client
    }

    /// The bucket this store is bound to.
    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// Creates the backing bucket.
    pub fn create_bucket(&self) -> FsResult<()> {
        self.client
            .buckets()
            .create(self.bucket.clone())
            .send()
            .map_err(to_fs)?;
        Ok(())
    }

    /// Fetches the full body of `key` together with its ETag.
    pub fn get(&self, key: &str) -> FsResult<Versioned<Vec<u8>>> {
        let req = self
            .client
            .objects()
            .get(&self.bucket, key)
            .send()
            .map_err(to_fs)?;
        let etag = req.etag.clone().ok_or(FsError::IOError)?;
        let bytes = req.bytes().map_err(to_fs)?;
        Ok(Versioned::new(bytes.to_vec(), etag))
    }

    /// Stores `body` under `key` only if its current ETag is `etag`
    /// (`If-Match`). A precondition failure is reported as
    /// [`CasOutcome::Conflict`].
    pub fn put_if_match(&self, key: &str, body: Vec<u8>, etag: &str) -> FsResult<CasOutcome> {
        let res = self
            .client
            .objects()
            .put(&self.bucket, key)
            .if_match(etag.to_owned())
            .map_err(to_fs)?
            .body_bytes(body)
            .send();
        classify_cas(res)
    }

    /// Stores `body` under `key` only if it does not already exist
    /// (`If-None-Match: *`). An existing object is reported as
    /// [`CasOutcome::Conflict`].
    pub fn put_if_none_match(&self, key: &str, body: Vec<u8>) -> FsResult<CasOutcome> {
        let res = self
            .client
            .objects()
            .put(&self.bucket, key)
            .if_none_match("*")
            .map_err(to_fs)?
            .body_bytes(body)
            .send();
        classify_cas(res)
    }

    /// Deletes `key`.
    pub fn delete(&self, key: &str) -> FsResult<()> {
        self.client
            .objects()
            .delete(&self.bucket, key)
            .send()
            .map_err(to_fs)?;
        Ok(())
    }
}

/// Classifies the result of a conditional write: success, a `412` precondition
/// failure (a CAS conflict), or a genuine error.
fn classify_cas<T>(res: Result<T, s3::Error>) -> FsResult<CasOutcome> {
    match res {
        Ok(_) => Ok(CasOutcome::Written),
        Err(err) if is_precondition_failed(&err) => Ok(CasOutcome::Conflict),
        Err(err) => Err(to_fs(err)),
    }
}

/// Whether `err` is an S3 `412 Precondition Failed` response.
fn is_precondition_failed(err: &s3::Error) -> bool {
    matches!(err, s3::Error::Api { status, .. } if status.as_u16() == 412)
}

/// Maps an `s3` transport/API error into an [`FsError`].
///
/// TODO: distinguish more cases (e.g. a 404 -> `EntryNotFound`); for now
/// everything collapses to `IOError`, which is enough for this experimental
/// prototype. CAS conflicts are handled separately via [`classify_cas`].
fn to_fs(_err: s3::Error) -> FsError {
    FsError::IOError
}
