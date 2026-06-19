use futures_core::future::BoxFuture;
use virtual_fs::{
    FileOpener, FileSystem, FsError, OpenOptions, OpenOptionsConfig, Result as FSResult,
    VirtualFile,
};

use crate::filesystem::{timestamp, tree::{DirObj, ObjName, S3FsDirEntry}};

use super::S3FileSystem;

impl FileSystem for S3FileSystem {
    fn readlink(&self, path: &std::path::Path) -> FSResult<std::path::PathBuf> {
        Ok(path.to_path_buf())
    }

    fn read_dir(&self, path: &std::path::Path) -> FSResult<virtual_fs::ReadDir> {
        let dir_obj = self.resolve_dir(path)?;
        Ok(virtual_fs::ReadDir::new(dir_obj.as_ref().into_dir_entries(path)))
    }

    fn create_dir(&self, path: &std::path::Path) -> FSResult<()> {
        let parent = path.parent().ok_or_else(|| FsError::InvalidInput)?;
        let parent_ref = self.resolve_dir_ref(parent)?;

        let file_name = path.file_name().unwrap().to_string_lossy().to_string();

        // Create the child directory object first (data-before-pointer). If the
        // name turns out to be taken, this object is simply left unreferenced
        // for the garbage collector.
        let dir_obj_name = ObjName::gen_dir();
        self.put_dir_create(&dir_obj_name, &DirObj::default())?;
        let ctime = timestamp();

        // The closure must stay pure: it can be re-run on a CAS conflict.
        self.update_dir(&parent_ref, |old_dir| {
            if old_dir.children.contains_key(&file_name) {
                return Err(FsError::AlreadyExists);
            }
            let mut dir = old_dir.clone();
            dir.children.insert(
                file_name.clone(),
                S3FsDirEntry {
                    obj_name: dir_obj_name.clone(),
                    ctime,
                    len: 0,
                },
            );
            Ok((dir, ()))
        })?;
        Ok(())
    }

    fn remove_dir(&self, path: &std::path::Path) -> FSResult<()> {
        let parent = path.parent().ok_or_else(|| FsError::InvalidInput)?;
        let parent_ref = self.resolve_dir_ref(parent)?;

        let file_name = path.file_name().unwrap().to_string_lossy().to_string();

        // Locate the child directory object.
        let child = self
            .load_dir(&parent_ref)?
            .as_ref()
            .get(&file_name)
            .ok_or(FsError::EntryNotFound)?
            .obj_name
            .clone();
        if !matches!(child, ObjName::Dir(_)) {
            return Err(FsError::InvalidInput);
        }

        // Phase 1: tombstone the child *on its own object*. This CAS contends
        // with any concurrent insert into the child, closing the
        // check-empty-then-unlink race: either we mark it deleted first (and a
        // racing insert then reloads and is refused), or the insert lands first
        // (and we reload, see it non-empty, and refuse with DirectoryNotEmpty).
        self.update_dir(&child, |old_dir| {
            if !old_dir.children.is_empty() {
                return Err(FsError::DirectoryNotEmpty);
            }
            let mut dir = old_dir.clone();
            dir.deleted = true;
            Ok((dir, ()))
        })?;

        // Phase 2: unlink from the parent (now safe — the child is sealed).
        self.update_dir(&parent_ref, |old_dir| {
            let mut dir = old_dir.clone();
            dir.children.remove(&file_name);
            Ok((dir, ()))
        })?;

        // Phase 3: physically remove the tombstoned object. A crash before this
        // point leaves a tombstoned/orphaned object for the garbage collector.
        self.store.delete(&child.to_string())?;
        Ok(())
    }

    fn rename<'a>(
        &'a self,
        from: &'a std::path::Path,
        to: &'a std::path::Path,
    ) -> BoxFuture<'a, FSResult<()>> {
        todo!()
    }

    fn metadata(&self, path: &std::path::Path) -> FSResult<virtual_fs::Metadata> {
        let parent = path.parent().unwrap();
        let parent_dir_obj = self.resolve_dir(parent)?;

        let file_name = path.file_name().unwrap().to_string_lossy().to_string();
        let ent = parent_dir_obj
            .as_ref()
            .get(&file_name)
            .ok_or(FsError::EntryNotFound)?;

        Ok(ent.into())
    }

    fn symlink_metadata(&self, path: &std::path::Path) -> FSResult<virtual_fs::Metadata> {
        self.metadata(path)
    }

    fn remove_file(&self, path: &std::path::Path) -> FSResult<()> {
        let parent = path.parent().ok_or_else(|| FsError::InvalidInput)?;
        let parent_ref = self.resolve_dir_ref(parent)?;

        let file_name = path.file_name().unwrap().to_string_lossy().to_string();

        // Same shape as `remove_dir`: CAS-unlink first, then delete the object.
        let child = self.update_dir(&parent_ref, |old_dir| {
            let ent = old_dir
                .children
                .get(&file_name)
                .ok_or(FsError::EntryNotFound)?;
            if !matches!(ent.obj_name, ObjName::File(_)) {
                return Err(FsError::InvalidInput);
            }
            let child = ent.obj_name.clone();

            let mut dir = old_dir.clone();
            dir.children.remove(&file_name);
            Ok((dir, child))
        })?;

        self.store.delete(&child.to_string())?;
        Ok(())
    }

    fn new_open_options(&self) -> virtual_fs::OpenOptions<'_> {
        OpenOptions::new(self)
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

impl FileOpener for S3FileSystem {
    fn open(
        &self,
        path: &std::path::Path,
        conf: &OpenOptionsConfig,
    ) -> FSResult<Box<dyn VirtualFile + Send + Sync + 'static>> {
        Ok(Box::new(self.open_file(path, conf)?))
    }
}

