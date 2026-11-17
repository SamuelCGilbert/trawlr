//! A minimal binding to FSEvents, the macOS service that reports changes to
//! files by path.
//!
//! Each event names one file or folder and carries flags saying, roughly,
//! what happened to it. Events for the same path are often merged, so the
//! flags are a hint: the indexer looks at the disk to see what is true now.
//! Every event has an id that only ever grows; a stream can start from an
//! old id and FSEvents replays what it recorded since then, which is how a
//! restarted daemon catches up.

use std::ffi::{c_char, c_void, CStr};
use std::io;

type CFAllocatorRef = *const c_void;
type CFArrayRef = *const c_void;
type CFStringRef = *const c_void;
type CFUUIDRef = *const c_void;
type CFIndex = isize;
type FSEventStreamRef = *mut c_void;
type DispatchQueue = *mut c_void;

const UTF8: u32 = 0x0800_0100;

pub const SINCE_NOW: u64 = u64::MAX;

const CREATE_NO_DEFER: u32 = 0x02;
const CREATE_WATCH_ROOT: u32 = 0x04;
const CREATE_FILE_EVENTS: u32 = 0x10;

pub const MUST_SCAN_SUBDIRS: u32 = 0x1;
pub const USER_DROPPED: u32 = 0x2;
pub const KERNEL_DROPPED: u32 = 0x4;
pub const IDS_WRAPPED: u32 = 0x8;
pub const HISTORY_DONE: u32 = 0x10;
pub const ROOT_CHANGED: u32 = 0x20;
pub const MOUNT: u32 = 0x40;
pub const UNMOUNT: u32 = 0x80;
pub const ITEM_CREATED: u32 = 0x100;
pub const ITEM_REMOVED: u32 = 0x200;
pub const ITEM_INODE_META_MOD: u32 = 0x400;
pub const ITEM_RENAMED: u32 = 0x800;
pub const ITEM_MODIFIED: u32 = 0x1000;
pub const ITEM_FINDER_INFO_MOD: u32 = 0x2000;
pub const ITEM_CHANGE_OWNER: u32 = 0x4000;
pub const ITEM_XATTR_MOD: u32 = 0x8000;
pub const ITEM_IS_FILE: u32 = 0x10000;
pub const ITEM_IS_DIR: u32 = 0x20000;
pub const ITEM_IS_SYMLINK: u32 = 0x40000;
pub const ITEM_CLONED: u32 = 0x400000;

#[repr(C)]
struct Context {
    version: CFIndex,
    info: *mut c_void,
    retain: *const c_void,
    release: Option<extern "C" fn(*const c_void)>,
    copy_description: *const c_void,
}

type Callback = extern "C" fn(FSEventStreamRef, *mut c_void, usize, *mut c_void, *const u32, *const u64);

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn FSEventStreamCreate(alloc: CFAllocatorRef, cb: Callback, ctx: *const Context, paths: CFArrayRef, since: u64, latency: f64, flags: u32) -> FSEventStreamRef;
    fn FSEventStreamSetDispatchQueue(s: FSEventStreamRef, q: DispatchQueue);
    fn FSEventStreamStart(s: FSEventStreamRef) -> u8;
    fn FSEventStreamFlushSync(s: FSEventStreamRef);
    fn FSEventStreamStop(s: FSEventStreamRef);
    fn FSEventStreamInvalidate(s: FSEventStreamRef);
    fn FSEventStreamRelease(s: FSEventStreamRef);
    fn FSEventsGetCurrentEventId() -> u64;
    fn FSEventsCopyUUIDForDevice(dev: i32) -> CFUUIDRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFTypeArrayCallBacks: [usize; 5];
    fn CFArrayCreate(alloc: CFAllocatorRef, values: *const *const c_void, n: CFIndex, cb: *const c_void) -> CFArrayRef;
    fn CFStringCreateWithBytes(alloc: CFAllocatorRef, bytes: *const u8, n: CFIndex, enc: u32, external: u8) -> CFStringRef;
    fn CFStringGetCString(s: CFStringRef, buf: *mut c_char, size: CFIndex, enc: u32) -> u8;
    fn CFUUIDCreateString(alloc: CFAllocatorRef, uuid: CFUUIDRef) -> CFStringRef;
    fn CFRelease(p: *const c_void);
}

extern "C" {
    fn dispatch_queue_create(label: *const c_char, attr: *const c_void) -> DispatchQueue;
    fn dispatch_release(obj: *mut c_void);
}

pub struct Event {
    pub path: Vec<u8>,
    pub flags: u32,
    pub id: u64,
}

type Sink = Box<dyn Fn(Vec<Event>) + Send + Sync>;

