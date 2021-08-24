use fuser::{
    FileAttr, FileType,
};
use std::io;
use std::fs;
use std::ffi::{OsStr, OsString};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use crate::jsonmetadata::JsonMetadata;

#[derive(Eq, Hash, Debug, Copy, Clone, PartialEq)]
pub enum EntryType {
    PDF,
    EPUB,
    RMLINES,
    NONE,
}


#[derive(Debug)]
pub struct DirEntry {
    pub root_path: PathBuf,
    pub prefix: OsString,
    pub entry_type: EntryType,
    pub name: OsString,
    pub parent: OsString,
    pub attr: FileAttr,

    json_metadata: JsonMetadata,
}

const ENTRYMAP: &'static [(EntryType, &'static str)] = &[
    (EntryType::EPUB, "epub"),
    (EntryType::PDF, "pdf"),
    (EntryType::RMLINES, "rm"),
];

pub fn entry_type_ext(e: &EntryType) -> &str {
    ENTRYMAP
        .iter()
        .find(|x| x.0 == *e)
        .unwrap_or(&(EntryType::NONE, ""))
        .1
}

fn determine_entry_type(path: &Path) -> (EntryType, u64) {
    let mut p = PathBuf::from(path);
    for (tp, ext) in ENTRYMAP {
        p.set_extension(ext);
        if p.exists() {
            let size = fs::File::open(p).unwrap().metadata().unwrap().len();
            return (*tp, size);
        }
    }
    return (EntryType::NONE, 0);
}

pub const DEFAULT_TTL: Duration = Duration::from_secs(1); // 1 second

pub const ROOT_DIR_ATTR: FileAttr = FileAttr {
    ino: 1,
    size: 0,
    blocks: 0,
    atime: UNIX_EPOCH, // 1970-01-01 00:00:00
    mtime: UNIX_EPOCH,
    ctime: UNIX_EPOCH,
    crtime: UNIX_EPOCH,
    kind: FileType::Directory,
    perm: 0o755,
    nlink: 2,
    uid: 501,
    gid: 20,
    rdev: 0,
    flags: 0,
    blksize: 512,
};


impl DirEntry {
    pub fn new(
        file_path: &Path,
        attr: &FileAttr,
        json_data: &JsonMetadata,
    ) -> DirEntry {
        let (tp, sz) = determine_entry_type(file_path);
        DirEntry {
            root_path: PathBuf::from(
                file_path.parent().unwrap_or(Path::new("")),
            ),
            prefix: file_path.file_stem().unwrap().to_os_string(),
            entry_type: tp,
            name: OsString::from(&json_data.visible_name),
            parent: OsString::from(&json_data.parent),
            attr: FileAttr {
                size: sz,
                kind: if tp == EntryType::NONE {
                    FileType::Directory
                } else {
                    FileType::RegularFile
                },
                perm: ROOT_DIR_ATTR.perm,
                ..*attr
            },
            json_metadata: json_data.clone(),
        }
    }

    pub fn make_root(dir_path: &Path) -> DirEntry {
        // TODO: make pathlike
        DirEntry {
            root_path: PathBuf::from(dir_path),
            prefix: OsString::from(""),
            entry_type: EntryType::NONE,
            name: OsString::from(""),
            parent: OsString::from(""),
            attr: ROOT_DIR_ATTR,

            json_metadata: JsonMetadata::new_file("", ""),
        }
    }

    pub fn make_dir(parent_dir: &DirEntry,
                    name: &OsStr,
                    mode: u32,
                    umask: u32) -> io::Result<DirEntry> {
        let uid = uuid::Uuid::new_v4();
        let mut dir = DirEntry {
            root_path: PathBuf::from(&parent_dir.root_path),
            prefix: OsString::from(uid.to_hyphenated().to_string()),
            entry_type: EntryType::NONE,
            name: OsString::from(name),
            parent: OsString::from(&parent_dir.prefix),
            attr: FileAttr {
                ino: 0, // need to replace with real ino after writing metadata
                perm: (mode & !umask) as u16,
                ..*&ROOT_DIR_ATTR
            },
            json_metadata: JsonMetadata::new_dir(name.to_str().unwrap(),
                                                 parent_dir.prefix.to_str().unwrap())
        };
        dir.json_metadata.save_file(dir.metadata_file_name())?;
        dir.attr.ino = fs::File::open(dir.metadata_file_name())?.metadata()?.ino();
        Ok(dir)
    }

    fn source_file_name(&self) -> OsString {
        let mut path = PathBuf::from(&self.prefix);
        path.set_extension(entry_type_ext(&self.entry_type));
        path.into_os_string()
    }

    pub fn file_name(&self) -> OsString {
        let mut path = PathBuf::from(&self.name);
        path.set_extension(entry_type_ext(&self.entry_type));
        path.into_os_string()
    }

    pub fn metadata_file_name(&self) -> PathBuf {
        let mut path = PathBuf::from(&self.root_path);
        path.push(&self.prefix);
        path.set_extension("metadata");
        path
    }

    pub fn is_parent(&self, parent: &DirEntry) -> bool {
        (parent.name == "." && self.parent == "")
            || self.parent == parent.prefix
    }

    pub fn parent_inode(&self) -> io::Result<u64> {
        let mut path = PathBuf::from(&self.root_path);
        path.push(&self.parent);
        path.set_extension("metadata");
        Ok(fs::File::open(path)?.metadata()?.ino())
    }

    pub fn rename(&self, newparent: &DirEntry, newname: &OsStr) -> io::Result<DirEntry> {
        let mut json_data = self.json_metadata.clone();
        json_data.visible_name = newname.to_string_lossy().to_string();
        json_data.parent = newparent.prefix.to_string_lossy().to_string();
        json_data.save_file(self.metadata_file_name())?;
        let res = DirEntry {
            name: OsString::from(newname),
            parent: newparent.prefix.clone(),
            json_metadata: json_data,
            root_path: self.root_path.clone(),
            prefix: self.prefix.clone(),
            ..*self
        };

        Ok(res)
    }
}

