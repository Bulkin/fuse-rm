use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyCreate, ReplyData,
    ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyWrite, Request,
};
use io::{Seek, Write};
use libc::ENOENT;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::iter::FromIterator;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::direntry::{
    entry_type_ext, DirEntry, EntryType, DEFAULT_TTL, ROOT_DIR_ATTR,
};
use crate::jsonmetadata::JsonMetadata;

pub struct RMXFS {
    source_dir: PathBuf,
    dir_map: HashMap<u64, (u32, Vec<DirEntry>)>, // refcounter because
    file_map: HashMap<u64, (u32, fs::File)>,     // releases may be interleaved
    // map for files being created
    // when closed, the must be moved from ".pending" to the root
    pending_map: HashMap<u64, (DirEntry, fs::File)>,
}

impl RMXFS {
    pub fn new(source: &str) -> RMXFS {
        RMXFS {
            source_dir: PathBuf::from(source),
            dir_map: HashMap::new(),
            file_map: HashMap::new(),
            pending_map: HashMap::new(),
        }
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

    // Special dirs (currently trash:2):
    res.push(DirEntry::make_trash(dir));

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
        debug!("lookup: {}/{}", parent, name.to_str().unwrap());
        match self.find_file(&|e: &DirEntry| {
            name == e.file_name() && parent == e.parent_inode().unwrap_or(1)
        }) {
            Some(entry) => {
                &entry;
                reply.entry(&DEFAULT_TTL, &entry.attr, 0)
            }
            None => {
                debug!("lookup: not found {}", name.to_str().unwrap());
                reply.error(ENOENT)
            }
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        if let Some((entry, _)) = self.pending_map.get(&ino) {
            reply.attr(&DEFAULT_TTL, &entry.attr);
            return;
        }
        if let Some(entry) = self.dir_from_ino(ino) {
            reply.attr(&DEFAULT_TTL, &entry.attr);
        } else {
            debug!("getattr not found {}", ino);
            reply.error(ENOENT)
        }
    }

    fn create(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        debug!("create: {}/{}", parent, name.to_str().unwrap());
        if let Some(parent_dir) = self.dir_from_ino(parent) {
            if self
                .pending_map
                .iter()
                .find(&|(_, (e, _)): &(&u64, &(DirEntry, fs::File))| {
                    e.name == name && e.parent_inode().unwrap_or(1) == parent
                })
                .is_some()
            {
                debug!("create: file already pending {}/{:?}", parent, name);
                reply.error(libc::EEXIST);
                return;
            }
            if self
                .find_file(&|e: &DirEntry| {
                    name == e.file_name()
                        && parent == e.parent_inode().unwrap_or(1)
                })
                .is_some()
            {
                debug!("create: file exists {}/{:?}", parent, name);
                reply.error(libc::EEXIST);
                return;
            }
            if flags & libc::O_ACCMODE != libc::O_WRONLY {
                debug!("create: wrong file mode");
                reply.error(libc::EINVAL);
                return;
            }
            match DirEntry::make_file(&parent_dir, name, mode, umask) {
                Ok(entry) => {
                    let path = entry.source_file_path();
                    if let Ok(file) = fs::File::create(&path) {
                        let ino = entry.attr.ino;
                        let attr = entry.attr;
                        self.pending_map.insert(ino, (entry, file));
                        reply.created(&Duration::new(0, 0), &attr, 0, ino, 0);
                    } else {
                        debug!("create: failed to create file at {:?}", &path);
                        reply.error(libc::EIO);
                    }
                }
                Err(e) => {
                    debug!("create: failed making file: {}", e);
                    reply.error(libc::EIO);
                }
            }
        }
    }

    fn mkdir(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        mode: u32,
        umask: u32,
        reply: ReplyEntry,
    ) {
        debug!("mkdir: {}/{}", parent, name.to_str().unwrap());
        // make KOReader's side cars go away
        if name
            .to_string_lossy()
            .as_bytes()
            .iter()
            .find(|x| **x == '.' as u8)
            .is_some()
        {
            reply.error(libc::ENOSYS);
            return;
        }
        if let Some(parent_dir) = self.dir_from_ino(parent) {
            match DirEntry::make_dir(&parent_dir, name, mode, umask) {
                Ok(dir) => reply.entry(&DEFAULT_TTL, &dir.attr, 0),
                Err(e) => {
                    debug!("mkdir: {}", e);
                    reply.error(libc::EIO);
                }
            }
        } else {
            debug!("mkdir: parent not found {}", parent);
            reply.error(ENOENT);
        }
    }

    fn rmdir(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        reply: ReplyEmpty,
    ) {
        debug!("rmdir: {}/{}", parent, name.to_str().unwrap());
        if let Some(parent_dir) = self.dir_from_ino(parent) {
            if let Some(dir) = self.find_file(&|e: &DirEntry| {
                e.parent == parent_dir.prefix && name == e.name
            }) {
                // Removing the directory is ok, since open dirs hang around
                // in the dir_map
                /* if self.dir_map.contains_key(&dir.attr.ino) {
                    reply.error(libc::EBUSY);
                } else */
                if self
                    .find_file(&|e: &DirEntry| e.parent == dir.prefix)
                    .is_some()
                {
                    reply.error(libc::ENOTEMPTY);
                } else {
                    match fs::remove_file(dir.metadata_file_name()) {
                        Ok(_) => reply.ok(),
                        Err(e) => {
                            debug!("rmdir: couldn't remove metadata: {}", e);
                            reply.error(libc::EIO);
                        }
                    }
                }
            } else {
                debug!("mkdir: dir not found {}", name.to_str().unwrap());
                reply.error(ENOENT);
            }
        } else {
            debug!("mkdir: parent not found: {}", parent);
            reply.error(ENOENT);
        }
    }

    fn unlink(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        reply: ReplyEmpty,
    ) {
        debug!("unlink: {}/{:?}", parent, name);
        if let Some(parent_dir) = self.dir_from_ino(parent) {
            debug!(
                "unlink: parent dir {}/{:?}",
                parent_dir.attr.ino, parent_dir.name
            );
            if let Some(entry) = self.find_file(&|e: &DirEntry| {
                parent_dir.prefix == e.parent && name == e.file_name()
            }) {
                if self.file_map.contains_key(&entry.attr.ino) {
                    debug!("unlink: file is being read: {}/{:?}", parent, name);
                    reply.error(libc::EBUSY);
                    return;
                }
                if let Err(e) = fs::remove_file(entry.source_file_path()) {
                    debug!(
                        "unlink: couldn't remove file {:?}: {}",
                        entry.source_file_path(),
                        e
                    );
                    reply.error(libc::EIO);
                    return;
                }
                let mut metadata_path = entry.metadata_file_name();
                debug!("unlink: removing {:?}", metadata_path);
                match fs::remove_file(&metadata_path) {
                    Ok(_) => reply.ok(),
                    Err(e) => {
                        debug!("unlink: couldn't remove metadata: {}", e);
                        reply.error(libc::EIO);
                    }
                }
                // TODO: have proper file list for direntry
                metadata_path.set_extension("content");
                fs::remove_file(metadata_path).unwrap_or(());
            } else {
                debug!("unlink: file not found {}/{:?}", parent, name);
                reply.error(ENOENT);
            }
        } else {
            debug!("unlink: parent not found: {}", parent);
            reply.error(ENOENT);
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
        debug!(
            "rename: {}/{} -> {}/{}",
            parent,
            name.to_str().unwrap(),
            newparent,
            newname.to_str().unwrap()
        );
        if let Some(entry) = self.find_file(&|e: &DirEntry| {
            e.parent_inode().unwrap_or(1) == parent && e.file_name() == name
        }) {
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
        debug!("release: {}", fh);
        if let Some((entry, _)) = self.pending_map.remove(&fh) {
            if let Err(e) = entry.finalize_pending() {
                debug!("release: couldn't finalize pending file {}", e);
                reply.error(libc::EIO);
                return;
            } else {
                reply.ok();
                return;
            }
        }
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
            if let Err(e) = file.read_exact_at(&mut buffer, offset as u64) {
                debug!("read: error {}", e);
                reply.error(libc::EIO);
            } else {
                reply.data(&buffer);
            }
        } else {
            debug!("read: not opened {}", fh);
            reply.error(ENOENT)
        }
    }

    fn write(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        debug!("write: {} {}", fh, offset);
        if let Some((entry, file)) = self.pending_map.get_mut(&fh) {
            // Check file compatibility and abort early
            if offset == 0 {
                if let Err(ext) = entry.update_type(data) {
                    debug!("write: unsupported type: {}", ext);
                    entry.forget_pending();
                    reply.error(libc::ENOSYS);
                    return;
                }
            }
            if let Err(e) = file.seek(io::SeekFrom::Start(offset as u64)) {
                debug!("write: seek failed: {}", e);
                reply.error(libc::EIO);
                return;
            }

            if let Err(e) = file.write_all(data) {
                debug!("write: write failed: {}", e);
                reply.error(libc::EIO);
                return;
            }

            reply.written(data.len() as u32);
        } else {
            debug!("write: no pending file open: {}", fh);
            reply.error(ENOENT);
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
