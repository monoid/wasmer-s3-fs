use std::fmt;
use std::str::FromStr;
use std::{collections::HashMap, path::Path};

use serde::{Deserialize, Serialize};
use serde_with::{DeserializeFromStr, SerializeDisplay};
use virtual_fs::{DirEntry, FileType, Metadata, Result as FsResult};

use super::ROOT_OBJ_NAME;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct DirObj {
    pub(crate) children: HashMap<String, S3FsDirEntry>,
}

impl DirObj {
    pub fn deserialize(data: &[u8]) -> FsResult<DirObj> {
        serde_json::from_slice(data).map_err(|_| virtual_fs::FsError::InvalidData)
    }

    pub fn serialize(&self) -> FsResult<Vec<u8>> {
        serde_json::to_vec(self).map_err(|_| virtual_fs::FsError::InvalidData)
    }

    pub fn get<'a>(&'a self, component: &str) -> Option<&'a S3FsDirEntry> {
        self.children.get(component)
    }

    pub fn into_dir_entries(&self, parent: &Path) -> Vec<DirEntry> {
        self.children
            .iter()
            .map(|(name, ent)| DirEntry {
                path: parent.join(name),
                metadata: Ok(ent.into()),
            })
            .collect()
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct S3FsDirEntry {
    pub obj_name: ObjName,
    pub ctime: u64,
    pub len: u64,
}

impl From<&S3FsDirEntry> for Metadata {
    fn from(value: &S3FsDirEntry) -> Self {
        let ft = match value.obj_name {
            ObjName::File(_) => FileType::new_file(),
            ObjName::Dir(_) => FileType::new_dir(),
        };
        Metadata {
            ft,
            accessed: 0,
            created: value.ctime,
            modified: 0,
            len: value.len,
        }
    }
}

#[derive(Debug, Clone, SerializeDisplay, DeserializeFromStr, PartialEq, Eq)]
pub enum ObjName {
    File(String),
    Dir(String),
}

impl ObjName {
    pub fn root() -> Self {
        Self::from_str(ROOT_OBJ_NAME).unwrap()
    }

    pub fn gen_dir() -> Self {
        Self::Dir(uuid::Uuid::new_v4().to_string())
    }

    pub fn gen_file() -> Self {
        Self::File(uuid::Uuid::new_v4().to_string())
    }
}

impl fmt::Display for ObjName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ObjName::File(str) => write!(f, "f_{str}"),
            ObjName::Dir(str) => write!(f, "d_{str}"),
        }
    }
}

impl FromStr for ObjName {
    type Err = String;

    fn from_str(str: &str) -> Result<Self, Self::Err> {
        match str.split_once('_') {
            Some(("f", suffix)) => Ok(Self::File(suffix.to_owned())),
            Some(("d", suffix)) => Ok(Self::Dir(suffix.to_owned())),
            _ => Err(format!("Invalid object name: {str:?}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::filesystem::ROOT_OBJ_NAME;

    use super::*;

    #[test]
    fn test_obj_name_root() {
        let root_ref = ObjName::from_str(ROOT_OBJ_NAME).unwrap();
        assert_eq!(root_ref, ObjName::Dir("root".to_owned()));

        assert_eq!(root_ref.to_string(), ROOT_OBJ_NAME);
    }

    #[test]
    fn test_dir_deserialize_empty() {
        let dir_obj_content = br#"{"children":{}}"#;
        let dir_obj = DirObj::deserialize(dir_obj_content).unwrap();
        assert!(dir_obj.children.is_empty());
    }

    #[test]
    fn test_dir_deserialize_children() {
        let dir_obj_content = br#"{
            "children": {
                "home":{"obj_name":"d_134fds","ctime":420232,"len":0},
                "file":{"obj_name":"f_ashasdf","ctime":3843,"len":42}
            }
        }"#;
        let dir_obj = DirObj::deserialize(dir_obj_content).unwrap();
        assert_eq!(
            dir_obj.children,
            maplit::hashmap! {
                "home".to_owned() => S3FsDirEntry {
                    obj_name: ObjName::from_str("d_134fds").unwrap(),
                    ctime: 420232,
                    len: 0,
                },
                "file".to_owned() => S3FsDirEntry {
                    obj_name: ObjName::from_str("f_ashasdf").unwrap(),
                    ctime: 3843,
                    len: 42,
                },
            }
        );
    }
}
