mod impls;
mod tree;

use std::{fmt, path::Component};
use std::path::Path;

use virtual_fs::{FsError, Result as FsResult};

use self::tree::{DirObj, ObjName};

pub const ROOT_OBJ_NAME: &str = "d_root";

pub struct S3FileSystem {
    bucket: String,
    root_dir: String,
    client: s3::BlockingClient,
}

impl S3FileSystem {
    pub fn new(bucket: String, client: s3::BlockingClient) -> Self {
        Self {
            bucket,
            root_dir: ROOT_OBJ_NAME.to_owned(),
            client,
        }
    }

    // Create the new bucket, initialize it and return the client.
    pub fn init(bucket: String, client: s3::BlockingClient) -> Self {
        client.buckets().create(bucket.clone()).send().unwrap();
        let fs = Self::new(bucket, client);
        fs.put_dir(&ObjName::root(), &DirObj::default()).unwrap();
        fs
    }

    fn resolve_dir_ref(&self, path: &Path) -> FsResult<ObjName> {
        let mut obj_name = ObjName::root();
        for comp in path.components() {
            if !matches!(comp, Component::RootDir) {
                // TODO we assume the paths are "normal".
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
        let req = self
            .client
            .objects()
            .get(&self.bucket, &obj_name.to_string())
            .send()
            .unwrap();
        // let etag = req.etag;
        DirObj::deserialize(&req.bytes().unwrap())
    }

    fn put_dir(&self, obj_name: &ObjName, obj: &DirObj) -> FsResult<()> {
        let obj_data = obj.serialize()?;
        let _update_req = self
            .client
            .objects()
            .put(&self.bucket, &obj_name.to_string())
            .body_bytes(obj_data)
            .send()
            .unwrap();
        Ok(())
    }

    fn update_dir(
        &self,
        obj_name: &ObjName,
        function: impl Fn(DirObj) -> FsResult<DirObj>,
    ) -> FsResult<DirObj> {
        // TODO CAS update

        let get_req = self
            .client
            .objects()
            .get(&self.bucket, &obj_name.to_string())
            .send()
            .unwrap();

        let dir_obj = DirObj::deserialize(&get_req.bytes().unwrap())?;
        let modified_dir_obj = function(dir_obj)?;

        self.put_dir(obj_name, &modified_dir_obj)?;

        Ok(modified_dir_obj)
    }
}

impl fmt::Debug for S3FileSystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3FileSystem")
            .field("root_dir", &self.root_dir)
            .finish_non_exhaustive()
    }
}
