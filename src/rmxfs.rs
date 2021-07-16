use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyData, ReplyDirectory,
    ReplyEmpty, ReplyEntry, ReplyOpen, Request,
};
use serde::{Deserialize, Serialize};
//use serde_json as json;
use libc::ENOENT;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::iter::FromIterator;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

#[allow(non_snake_case)]
#[derive(Serialize, Deserialize)]
struct JsonFileEntry {
    parent: String,
    visibleName: String,
}

#[derive(Debug, Copy, Clone, PartialEq)]
enum EntryType {
    PDF,
    NONE,
}

fn entry_type_ext(e: &EntryType) -> &str {
    match e {
        EntryType::PDF => "pdf",
        EntryType::NONE => "",
    }
}

fn determine_entry_type(path: &Path) -> (EntryType, u64) {
    let mut p = PathBuf::from(path);
    p.set_extension("pdf");
    if p.exists() {
        let size = fs::File::open(p).unwrap().metadata().unwrap().len();
        (EntryType::PDF, size)
    } else {
        (EntryType::NONE, 0)
    }
}

#[derive(Debug)]
struct DirEntry {
    root_path: PathBuf,
    prefix: OsString,
    //file_ext: OsString,
    entry_type: EntryType,
    name: OsString,
    parent: OsString,
    attr: FileAttr,
}

impl DirEntry {
    fn new(
        file_path: &Path,
        attr: &FileAttr,
        json_data: &JsonFileEntry,
    ) -> DirEntry {
        let (tp, sz) = determine_entry_type(file_path);
        DirEntry {
            root_path: PathBuf::from(
                file_path.parent().unwrap_or(Path::new("")),
            ),
            prefix: file_path.file_stem().unwrap().to_os_string(),
            entry_type: tp,
            name: OsString::from(&json_data.visibleName),
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
        }
    }

    fn make_root(dir_path: &Path) -> DirEntry {
        // TODO: make pathlike
        DirEntry {
            root_path: PathBuf::from(dir_path),
            prefix: OsString::from(""),
            entry_type: EntryType::NONE,
            name: OsString::from("."),
            parent: OsString::from(""),
            attr: HELLO_DIR_ATTR,
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

    fn is_parent(&self, parent: &DirEntry) -> bool {
        (parent.name == "." && self.parent == "")
            || self.parent == parent.prefix
    }

    fn parent_inode(&self) -> io::Result<u64> {
        let mut path = PathBuf::from(&self.root_path);
        path.push(&self.prefix);
        path.set_extension("metadata");
        Ok(fs::File::open(path)?.metadata()?.ino())
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
        let json_data: JsonFileEntry =
            serde_json::from_str(&fs::read_to_string(&path)?)?;
        res.push(DirEntry::new(&path, &conv_attr(&e)?, &json_data));
    }
    Ok(res)
}

impl RMXFS {
    fn find_file(&self, pred: &dyn Fn(&DirEntry) -> bool) -> Option<DirEntry> {
        match list_dir_metadata(&self.source_dir) {
            Ok(files) => files.into_iter().find(pred),
            Err(e) => {
                println!("Find file err: {}", e);
                None
            }
        }
    }
}

impl Filesystem for RMXFS {
    fn lookup(
        &mut self,
        _req: &Request,
        _parent: u64,
        name: &OsStr,
        reply: ReplyEntry,
    ) {
        match self.find_file(&|e: &DirEntry| name == e.file_name()) {
            Some(entry) => {
                &entry;
                reply.entry(&TTL, &entry.attr, 0)
            }
            None => {
                println!("lookup: not found {}", name.to_str().unwrap());
                reply.error(ENOENT)
            }
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        dbg!("getattr", ino);
        if ino == 1 {
            reply.attr(&TTL, &HELLO_DIR_ATTR)
        } else {
            match self.find_file(&|e: &DirEntry| ino == e.attr.ino) {
                Some(entry) => reply.attr(&TTL, &entry.attr),
                None => {
                    println!("getattr not found {}", ino);
                    reply.error(ENOENT)
                }
            }
        }
    }

    fn open(
        &mut self,
        _req: &Request,
        ino: u64,
        _flags: i32,
        reply: ReplyOpen,
    ) {
        println!("open: {}", ino);
        if let Some((counter, file)) = self.file_map.remove(&ino) {
            self.file_map.insert(ino, (counter + 1, file));
            reply.opened(ino, 0);
        } else {
            match self.find_file(&|e: &DirEntry| ino == e.attr.ino) {
                Some(entry) => {
                    let mut path = PathBuf::from(&self.source_dir);
                    path.push(entry.prefix);
                    path.set_extension(entry_type_ext(&entry.entry_type));
                    dbg!(&path); // TODO: cache open files
                    if let Ok(file) = fs::File::open(&path) {
                        self.file_map.insert(ino, (1, file));
                        reply.opened(ino, 0);
                    } else {
                        println!("open failed: {}", ino);
                        reply.error(libc::ENODATA);
                    }
                }
                None => {
                    println!("open: not found {}", ino);
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
                println!("release: {} ref {}", fh, counter);
                if counter > 1 {
                    self.file_map.insert(fh, (counter - 1, file));
                }
                reply.ok();
            }
            None => {
                println!("releasedir failed on: {}", fh);
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
            println!("read: not opened {}", fh);
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
        let parent = if ino == 1 {
            DirEntry::make_root(&self.source_dir)
        } else {
            match self.find_file(&|e| ino == e.attr.ino) {
                Some(entry) => entry,
                None => {
                    println!("opendir: not found: {}", ino);
                    reply.error(ENOENT);
                    return;
                }
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
                println!("releasedir: {} ref {}", fh, counter);
                if counter > 1 {
                    self.dir_map.insert(fh, (counter - 1, entries));
                }
                reply.ok();
            }
            None => {
                println!("releasedir failed on: {}", fh);
                reply.error(ENOENT);
            }
        }
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        if let Some((_, entries)) = self.dir_map.get(&fh) {
            dbg!(offset);
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
            println!("readdir: no handle {}", fh);
            reply.error(ENOENT);
        }
    }
}
