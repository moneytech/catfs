extern crate fuse;
extern crate libc;
extern crate xattr;

use std::ffi::{CStr, CString, OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::io;
use self::fuse::FileType;
use std::path::Path;
use std::os::unix::io::AsRawFd;
use std::os::unix::io::RawFd;
use std::os::unix::fs::FileExt;

use self::xattr::FileExt as XattrFileExt;
use catfs::error::RError;

// libc defines these as i32 which means they can't naturally be OR'ed
// with u32
pub static O_ACCMODE: u32 = libc::O_ACCMODE as u32;
pub static O_RDONLY: u32 = libc::O_RDONLY as u32;
pub static O_WRONLY: u32 = libc::O_WRONLY as u32;
pub static O_RDWR: u32 = libc::O_RDWR as u32;

pub static O_CLOEXEC: u32 = libc::O_CLOEXEC as u32;
pub static O_CREAT: u32 = libc::O_CREAT as u32;
pub static O_EXCL: u32 = libc::O_EXCL as u32;
pub static O_TRUNC: u32 = libc::O_TRUNC as u32;

pub fn to_cstring(path: &AsRef<Path>) -> CString {
    let bytes = path.as_ref().as_os_str().to_os_string().into_vec();
    return CString::new(bytes).unwrap();
}

pub fn opendir(path: &AsRef<Path>) -> io::Result<*mut libc::DIR> {
    let s = to_cstring(path);
    let dh = unsafe { libc::opendir(s.as_ptr()) };
    if dh.is_null() {
        return Err(io::Error::last_os_error());
    } else {
        return Ok(dh);
    }
}

pub fn closedir(dir: *mut libc::DIR) -> io::Result<()> {
    let err: libc::c_int;
    unsafe { err = libc::closedir(dir) }
    match err {
        0 => return Ok(()),
        _ => return Err(io::Error::last_os_error()),
    }
}

pub fn seekdir(dir: *mut libc::DIR, loc: u64) {
    unsafe {
        libc::seekdir(dir, loc as i64);
    }
}

#[derive(Clone)]
pub struct Dirent {
    en: libc::dirent,
}

impl Default for Dirent {
    fn default() -> Dirent {
        return Dirent {
            en: libc::dirent {
                d_ino: 0,
                d_off: 0,
                d_reclen: 0,
                d_type: libc::DT_REG,
                d_name: [0i8; 256], // FIXME: don't hardcode 256
            },
        };
    }
}

fn array_to_osstring(cslice: &[libc::c_char]) -> OsString {
    let s = unsafe { CStr::from_ptr(cslice.as_ptr()) };
    return OsStr::from_bytes(s.to_bytes()).to_os_string();
}


impl Dirent {
    pub fn ino(&self) -> u64 {
        return self.en.d_ino;
    }
    pub fn off(&self) -> u64 {
        return self.en.d_off as u64;
    }
    pub fn kind(&self) -> fuse::FileType {
        match self.en.d_type {
            libc::DT_BLK => return FileType::BlockDevice,
            libc::DT_CHR => return FileType::CharDevice,
            libc::DT_DIR => return FileType::Directory,
            libc::DT_FIFO => return FileType::NamedPipe,
            libc::DT_LNK => return FileType::Symlink,
            _ => return FileType::RegularFile,
        }
    }
    pub fn name(&self) -> OsString {
        return array_to_osstring(&self.en.d_name);
    }
}

pub fn readdir(dir: *mut libc::DIR) -> io::Result<Option<Dirent>> {
    let mut entry: Dirent = Default::default();
    let mut entry_p: *mut libc::dirent = &mut entry.en;
    let entry_pp: *mut *mut libc::dirent = &mut entry_p;

    unsafe {
        let err = libc::readdir_r(dir, entry_p, entry_pp);
        if err == 0 {
            if (*entry_pp).is_null() {
                return Ok(None);
            } else {
                return Ok(Some(entry));
            }
        } else {
            return Err(io::Error::last_os_error());
        }
    }
}

pub fn mkdir(path: &AsRef<Path>, mode: u32) -> io::Result<()> {
    let s = to_cstring(path);
    let res = unsafe { libc::mkdir(s.as_ptr(), mode) };
    if res < 0 {
        return Err(io::Error::last_os_error());
    } else {
        return Ok(());
    }
}

pub struct File {
    fd: libc::c_int,
}

fn as_void_ptr<T>(s: &[T]) -> *const libc::c_void {
    return s.as_ptr() as *const libc::c_void;
}

fn as_mut_void_ptr<T>(s: &mut [T]) -> *mut libc::c_void {
    return s.as_mut_ptr() as *mut libc::c_void;
}

impl File {
    pub fn open(path: &AsRef<Path>, flags: u32, mode: u32) -> io::Result<File> {
        let s = to_cstring(path);
        let fd = unsafe { libc::open(s.as_ptr(), (flags | O_CLOEXEC) as i32, mode) };
        debug!(
            "<-- open {:?} {:b} {:#o} = {}",
            path.as_ref(),
            flags,
            mode,
            fd
        );
        if fd == -1 {
            return Err(io::Error::last_os_error());
        } else {
            return Ok(File { fd: fd });
        }
    }

    pub fn valid(&self) -> bool {
        return self.fd != -1;
    }

    pub fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        let nbytes =
            unsafe { libc::pread(self.fd, as_mut_void_ptr(buf), buf.len(), offset as i64) };
        if nbytes < 0 {
            return Err(io::Error::last_os_error());
        } else {
            return Ok(nbytes as usize);
        }
    }

    pub fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<usize> {
        let nbytes = unsafe { libc::pwrite(self.fd, as_void_ptr(buf), buf.len(), offset as i64) };
        if nbytes < 0 {
            return Err(io::Error::last_os_error());
        } else {
            return Ok(nbytes as usize);
        }
    }

    pub fn flush(&self) -> io::Result<()> {
        // trigger a flush for the underly fd, this could be called
        // multiple times, for ex:
        //
        // int fd2 = dup(fd); close(fd2); close(fd)
        //
        // so the fd needs to stay valid. Note that this means when an
        // application sends close(), kernel will send us
        // flush()/release(), and we will send close()/close(), which
        // will be translated to flush()/flush()/release() to the
        // underlining filesystem
        let fd = unsafe { libc::dup(self.fd) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        } else {
            let res = unsafe { libc::close(fd) };
            if res < 0 {
                return Err(io::Error::last_os_error());
            } else {
                return Ok(());
            }
        }
    }

    pub fn close(&mut self) -> io::Result<()> {
        let res = unsafe { libc::close(self.fd) };
        self.fd = -1;
        if res < 0 {
            return Err(io::Error::last_os_error());
        } else {
            return Ok(());
        }
    }
}

impl Default for File {
    fn default() -> File {
        File { fd: -1 }
    }
}

impl Drop for File {
    fn drop(&mut self) {
        if self.fd != -1 {
            error!(
                "{} dropped but not closed: {}",
                self.fd,
                RError::from(io::Error::from_raw_os_error(libc::EIO))
            );
            if let Err(e) = self.close() {
                error!("!close({}) = {}", self.fd, RError::from(e));
            }
        }
    }
}

impl FileExt for File {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        File::read_at(self, buf, offset)
    }

    fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<usize> {
        File::write_at(self, buf, offset)
    }
}

impl AsRawFd for File {
    fn as_raw_fd(&self) -> RawFd {
        if !self.valid() {
            error!("as_raw_fd called on invalid fd");
        }

        return self.fd;
    }
}

impl XattrFileExt for File {}
