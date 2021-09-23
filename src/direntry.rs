use fuser::{FileAttr, FileType};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use crate::jsonmetadata::JsonMetadata;

#[derive(Eq, Hash, Debug, Copy, Clone, PartialEq)]
pub enum EntryType {
    PDF,
    EPUB,
    RMLINES,
    PENDING,
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

pub fn ext_entry_type(ext: &str) -> &EntryType {
    &ENTRYMAP
        .iter()
        .find(|x| x.1 == ext)
        .unwrap_or(&(EntryType::NONE, ""))
        .0
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

    pub fn make_trash(dir_path: &Path) -> DirEntry {
        DirEntry {
            root_path: PathBuf::from(dir_path),
            prefix: OsString::from("trash"),
            entry_type: EntryType::NONE,
            name: OsString::from("trash"),
            parent: OsString::from(""),
            attr: FileAttr {
                ino: 2,
                ..*&ROOT_DIR_ATTR
            },

            json_metadata: JsonMetadata::new_file("trash", ""),
        }
    }

    pub fn create_entry(
        parent_dir: &DirEntry,
        name: &OsStr,
        mode: u32,
        umask: u32,
        is_dir: bool,
    ) -> io::Result<DirEntry> {
        let uid = uuid::Uuid::new_v4();
        let mut entry = DirEntry {
            root_path: PathBuf::from(&parent_dir.root_path),
            prefix: OsString::from(uid.to_hyphenated().to_string()),
            entry_type: if is_dir {
                EntryType::NONE
            } else {
                EntryType::PENDING
            },
            name: OsString::from(name),
            parent: OsString::from(&parent_dir.prefix),
            attr: FileAttr {
                ino: 0, // need to replace with real ino after writing metadata
                perm: (mode & !umask) as u16,
                kind: if is_dir {
                    FileType::Directory
                } else {
                    FileType::RegularFile
                },
                ..*&ROOT_DIR_ATTR
            },
            json_metadata: if is_dir {
                JsonMetadata::new_dir(
                    name.to_str().unwrap(),
                    parent_dir.prefix.to_str().unwrap(),
                )
            } else {
                JsonMetadata::new_file(
                    name.to_str().unwrap(),
                    parent_dir.prefix.to_str().unwrap(),
                )
            },
        };
        let ino = if is_dir {
            entry.json_metadata.save_file(entry.metadata_file_name())?
        } else {
            // We rely on the inode not changing on mv
            let mut temp_file = PathBuf::from(&entry.root_path);
            temp_file.push(".pending");
            if !temp_file.exists() {
                fs::create_dir(&temp_file)?;
            }
            temp_file.push(&entry.prefix);
            temp_file.set_extension("metadata");
            entry.json_metadata.save_file(temp_file)?
        };
        entry.attr.ino = ino;
        Ok(entry)
    }

    pub fn make_dir(
        parent_dir: &DirEntry,
        name: &OsStr,
        mode: u32,
        umask: u32,
    ) -> io::Result<DirEntry> {
        DirEntry::create_entry(parent_dir, name, mode, umask, true)
    }

    pub fn make_file(
        parent_dir: &DirEntry,
        name: &OsStr,
        mode: u32,
        umask: u32,
    ) -> io::Result<DirEntry> {
        DirEntry::create_entry(parent_dir, name, mode, umask, false)
    }

    pub fn forget_pending(&self) {
        let data_file_path = self.source_file_path();
        if data_file_path.exists() {
            fs::remove_file(data_file_path).unwrap();
        }
        let metadata_path = self.metadata_file_name();
        if metadata_path.exists() {
            fs::remove_file(metadata_path).unwrap();
        }
    }

    pub fn finalize_pending(&self) -> io::Result<()> {
        if self.entry_type == EntryType::NONE
            || self.entry_type == EntryType::PENDING
        {
            return Err(io::Error::from_raw_os_error(libc::EPERM));
        }
        let mut source_path = PathBuf::from(&self.root_path);
        source_path.push(".pending");
        source_path.push(&self.prefix);
        fs::rename(&source_path, self.source_file_path())?;
        source_path.set_extension("metadata");
        fs::rename(&source_path, self.metadata_file_name())?;

        // The file type is stored in "*.content" (worked without it before)
        let mut content_path = self.metadata_file_name();
        content_path.set_extension("content");
        let content_data = json!({
            "fileType": entry_type_ext(&self.entry_type)
        });
        fs::write(content_path, serde_json::to_vec(&content_data)?)?;

        Ok(())
    }

    pub fn source_file_path(&self) -> PathBuf {
        let mut path = PathBuf::from(&self.root_path);
        if self.entry_type == EntryType::PENDING {
            path.push(".pending");
        }
        path.push(&self.prefix);
        path.set_extension(entry_type_ext(&self.entry_type));
        path
    }

    pub fn file_name(&self) -> OsString {
        let mut path = PathBuf::from(&self.name);
        path.set_extension(entry_type_ext(&self.entry_type));
        path.into_os_string()
    }

    pub fn metadata_file_name(&self) -> PathBuf {
        let mut path = PathBuf::from(&self.root_path);
        if self.entry_type == EntryType::PENDING {
            path.push(".pending");
        }
        path.push(&self.prefix);
        path.set_extension("metadata");
        path
    }

    pub fn is_parent(&self, parent: &DirEntry) -> bool {
        (parent.name == "." && self.parent == "")
            || self.parent == parent.prefix
    }

    pub fn parent_inode(&self) -> io::Result<u64> {
        if self.parent == "trash" {
            return Ok(2);
        }
        let mut path = PathBuf::from(&self.root_path);
        path.push(&self.parent);
        path.set_extension("metadata");
        Ok(fs::File::open(path)?.metadata()?.ino())
    }

    pub fn rename(
        &self,
        newparent: &DirEntry,
        newname: &OsStr,
    ) -> io::Result<DirEntry> {
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

    pub fn update_type(&mut self, buf: &[u8]) -> Result<(), &str> {
        match infer::get(buf) {
            Some(tp) => {
                if ext_entry_type(tp.extension()) != &EntryType::NONE {
                    Ok(self.entry_type = *ext_entry_type(tp.extension()))
                } else {
                    Err(tp.extension())
                }
            }
            None => Err("unknown"),
        }
    }
}
