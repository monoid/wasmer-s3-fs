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

    /// Fetches the full body of `key`.
    pub fn get(&self, key: &str) -> FsResult<Vec<u8>> {
        let bytes = self
            .client
            .objects()
            .get(&self.bucket, key)
            .send()
            .map_err(to_fs)?
            .bytes()
            .map_err(to_fs)?;
        Ok(bytes.to_vec())
    }

    /// Stores `body` under `key`.
    pub fn put(&self, key: &str, body: Vec<u8>) -> FsResult<()> {
        self.client
            .objects()
            .put(&self.bucket, key)
            .body_bytes(body)
            .send()
            .map_err(to_fs)?;
        Ok(())
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

/// Maps an `s3` transport/API error into an [`FsError`].
///
/// TODO: distinguish cases (e.g. a 404 -> `EntryNotFound`); for now everything
/// collapses to `IOError`, which is enough for this experimental prototype.
fn to_fs(_err: s3::Error) -> FsError {
    FsError::IOError
}
