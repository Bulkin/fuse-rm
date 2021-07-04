use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyData,
    ReplyDirectory, ReplyEntry, Request,
};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH, SystemTime};
use libc::ENOENT;

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

const HELLO_TXT_CONTENT: &str = "Hello World!\n";

const HELLO_TXT_ATTR: FileAttr = FileAttr {
    ino: 2,
    size: 13,
    blocks: 1,
    atime: UNIX_EPOCH, // 1970-01-01 00:00:00
    mtime: UNIX_EPOCH,
    ctime: UNIX_EPOCH,
    crtime: UNIX_EPOCH,
    kind: FileType::RegularFile,
    perm: 0o644,
    nlink: 1,
    uid: 501,
    gid: 20,
    rdev: 0,
    flags: 0,
    blksize: 512,
};

pub struct RMXFS {
    source_dir: PathBuf,
}

impl RMXFS {
    pub fn new(source: &str) -> RMXFS {
        RMXFS {
            source_dir: PathBuf::from(source)
        }
    }
}

struct ShadowEntry (u64, FileType, OsString, FileAttr);

fn secs_to_systime(secs: i64) -> SystemTime {
    use std::convert::TryInto;
    let dur = Duration::from_secs(secs.abs().try_into().unwrap());
    if secs > 0 {
        UNIX_EPOCH + dur
    } else {
        UNIX_EPOCH - dur
    }
}

impl ShadowEntry {
    fn from_fs(e: &fs::DirEntry) -> io::Result<ShadowEntry> {
        let meta = e.metadata()?;
        Ok(ShadowEntry(meta.ino(),
                       if meta.is_dir() { FileType::Directory }
                       else { FileType::RegularFile },
                       e.file_name(),
                       FileAttr {
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
                       }))
    }
}

fn list_dir(dir: &PathBuf) -> io::Result<Vec<ShadowEntry>> {
    let mut res = Vec::new();
    for entry in fs::read_dir(dir)? {
        let e = ShadowEntry::from_fs(&entry?)?;
        res.push(e);
    }
    Ok(res)
}

impl RMXFS {
    fn find_file(&self, pred: &dyn Fn(&ShadowEntry) -> bool) -> Option<ShadowEntry> {
        match list_dir(&self.source_dir) {
            Ok(files) => files.into_iter().find(pred),
            Err(_e) => None
        }
    }
}

impl Filesystem for RMXFS {
    fn lookup(&mut self, _req: &Request, _parent: u64,
              name: &OsStr, reply: ReplyEntry) {
        match self.find_file(&|e: &ShadowEntry| name == e.2) {
            Some(entry) => reply.entry(&TTL, &entry.3, 0),
            None => reply.error(ENOENT)
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        if ino == 1 {
            reply.attr(&TTL, &HELLO_DIR_ATTR)
        } else {
            match {
                match list_dir(&self.source_dir) {
                    Ok(files) => files.into_iter().find(
                        &|e: &ShadowEntry| ino == e.3.ino),
                    Err(_e) => None
                }
            } {
                Some(entry) => reply.attr(&TTL, &entry.3),
                None        => reply.error(ENOENT)
            }
        }
    }

    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        _size: u32,
        _flags: i32,
        _lock: Option<u64>,
        reply: ReplyData,
    ) {
        match self.find_file(&|e: &ShadowEntry| ino == e.3.ino) {
            Some(entry) => {
                let mut path = PathBuf::from(&self.source_dir);
                path.push(entry.2);
                match fs::read(path) {
                    Ok(data) => reply.data(&data[offset as usize..]),
                    Err(_e) => reply.error(libc::ENODATA)
                }
            }
            None => reply.error(ENOENT)
        }
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        if ino != 1 {
            reply.error(ENOENT);
            return;
        }

        match list_dir(&self.source_dir) {
            Ok(entries) => {
                for (i, entry) in entries.into_iter().enumerate().skip(offset as usize) {
                    // i + 1 means the index of the next entry
                    if reply.add(entry.0, (i + 1) as i64, entry.1, entry.2) {
                        break;
                    }
                }
                reply.ok();
            },
            Err(_e) => { reply.error(ENOENT); }
        }
    }
}
