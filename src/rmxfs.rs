use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyData, ReplyDirectory,
    ReplyEmpty, ReplyEntry, ReplyOpen, Request,
};
use libc::ENOENT;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::iter::FromIterator;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::jsonmetadata::JsonMetadata;

const TTL: Duration = Duration::from_secs(1); // 1 second

const HELLO_DIR_ATTR: FileAttr = FileAttr {
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

pub struct RMXFS {
    source_dir: PathBuf,
    dir_map: HashMap<u64, (u32, Vec<DirEntry>)>, // refcounter because
    file_map: HashMap<u64, (u32, fs::File)>,     // releases may be interleaved
}

impl RMXFS {
    pub fn new(source: &str) -> RMXFS {
        RMXFS {
            source_dir: PathBuf::from(source),
            dir_map: HashMap::new(),
            file_map: HashMap::new(),
        }
    }
}

#[derive(Eq, Hash, Debug, Copy, Clone, PartialEq)]
enum EntryType {
    PDF,
    EPUB,
    RMLINES,
    NONE,
}

const ENTRYMAP: &'static [(EntryType, &'static str)] = &[
    (EntryType::EPUB, "epub"),
    (EntryType::PDF, "pdf"),
    (EntryType::RMLINES, "rm"),
];

fn entry_type_ext(e: &EntryType) -> &str {
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

#[derive(Debug)]
struct DirEntry {
    root_path: PathBuf,
    prefix: OsString,
    entry_type: EntryType,
    name: OsString,
    parent: OsString,
    attr: FileAttr,

    json_metadata: JsonMetadata,
}

impl DirEntry {
    fn new(
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
                perm: HELLO_DIR_ATTR.perm,
                ..*attr
            },
            json_metadata: json_data.clone(),
        }
    }

    fn make_root(dir_path: &Path) -> DirEntry {
        // TODO: make pathlike
        DirEntry {
            root_path: PathBuf::from(dir_path),
            prefix: OsString::from(""),
            entry_type: EntryType::NONE,
            name: OsString::from(""),
            parent: OsString::from(""),
            attr: HELLO_DIR_ATTR,

            json_metadata: JsonMetadata::new("", ""),
        }
    }

    fn source_file_name(&self) -> OsString {
        let mut path = PathBuf::from(&self.prefix);
        path.set_extension(entry_type_ext(&self.entry_type));
        path.into_os_string()
    }

    fn file_name(&self) -> OsString {
        let mut path = PathBuf::from(&self.name);
        path.set_extension(entry_type_ext(&self.entry_type));
        path.into_os_string()
    }

    fn metadata_file_name(&self) -> PathBuf {
        let mut path = PathBuf::from(&self.root_path);
        path.push(&self.prefix);
        path.set_extension("metadata");
        path
    }

    fn is_parent(&self, parent: &DirEntry) -> bool {
        (parent.name == "." && self.parent == "")
            || self.parent == parent.prefix
    }

    fn parent_inode(&self) -> io::Result<u64> {
        let mut path = PathBuf::from(&self.root_path);
        path.push(&self.parent);
        path.set_extension("metadata");
        Ok(fs::File::open(path)?.metadata()?.ino())
    }

    fn rename(&self, newparent: &DirEntry, newname: &OsStr) -> io::Result<DirEntry> {
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

fn secs_to_systime(secs: i64) -> SystemTime {
    use std::convert::TryInto;
    let dur = Duration::from_secs(secs.abs().try_into().unwrap());
    if secs > 0 {
        UNIX_EPOCH + dur
    } else {
        UNIX_EPOCH - dur
    }
}

fn conv_attr(attr: &fs::DirEntry) -> io::Result<FileAttr> {
    let meta = attr.metadata()?;
    Ok(FileAttr {
        ino: meta.ino(),
        size: meta.size(),
        blocks: meta.blocks(),
        atime: secs_to_systime(meta.atime()),
        mtime: secs_to_systime(meta.mtime()),
        ctime: secs_to_systime(meta.ctime()),
        crtime: meta.created()?,
        kind: if meta.is_dir() {
            FileType::Directory
        } else {
            FileType::RegularFile
        },
        perm: meta.mode() as u16,
        nlink: 1,
        uid: meta.uid(),
        gid: meta.gid(),
        rdev: meta.rdev() as u32,
        flags: 0,
        blksize: meta.blksize() as u32,
    })
}

fn list_dir_metadata(dir: &PathBuf) -> io::Result<Vec<DirEntry>> {
    let mut res = Vec::new();
    for entry in fs::read_dir(dir)? {
        let e = entry?;
        if !e.file_name().to_str().unwrap_or("").ends_with(".metadata") {
            continue;
        }
        let mut path = PathBuf::from(dir);
        path.push(e.file_name());
        let json_data = JsonMetadata::from_file(&path)?;
        res.push(DirEntry::new(&path, &conv_attr(&e)?, &json_data));
    }
    Ok(res)
}

impl RMXFS {
    fn find_file(&self, pred: &dyn Fn(&DirEntry) -> bool) -> Option<DirEntry> {
        match list_dir_metadata(&self.source_dir) {
            Ok(files) => files.into_iter().find(pred),
            Err(e) => {
                debug!("Find file err: {}", e);
                None
            }
        }
    }

    fn dir_from_ino(&self, ino: u64) -> Option<DirEntry> {
        if ino == 1 {
            Some(DirEntry::make_root(&self.source_dir))
        } else {
            self.find_file(&|e: &DirEntry| e.attr.ino == ino)
        }
    }
}

impl Filesystem for RMXFS {
    fn lookup(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        reply: ReplyEntry,
    ) {
        debug!("lookup: {}", name.to_str().unwrap());
        match self.find_file(&|e: &DirEntry| name == e.file_name() &&
                             parent == e.parent_inode().unwrap_or(1)) {
            Some(entry) => {
                &entry;
                reply.entry(&TTL, &entry.attr, 0)
            }
            None => {
                debug!("lookup: not found {}", name.to_str().unwrap());
                reply.error(ENOENT)
            }
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        if ino == 1 {
            reply.attr(&TTL, &HELLO_DIR_ATTR)
        } else {
            match self.find_file(&|e: &DirEntry| ino == e.attr.ino) {
                Some(entry) => reply.attr(&TTL, &entry.attr),
                None => {
                    debug!("getattr not found {}", ino);
                    reply.error(ENOENT)
                }
            }
        }
    }

    fn rename(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        _flags: u32,
        reply: ReplyEmpty,
    ) {
        debug!("rename: {}/{} -> {}/{}",
               parent, name.to_str().unwrap(),
               newparent, newname.to_str().unwrap());
        // need to get the ino of the parent -- easily done by opening e.parent + '.metadata'.
        if let Some(entry) = self.find_file(
            &|e: &DirEntry| (e.parent_inode().unwrap_or(1) == parent &&
                             e.file_name() == name)) {

            if let Some(parent_entry) = self.dir_from_ino(newparent) {
                if let Err(_) = entry.rename(&parent_entry, newname) {
                    reply.error(libc::EIO);
                    return;
                }
                reply.ok();
                return;
            } else {
                debug!("rename: newparent not found: {}", newparent);
            }
        }
        debug!("rename: not found {}/{}", parent, name.to_str().unwrap());
        reply.error(ENOENT);
    }

    fn open(
        &mut self,
        _req: &Request,
        ino: u64,
        _flags: i32,
        reply: ReplyOpen,
    ) {
        debug!("open: {}", ino);
        if let Some((counter, file)) = self.file_map.remove(&ino) {
            self.file_map.insert(ino, (counter + 1, file));
            reply.opened(ino, 0);
        } else {
            match self.find_file(&|e: &DirEntry| ino == e.attr.ino) {
                Some(entry) => {
                    let mut path = PathBuf::from(&self.source_dir);
                    path.push(entry.prefix);
                    path.set_extension(entry_type_ext(&entry.entry_type));
                    if let Ok(file) = fs::File::open(&path) {
                        self.file_map.insert(ino, (1, file));
                        reply.opened(ino, 0);
                    } else {
                        debug!("open failed: {}", ino);
                        reply.error(libc::ENODATA);
                    }
                }
                None => {
                    debug!("open: not found {}", ino);
                    reply.error(ENOENT);
                }
            }
        }
    }

    fn release(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        match self.file_map.remove(&fh) {
            Some((counter, file)) => {
                debug!("release: {} ref {}", fh, counter);
                if counter > 1 {
                    self.file_map.insert(fh, (counter - 1, file));
                }
                reply.ok();
            }
            None => {
                debug!("releasedir failed on: {}", fh);
                reply.error(ENOENT);
            }
        }
    }

    fn read(
        &mut self,
        _req: &Request,
        _ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock: Option<u64>,
        reply: ReplyData,
    ) {
        if let Some((_, file)) = self.file_map.get(&fh) {
            use std::cmp::min;
            use std::os::unix::fs::FileExt;
            let file_size = file.metadata().unwrap().len();
            let read_size =
                min(size, file_size.saturating_sub(offset as u64) as u32);
            let mut buffer = vec![0; read_size as usize];
            file.read_exact_at(&mut buffer, offset as u64).unwrap();
            reply.data(&buffer);
        } else {
            debug!("read: not opened {}", fh);
            reply.error(ENOENT)
        }
    }

    fn opendir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _flags: i32,
        reply: ReplyOpen,
    ) {
        debug!("opendir: {}", ino);
        let parent = match self.dir_from_ino(ino) {
            Some(entry) => entry,
            None => {
                debug!("opendir: not found: {}", ino);
                reply.error(ENOENT);
                return;
            }
        };

        if let Some((counter, entries)) = self.dir_map.remove(&ino) {
            self.dir_map.insert(ino, (counter + 1, entries));
            reply.opened(ino, 0);
        } else {
            match list_dir_metadata(&self.source_dir) {
                Ok(entries) => {
                    self.dir_map.insert(
                        ino,
                        (
                            1,
                            Vec::from_iter(
                                entries
                                    .into_iter()
                                    .filter(|e| e.is_parent(&parent)),
                            ),
                        ),
                    );
                    reply.opened(ino, 0);
                }
                Err(_e) => {
                    reply.error(ENOENT);
                }
            }
        }
    }

    fn releasedir(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        _flags: i32,
        reply: ReplyEmpty,
    ) {
        match self.dir_map.remove(&fh) {
            Some((counter, entries)) => {
                debug!("releasedir: {} ref {}", fh, counter);
                if counter > 1 {
                    self.dir_map.insert(fh, (counter - 1, entries));
                }
                reply.ok();
            }
            None => {
                debug!("releasedir failed on: {}", fh);
                reply.error(ENOENT);
            }
        }
    }

    fn readdir(
        &mut self,
        _req: &Request,
        _ino: u64,
        fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        debug!("readdir: {}", fh);
        if let Some((_, entries)) = self.dir_map.get(&fh) {
            for (i, entry) in
                entries.into_iter().enumerate().skip(offset as usize)
            {
                if reply.add(
                    entry.attr.ino,
                    (i + 1) as i64,
                    if entry.entry_type == EntryType::PDF {
                        FileType::RegularFile
                    } else {
                        FileType::Directory
                    },
                    entry.file_name(),
                ) {
                    break;
                }
            }
            reply.ok();
        } else {
            debug!("readdir: no handle {}", fh);
            reply.error(ENOENT);
        }
    }
}
