mod file;
mod impls;
mod store;
mod tree;

pub use file::S3VirtualFile;

use std::{fmt, path::Component};
use std::path::Path;

use virtual_fs::{FsError, OpenOptionsConfig, Result as FsResult};

use self::store::ObjectStore;
use self::tree::{DirObj, ObjName};

pub const ROOT_OBJ_NAME: &str = "d_root";

pub struct S3FileSystem {
    root_dir: String,
    store: ObjectStore,
}

impl S3FileSystem {
    pub fn new(bucket: String, client: s3::BlockingClient) -> Self {
        Self {
            root_dir: ROOT_OBJ_NAME.to_owned(),
            store: ObjectStore::new(bucket, client),
        }
    }

    // Create the new bucket, initialize it and return the client.
    pub fn init(bucket: String, client: s3::BlockingClient) -> Self {
        let fs = Self::new(bucket, client);
        fs.store.create_bucket().unwrap();
        fs.put_dir(&ObjName::root(), &DirObj::default()).unwrap();
        fs
    }

    fn resolve_dir_ref(&self, path: &Path) -> FsResult<ObjName> {
        let mut obj_name = ObjName::root();
        for comp in path.components() {
            if !matches!(comp, Component::RootDir) {
                // TODO we assume the components are "normal".
                obj_name =
                    self.resolve_dir_component(&obj_name, &comp.as_os_str().to_string_lossy())?;
            }
        }

        Ok(obj_name)
    }

    fn resolve_dir(&self, path: &Path) -> FsResult<DirObj> {
        let obj_name = self.resolve_dir_ref(path)?;
        self.load_dir(&obj_name)
    }

    fn resolve_dir_component(&self, parent_name: &ObjName, component: &str) -> FsResult<ObjName> {
        let parent = self.load_dir(parent_name)?;
        let obj_ref = parent
            .get(component)
            .ok_or_else(|| FsError::EntryNotFound)?;
        if !matches!(obj_ref.obj_name, ObjName::Dir(_)) {
            Err(FsError::InvalidInput)
        } else {
            Ok(obj_ref.obj_name.clone())
        }
    }

    fn load_dir(&self, obj_name: &ObjName) -> FsResult<DirObj> {
        // Guard against loading a file as a directory: the object content might
        // happen to deserialize as a `DirObj`, so the name's type is the only
        // reliable check.
        if !matches!(obj_name, ObjName::Dir(_)) {
            return Err(FsError::InvalidInput);
        }
        let data = self.store.get(&obj_name.to_string())?;
        DirObj::deserialize(&data)
    }

    fn put_dir(&self, obj_name: &ObjName, obj: &DirObj) -> FsResult<()> {
        self.store.put(&obj_name.to_string(), obj.serialize()?)
    }

    fn update_dir(
        &self,
        obj_name: &ObjName,
        function: impl Fn(DirObj) -> FsResult<DirObj>,
    ) -> FsResult<DirObj> {
        // TODO CAS update
        let dir_obj = self.load_dir(obj_name)?;
        let modified_dir_obj = function(dir_obj)?;

        self.put_dir(obj_name, &modified_dir_obj)?;

        Ok(modified_dir_obj)
    }

    /// Opens `path` according to `conf`, returning the concrete file enum.
    ///
    /// Only two flag combinations are accepted:
    ///
    /// * read-only — opens an existing file for reading;
    /// * write + create — creates a brand new file.
    ///
    /// Everything else (appending, opening an existing file for writing, etc.)
    /// is rejected, matching the design's "new files, written whole" model.
    fn open_file(&self, path: &std::path::Path, conf: &OpenOptionsConfig) -> FsResult<S3VirtualFile> {
        let parent = path.parent().ok_or(FsError::InvalidInput)?;
        let parent_ref = self.resolve_dir_ref(parent)?;
        let name = path
            .file_name()
            .ok_or(FsError::InvalidInput)?
            .to_string_lossy()
            .to_string();

        let parent_dir = self.load_dir(&parent_ref)?;
        let existing = parent_dir.get(&name);

        // Read-only open of an existing file.
        if conf.read && !conf.write && !conf.append && !conf.create && !conf.create_new {
            let ent = existing.ok_or(FsError::EntryNotFound)?;
            if !matches!(ent.obj_name, ObjName::File(_)) {
                return Err(FsError::InvalidInput);
            }
            return Ok(S3VirtualFile::new_read(
                self.store.client().clone(),
                self.store.bucket().to_owned(),
                ent.obj_name.clone(),
                ent.len,
                ent.ctime,
            ));
        }

        // Write + create of a brand new file.
        if conf.write && !conf.append && (conf.create || conf.create_new) {
            // Only new files can be written (no updates, see design limitations).
            if existing.is_some() {
                return Err(FsError::AlreadyExists);
            }

            // File I/O is not part of `ObjectStore` yet; talk to the client
            // directly for the multipart upload.
            let obj_name = ObjName::gen_file();
            let upload_id = self
                .store
                .client()
                .objects()
                .create_multipart_upload(self.store.bucket(), obj_name.to_string())
                .send()
                .map_err(|_| FsError::IOError)?
                .upload_id;

            return Ok(S3VirtualFile::new_write(
                self.store.client().clone(),
                self.store.bucket().to_owned(),
                obj_name,
                upload_id,
                parent_ref,
                name,
                timestamp(),
            ));
        }

        Err(FsError::Unsupported)
    }
}

impl fmt::Debug for S3FileSystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3FileSystem")
            .field("root_dir", &self.root_dir)
            .finish_non_exhaustive()
    }
}

pub(crate) fn timestamp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let start = SystemTime::now();
    let since_the_epoch = start
        .duration_since(UNIX_EPOCH)
        .expect("time should go forward");
    since_the_epoch.as_secs()
}
