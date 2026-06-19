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
        Ok(virtual_fs::ReadDir::new(dir_obj.into_dir_entries(path)))
    }

    fn create_dir(&self, path: &std::path::Path) -> FSResult<()> {
        use std::collections::hash_map::Entry;

        let parent = path.parent().ok_or_else(|| FsError::InvalidInput)?;
        let parent_ref = self.resolve_dir_ref(parent)?;

        let file_name = path.file_name().unwrap().to_string_lossy().to_string();

        self.update_dir(&parent_ref, |mut dir| {
            match dir.children.entry(file_name.clone()) {
                Entry::Occupied(_occupied_entry) => return Err(FsError::AlreadyExists),
                Entry::Vacant(vacant_entry) => {
                    let dir_obj_name = ObjName::gen_dir();
                    self.put_dir(&dir_obj_name, &DirObj::default())?;

                    let since_the_epoch = timestamp();

                    vacant_entry.insert(S3FsDirEntry {
                        obj_name: dir_obj_name,
                        ctime: since_the_epoch,
                        len: 0,
                    });
                }
            };
            Ok(dir)
        })?;
        Ok(())
    }

    fn remove_dir(&self, path: &std::path::Path) -> FSResult<()> {
        use std::collections::hash_map::Entry;

        let parent = path.parent().ok_or_else(|| FsError::InvalidInput)?;
        let parent_ref = self.resolve_dir_ref(parent)?;

        let file_name = path.file_name().unwrap().to_string_lossy().to_string();

        self.update_dir(&parent_ref, |mut dir| {
            match dir.children.entry(file_name.clone()) {
                Entry::Occupied(occupied_entry) => {
                    let dir_obj_name = &occupied_entry.get().obj_name;

                    let dir_obj = self.load_dir(dir_obj_name)?;
                    if !dir_obj.children.is_empty() {
                        return Err(FsError::DirectoryNotEmpty);
                    }
                    self.store.delete(&dir_obj_name.to_string())?;
                    occupied_entry.remove();
                }
                Entry::Vacant(_vacant_entry) => {
                    return Err(FsError::EntryNotFound);
                }
            }
            Ok(dir)
        })?;
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
            .get(&file_name)
            .ok_or(FsError::EntryNotFound)?;

        Ok(ent.into())
    }

    fn symlink_metadata(&self, path: &std::path::Path) -> FSResult<virtual_fs::Metadata> {
        self.metadata(path)
    }

    fn remove_file(&self, path: &std::path::Path) -> FSResult<()> {
        use std::collections::hash_map::Entry;

        let parent = path.parent().ok_or_else(|| FsError::InvalidInput)?;
        let parent_ref = self.resolve_dir_ref(parent)?;

        let file_name = path.file_name().unwrap().to_string_lossy().to_string();

        self.update_dir(&parent_ref, |mut dir| {
            match dir.children.entry(file_name.clone()) {
                Entry::Occupied(occupied_entry) => {
                    let dir_obj_name = &occupied_entry.get().obj_name;

                    match dir_obj_name {
                        ObjName::File(_) => {
                            self.store.delete(&dir_obj_name.to_string())?;
                            occupied_entry.remove();
                        }
                        ObjName::Dir(_) => {
                            return Err(FsError::InvalidInput);
                        }
                    }
                }
                Entry::Vacant(_vacant_entry) => {
                    return Err(FsError::EntryNotFound);
                }
            }
            Ok(dir)
        })?;
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

