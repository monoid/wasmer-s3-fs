use std::fmt;
use std::{collections::HashMap, path::Path};

use futures_core::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;
use virtual_fs::{FileSystem, FsError, Result as FSResult};

const ROOT_DIR: &str = "d_root";

pub struct S3FileSystem {
    bucket: String,
    root_dir: String,
    client: s3::BlockingClient,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DirObj {
    children: HashMap<String, S3FsRef>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct S3FsRef {
    obj_name: String,
    ctime: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum ObjName {
    File(String),
    Dir(String),
}

impl S3FileSystem {
    pub fn new(
        bucket: String,
        client: s3::BlockingClient,
    ) -> Self {
        Self {
            bucket,
            root_dir: ROOT_DIR.to_owned(),
            client,
        }
    }

    fn resolve_root_dir(&self) -> FSResult<DirObj> {
        let req = self.client.objects().get(&self.bucket, &self.root_dir).send().unwrap();
        Ok(serde_json::from_slice(&req.bytes().unwrap()).unwrap())
    }

    fn resolve_dir(&self, path: &Path) -> FSResult<DirObj> {
        let mut dir = self.resolve_root_dir()?;
        for comp in path.components() {
            dir = self.resolve_dir_component(&dir, &comp.as_os_str().to_string_lossy())?;
        }

        Ok(dir)
    }

    fn resolve_dir_component(&self, parent: &DirObj, component: &str) -> FSResult<DirObj> {
        todo!()
    }
}

impl fmt::Debug for S3FileSystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3FileSystem")
            .field("root_dir", &self.root_dir)
            .finish_non_exhaustive()
    }
}

impl FileSystem for S3FileSystem {
    fn readlink(&self, path: &std::path::Path) -> FSResult<std::path::PathBuf> {
        todo!()
    }

    fn read_dir(&self, path: &std::path::Path) -> FSResult<virtual_fs::ReadDir> {
        todo!()
    }

    fn create_dir(&self, path: &std::path::Path) -> FSResult<()> {
        let handle = Handle::current();
        handle.block_on(async {
            let dir_obj = self.resolve_dir(path.parent().ok_or_else(|| FsError::InvalidInput)?)?;
            todo!()
        })
    }

    fn remove_dir(&self, path: &std::path::Path) -> FSResult<()> {
        todo!()
    }

    fn rename<'a>(
        &'a self,
        from: &'a std::path::Path,
        to: &'a std::path::Path,
    ) -> BoxFuture<'a, FSResult<()>> {
        todo!()
    }

    fn metadata(&self, path: &std::path::Path) -> FSResult<virtual_fs::Metadata> {
        todo!()
    }

    fn symlink_metadata(&self, path: &std::path::Path) -> FSResult<virtual_fs::Metadata> {
        todo!()
    }

    fn remove_file(&self, path: &std::path::Path) -> FSResult<()> {
        todo!()
    }

    fn new_open_options(&self) -> virtual_fs::OpenOptions<'_> {
        todo!()
    }

    fn mount(
        &self,
        name: String,
        path: &std::path::Path,
        fs: Box<dyn FileSystem + Send + Sync>,
    ) -> FSResult<()> {
        todo!()
    }
}