extern "C" fn on_events(_s: FSEventStreamRef, info: *mut c_void, n: usize, paths: *mut c_void, flags: *const u32, ids: *const u64) {
    let sink = unsafe { &*(info as *const Sink) };
    let paths = paths as *const *const c_char;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let p = unsafe { *paths.add(i) };
        if p.is_null() {
            continue;
        }
        let path = unsafe { CStr::from_ptr(p) }.to_bytes().to_vec();
        out.push(Event { path, flags: unsafe { *flags.add(i) }, id: unsafe { *ids.add(i) } });
    }
    sink(out);
}

extern "C" fn release_sink(info: *const c_void) {
    drop(unsafe { Box::from_raw(info as *mut Sink) });
}

/// A running stream. Dropping it delivers what is still pending, then stops.
pub struct Stream {
    s: FSEventStreamRef,
    q: DispatchQueue,
}

// The stream is only started, flushed and stopped, which FSEvents allows
// from any thread when it delivers on a dispatch queue.
unsafe impl Send for Stream {}

impl Stream {
    /// Watch `paths` (absolute) and everything beneath them, delivering
    /// events from `since` on (or [`SINCE_NOW`]) to `sink`, on a thread of
    /// FSEvents' own, gathered for up to `latency` seconds.
    pub fn start(paths: &[Vec<u8>], since: u64, latency: f64, sink: Sink) -> io::Result<Stream> {
        let strs: Vec<CFStringRef> = paths.iter().map(|p| unsafe { CFStringCreateWithBytes(std::ptr::null(), p.as_ptr(), p.len() as CFIndex, UTF8, 0) }).collect();
        if strs.iter().any(|s| s.is_null()) {
            for s in strs.into_iter().filter(|s| !s.is_null()) {
                unsafe { CFRelease(s) };
            }
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "a path to watch is not valid UTF-8"));
        }
        let array = unsafe { CFArrayCreate(std::ptr::null(), strs.as_ptr(), strs.len() as CFIndex, std::ptr::addr_of!(kCFTypeArrayCallBacks).cast()) };
        for s in &strs {
            unsafe { CFRelease(*s) };
        }
        if array.is_null() {
            return Err(io::Error::other("CFArrayCreate failed"));
        }
        let info = Box::into_raw(Box::new(sink)) as *mut c_void;
        let ctx = Context { version: 0, info, retain: std::ptr::null(), release: Some(release_sink), copy_description: std::ptr::null() };
        let flags = CREATE_FILE_EVENTS | CREATE_NO_DEFER | CREATE_WATCH_ROOT;
        let s = unsafe { FSEventStreamCreate(std::ptr::null(), on_events, &ctx, array, since, latency, flags) };
        unsafe { CFRelease(array) };
        if s.is_null() {
            release_sink(info);
            return Err(io::Error::other("FSEventStreamCreate failed"));
        }
        let q = unsafe { dispatch_queue_create(c"io.trawlr.fsevents".as_ptr(), std::ptr::null()) };
        unsafe { FSEventStreamSetDispatchQueue(s, q) };
        if unsafe { FSEventStreamStart(s) } == 0 {
            unsafe {
                FSEventStreamInvalidate(s);
                FSEventStreamRelease(s);
                dispatch_release(q);
            }
            return Err(io::Error::other("FSEventStreamStart failed"));
        }
        Ok(Stream { s, q })
    }

    /// Deliver every event that has happened so far before returning.
    pub fn flush(&self) {
        unsafe { FSEventStreamFlushSync(self.s) };
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        unsafe {
            FSEventStreamFlushSync(self.s);
            FSEventStreamStop(self.s);
            FSEventStreamInvalidate(self.s);
            FSEventStreamRelease(self.s);
            dispatch_release(self.q);
        }
    }
}

/// The id the next event will have.
pub fn current_id() -> u64 {
    unsafe { FSEventsGetCurrentEventId() }
}

/// The identity FSEvents gives the volume `dev` is on; it changes when the
/// volume is erased or its event history is reset.
pub fn volume_uuid(dev: i32) -> Option<String> {
    let u = unsafe { FSEventsCopyUUIDForDevice(dev) };
    if u.is_null() {
        return None;
    }
    let s = unsafe { CFUUIDCreateString(std::ptr::null(), u) };
    unsafe { CFRelease(u) };
    if s.is_null() {
        return None;
    }
    let mut buf = [0 as c_char; 64];
    let ok = unsafe { CFStringGetCString(s, buf.as_mut_ptr(), buf.len() as CFIndex, UTF8) };
    unsafe { CFRelease(s) };
    (ok != 0).then(|| unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned())
}
