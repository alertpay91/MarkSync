//! File-system helpers: file kinds, sizes, hashing (with a persistent cache),
//! drive list, known folders and a cancellable copy with progress.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Serialize, Deserialize)]
pub enum Kind {
    Image,
    Video,
    Audio,
    Document,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct KindFilter {
    pub images: bool,
    pub videos: bool,
    pub audio: bool,
    pub docs: bool,
    pub other: bool,
}

impl Default for KindFilter {
    fn default() -> Self {
        Self { images: true, videos: true, audio: false, docs: false, other: false }
    }
}

impl KindFilter {
    pub fn accepts(&self, k: Kind) -> bool {
        match k {
            Kind::Image => self.images,
            Kind::Video => self.videos,
            Kind::Audio => self.audio,
            Kind::Document => self.docs,
            Kind::Other => self.other,
        }
    }
    pub fn any(&self) -> bool {
        self.images || self.videos || self.audio || self.docs || self.other
    }
}

fn ext_lower(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

pub fn kind_of(path: &Path) -> Kind {
    match ext_lower(path).as_str() {
        "jpg" | "jpeg" | "jpe" | "png" | "gif" | "bmp" | "webp" | "heic" | "heif" | "tif" | "tiff"
        | "raw" | "cr2" | "cr3" | "nef" | "arw" | "dng" | "orf" | "rw2" | "raf" | "srw" | "psd"
        | "svg" | "avif" | "jxl" | "ico" => Kind::Image,
        "mp4" | "mov" | "avi" | "mkv" | "wmv" | "m4v" | "3gp" | "3g2" | "mts" | "m2ts" | "mpg"
        | "mpeg" | "webm" | "flv" | "vob" | "ts" | "insv" => Kind::Video,
        "mp3" | "wav" | "flac" | "aac" | "m4a" | "ogg" | "opus" | "wma" | "aiff" | "aif" | "amr"
        | "mid" | "midi" => Kind::Audio,
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "rtf" | "odt" | "ods"
        | "odp" | "csv" | "md" | "epub" | "pages" | "numbers" | "key" => Kind::Document,
        _ => Kind::Other,
    }
}

/// Image formats we can decode for thumbnails.
pub fn can_thumb(path: &Path) -> bool {
    matches!(
        ext_lower(path).as_str(),
        "jpg" | "jpeg" | "jpe" | "png" | "gif" | "bmp" | "webp" | "tif" | "tiff"
    )
}

pub const TMP_SUFFIX: &str = ".marksync-tmp";
pub const DUP_FOLDER: &str = "_MarkSync duplikati";

/// System or temporary files that are never worth copying.
pub fn is_junk(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l == "thumbs.db"
        || l == "desktop.ini"
        || l == ".ds_store"
        || l == ".nomedia"
        || l.starts_with("~$")
        || l.starts_with("._")
        || l.ends_with(TMP_SUFFIX)
}

/// Folders we never walk into.
pub fn is_skipped_dir(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l == "$recycle.bin"
        || l == "system volume information"
        || l == "recycler"
        || l == "$windows.~bt"
        || l == "found.000"
        || l == ".thumbnails"
        || l == ".trash"
        || l == DUP_FOLDER.to_ascii_lowercase()
}

pub fn is_hidden(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name.starts_with('.') {
        return true;
    }
    match fs::symlink_metadata(path) {
        // FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM
        Ok(m) => m.file_attributes() & (0x2 | 0x4) != 0,
        Err(_) => false,
    }
}

pub fn secs(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

pub fn mtime_of(meta: &fs::Metadata) -> SystemTime {
    meta.modified().unwrap_or(UNIX_EPOCH)
}

/// Lower-cased path string: Windows paths are case-insensitive.
pub fn path_key(p: &Path) -> String {
    let s = p.to_string_lossy();
    // phone paths are case-sensitive (a.jpg and A.jpg can both exist on Android)
    if s.starts_with('📱') {
        return s.to_string();
    }
    s.to_lowercase()
}

/// "name (2).ext", "name (3).ext"... the first that is free on disk and not in `taken`.
pub fn unique_path(p: &Path, taken: &dyn Fn(&Path) -> bool) -> PathBuf {
    if !p.exists() && !taken(p) {
        return p.to_path_buf();
    }
    let parent = p.parent().unwrap_or(Path::new(""));
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = p.extension().and_then(|s| s.to_str());
    for i in 2..100_000 {
        let name = match ext {
            Some(e) => format!("{stem} ({i}).{e}"),
            None => format!("{stem} ({i})"),
        };
        let c = parent.join(name);
        if !c.exists() && !taken(&c) {
            return c;
        }
    }
    p.to_path_buf()
}

pub fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c })
        .collect();
    let s = s.trim().trim_end_matches('.').trim().to_string();
    if s.is_empty() { "_".into() } else { s }
}

// ---------------------------------------------------------------------------
// Hashing
// ---------------------------------------------------------------------------

pub type Hash = [u8; 32];

/// Bytes read for the quick fingerprint. Only the start of the file, because phones (MTP)
/// can't seek to the end cheaply.
pub const QUICK_CHUNK: usize = 128 * 1024;

pub fn quick_hash_bytes(size: u64, first: &[u8]) -> Hash {
    let mut h = blake3::Hasher::new();
    h.update(&size.to_le_bytes());
    h.update(&first[..first.len().min(QUICK_CHUNK)]);
    *h.finalize().as_bytes()
}

/// Cheap fingerprint: size + first 128 KB. Equal full content ⇒ equal quick hash.
pub fn quick_hash(path: &Path, size: u64) -> io::Result<Hash> {
    let f = File::open(path)?;
    let mut buf = Vec::with_capacity(QUICK_CHUNK);
    f.take(QUICK_CHUNK as u64).read_to_end(&mut buf)?;
    Ok(quick_hash_bytes(size, &buf))
}

struct CacheEntry {
    size: u64,
    mtime: i64,
    hash: Hash,
}

fn cache() -> &'static Mutex<HashMap<String, CacheEntry>> {
    static C: OnceLock<Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(load_cache()))
}

fn cache_file() -> PathBuf {
    crate::settings::data_dir().join("hashes.tsv")
}

fn to_hex(h: &Hash) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(s: &str) -> Option<Hash> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn load_cache() -> HashMap<String, CacheEntry> {
    let mut map = HashMap::new();
    if let Ok(f) = File::open(cache_file()) {
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            let mut it = line.splitn(4, '\t');
            let (Some(sz), Some(mt), Some(h), Some(p)) = (it.next(), it.next(), it.next(), it.next())
            else {
                continue;
            };
            if let (Ok(size), Ok(mtime), Some(hash)) = (sz.parse(), mt.parse(), from_hex(h)) {
                map.insert(p.to_string(), CacheEntry { size, mtime, hash });
            }
        }
    }
    map
}

pub fn save_cache() {
    let map = cache().lock().unwrap();
    let _ = fs::create_dir_all(crate::settings::data_dir());
    let tmp = cache_file().with_extension("tmp");
    if let Ok(f) = File::create(&tmp) {
        let mut w = io::BufWriter::new(f);
        for (p, e) in map.iter() {
            let _ = writeln!(w, "{}\t{}\t{}\t{}", e.size, e.mtime, to_hex(&e.hash), p);
        }
        if w.flush().is_ok() {
            drop(w);
            let _ = fs::rename(&tmp, cache_file());
        }
    }
}

pub fn clear_cache() {
    cache().lock().unwrap().clear();
    let _ = fs::remove_file(cache_file());
}

/// Full-content hash, remembered across runs as long as size and date are unchanged.
pub fn full_hash(path: &Path, size: u64, mtime: SystemTime, cancel: &AtomicBool) -> io::Result<Hash> {
    full_hash_with(path, size, mtime, cancel, || Ok(Box::new(File::open(path)?)))
}

/// Like [`full_hash`] for any source (`key` names it in the cache).
pub fn full_hash_with(
    key: &Path,
    size: u64,
    mtime: SystemTime,
    cancel: &AtomicBool,
    open: impl FnOnce() -> io::Result<Box<dyn Read>>,
) -> io::Result<Hash> {
    let key = path_key(key);
    let mt = secs(mtime);
    if let Some(e) = cache().lock().unwrap().get(&key) {
        if e.size == size && e.mtime == mt {
            return Ok(e.hash);
        }
    }
    let mut f = open()?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    let hash = *h.finalize().as_bytes();
    cache().lock().unwrap().insert(key, CacheEntry { size, mtime: mt, hash });
    Ok(hash)
}

// ---------------------------------------------------------------------------
// Copy with progress
// ---------------------------------------------------------------------------

/// Copies `src` to `dst` through a temporary file, keeps the modification date.
pub fn copy_file(src: &Path, dst: &Path, bytes: &AtomicU64, cancel: &AtomicBool) -> io::Result<()> {
    let meta = fs::metadata(src)?;
    copy_from(Box::new(File::open(src)?), meta.len(), mtime_of(&meta), dst, bytes, cancel)
}

/// Copies from any reader (disk or phone) through a temporary file and sets the date.
pub fn copy_from(
    mut r: Box<dyn Read>,
    len: u64,
    mtime: SystemTime,
    dst: &Path,
    bytes: &AtomicU64,
    cancel: &AtomicBool,
) -> io::Result<()> {
    if let Some(p) = dst.parent() {
        fs::create_dir_all(p)?;
    }
    let tmp = {
        let mut s = dst.as_os_str().to_owned();
        s.push(TMP_SUFFIX);
        PathBuf::from(s)
    };
    let res = (|| -> io::Result<()> {
        let mut w = File::create(&tmp)?;
        let mut h = blake3::Hasher::new();
        let mut buf = vec![0u8; 1 << 20];
        let mut total = 0u64;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            let n = r.read(&mut buf)?;
            if n == 0 {
                break;
            }
            w.write_all(&buf[..n])?;
            h.update(&buf[..n]);
            total += n as u64;
            bytes.fetch_add(n as u64, Ordering::Relaxed);
        }
        w.flush()?;
        w.sync_all()?;
        drop(w);
        if len > 0 && total != len {
            return Err(io::Error::other(crate::i18n::t("err_short_read")));
        }
        // read the written copy back and compare it with what was read from the source
        let written = hash_reader(&mut File::open(&tmp)?, cancel)?;
        let expected = *h.finalize().as_bytes();
        if written != expected {
            return Err(io::Error::other(crate::i18n::t("err_verify")));
        }
        if mtime > UNIX_EPOCH {
            let _ = filetime::set_file_mtime(&tmp, filetime::FileTime::from_system_time(mtime));
        }
        rename_noreplace(&tmp, dst)?;
        if let Ok(md) = fs::metadata(dst) {
            remember_hash(dst, md.len(), mtime_of(&md), expected);
        }
        Ok(())
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

fn hash_reader(r: &mut dyn Read, cancel: &AtomicBool) -> io::Result<Hash> {
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let n = r.read(&mut buf)?;
        if n == 0 {
            return Ok(*h.finalize().as_bytes());
        }
        h.update(&buf[..n]);
    }
}

fn remember_hash(path: &Path, size: u64, mtime: SystemTime, hash: Hash) {
    cache().lock().unwrap().insert(path_key(path), CacheEntry { size, mtime: secs(mtime), hash });
}

/// Windows path for the wide-string APIs, long paths included.
fn win_path(p: &Path) -> Vec<u16> {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let s = abs.to_string_lossy().to_string();
    let s = if s.starts_with(r"\\?\") || s.len() < 248 {
        s
    } else if let Some(unc) = s.strip_prefix(r"\\") {
        format!(r"\\?\UNC\{unc}")
    } else {
        format!(r"\\?\{s}")
    };
    wide(&s)
}

/// Rename that never overwrites: fails if `to` already exists
/// (std::fs::rename on Windows silently replaces the target).
pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};
    let (a, b) = (win_path(from), win_path(to));
    let ok = unsafe { MoveFileExW(a.as_ptr(), b.as_ptr(), MOVEFILE_WRITE_THROUGH) };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// True if both paths are the very same file on disk (e.g. seen through two drive letters).
pub fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
    let id = |p: &Path| -> Option<(u32, u32, u32)> {
        let f = File::open(p).ok()?;
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        let ok = unsafe { GetFileInformationByHandle(f.as_raw_handle() as _, &mut info) };
        (ok != 0).then_some((info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow))
    };
    match (id(a), id(b)) {
        (Some(x), Some(y)) => x == y,
        _ => true, // can't tell: treat as the same so nothing gets deleted
    }
}

/// Before an "extra" copy is removed: the kept file must still exist, be a different file,
/// and have exactly the same content.
pub fn safe_to_remove_extra(extra: &Path, kept: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let (Ok(me), Ok(mk)) = (fs::metadata(extra), fs::metadata(kept)) else {
        return Err(crate::i18n::t("err_kept_missing").into());
    };
    if me.len() != mk.len() || same_file(extra, kept) {
        return Err(crate::i18n::t("err_not_dup").into());
    }
    let he = hash_reader(&mut File::open(extra).map_err(|e| e.to_string())?, cancel).map_err(|e| e.to_string())?;
    let hk = hash_reader(&mut File::open(kept).map_err(|e| e.to_string())?, cancel).map_err(|e| e.to_string())?;
    if he != hk {
        return Err(crate::i18n::t("err_not_dup").into());
    }
    Ok(())
}

/// Move: rename when possible (same disk), otherwise a verified copy and then delete.
pub fn move_file(src: &Path, dst: &Path, bytes: &AtomicU64, cancel: &AtomicBool) -> io::Result<()> {
    if let Some(p) = dst.parent() {
        fs::create_dir_all(p)?;
    }
    if dst.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "destination exists"));
    }
    match rename_noreplace(src, dst) {
        Ok(()) => {
            if let Ok(m) = fs::metadata(dst) {
                bytes.fetch_add(m.len(), Ordering::Relaxed);
            }
            Ok(())
        }
        // ERROR_NOT_SAME_DEVICE: only then copy + delete; every other error is reported
        Err(e) if e.raw_os_error() == Some(17) => {
            copy_file(src, dst, bytes, cancel)?;
            fs::remove_file(src)
        }
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// History (what was done, in plain text, for people who forget)
// ---------------------------------------------------------------------------

pub fn history_dir() -> PathBuf {
    crate::settings::data_dir().join("Istorija")
}

pub fn log(action: &str, from: &Path, to: Option<&Path>) {
    let now = chrono::Local::now();
    let _ = fs::create_dir_all(history_dir());
    let file = history_dir().join(format!("{}.txt", now.format("%Y-%m")));
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(file) {
        let line = match to {
            Some(t) => format!("{}  {}  {}  →  {}\r\n", now.format("%d.%m.%Y %H:%M:%S"), action, from.display(), t.display()),
            None => format!("{}  {}  {}\r\n", now.format("%d.%m.%Y %H:%M:%S"), action, from.display()),
        };
        let _ = f.write_all(line.as_bytes());
    }
}

/// Removes now-empty folders starting from `dirs`, walking up but never above `stop`.
pub fn remove_empty_dirs(mut dirs: Vec<PathBuf>, stop: &Path) {
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    dirs.dedup();
    for d in dirs {
        let mut cur = Some(d.as_path());
        while let Some(c) = cur {
            if c == stop || !c.starts_with(stop) {
                break;
            }
            // only succeeds if empty (a leftover Thumbs.db/desktop.ini is removed first)
            if let Ok(rd) = fs::read_dir(c) {
                let entries: Vec<_> = rd.flatten().collect();
                if entries.iter().all(|e| {
                    e.file_type().map(|t| t.is_file()).unwrap_or(false)
                        && is_junk(&e.file_name().to_string_lossy())
                }) {
                    for e in entries {
                        let _ = fs::remove_file(e.path());
                    }
                }
            }
            if fs::remove_dir(c).is_err() {
                break;
            }
            cur = c.parent();
        }
    }
}

// ---------------------------------------------------------------------------
// Drives & known folders
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Drive {
    pub root: PathBuf,
    pub label: String,
    pub total: u64,
    pub free: u64,
    pub removable: bool,
    pub system: bool,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn init_error_mode() {
    use windows_sys::Win32::System::Diagnostics::Debug::{SetErrorMode, SEM_FAILCRITICALERRORS};
    unsafe {
        SetErrorMode(SEM_FAILCRITICALERRORS);
    }
}

pub fn list_drives() -> Vec<Drive> {
    use windows_sys::Win32::Storage::FileSystem::*;
    let sys = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()).to_ascii_uppercase();
    let mut out = Vec::new();
    let mask = unsafe { GetLogicalDrives() };
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = format!("{letter}:\\");
        let w = wide(&root);
        let ty = unsafe { GetDriveTypeW(w.as_ptr()) };
        // 2 removable, 3 fixed, 4 network; skip CD-ROM (5), RAM disk, unknown
        if !(ty == 2 || ty == 3 || ty == 4) {
            continue;
        }
        let mut name = [0u16; 261];
        let ok = unsafe {
            GetVolumeInformationW(
                w.as_ptr(),
                name.as_mut_ptr(),
                name.len() as u32,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
            )
        };
        if ok == 0 {
            continue; // empty card reader etc.
        }
        let len = name.iter().position(|&c| c == 0).unwrap_or(0);
        let label = String::from_utf16_lossy(&name[..len]);
        let (mut free, mut total, mut tfree) = (0u64, 0u64, 0u64);
        unsafe {
            GetDiskFreeSpaceExW(w.as_ptr(), &mut free, &mut total, &mut tfree);
        }
        out.push(Drive {
            system: format!("{letter}:") == sys,
            root: PathBuf::from(root),
            label,
            total,
            free,
            removable: ty == 2,
        });
    }
    out
}

pub fn free_space(path: &Path) -> Option<u64> {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let mut p = path;
    while !p.exists() {
        p = p.parent()?;
    }
    let w = wide(&p.to_string_lossy());
    let (mut free, mut total, mut tfree) = (0u64, 0u64, 0u64);
    let ok = unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut free, &mut total, &mut tfree) };
    (ok != 0).then_some(free)
}

fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(a) = rest.find('%') {
        out.push_str(&rest[..a]);
        let after = &rest[a + 1..];
        if let Some(b) = after.find('%') {
            let var = &after[..b];
            match std::env::var(var) {
                Ok(v) => out.push_str(&v),
                Err(_) => {
                    out.push('%');
                    out.push_str(var);
                    out.push('%');
                }
            }
            rest = &after[b + 1..];
        } else {
            out.push_str(&rest[a..]);
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

#[derive(Clone, Copy)]
pub enum Known {
    Pictures,
    Videos,
    Desktop,
    Downloads,
    Documents,
}

pub fn known_folder(k: Known) -> Option<PathBuf> {
    let value = match k {
        Known::Pictures => "My Pictures",
        Known::Videos => "My Video",
        Known::Desktop => "Desktop",
        Known::Downloads => "{374DE290-123F-4565-9164-B4428DB58BC0}",
        Known::Documents => "Personal",
    };
    let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders")
        .ok();
    let from_reg = key
        .and_then(|k| k.get_value::<String, _>(value).ok())
        .map(|s| PathBuf::from(expand_env(&s)));
    let p = from_reg.or_else(|| {
        let home = PathBuf::from(std::env::var("USERPROFILE").ok()?);
        Some(home.join(match k {
            Known::Pictures => "Pictures",
            Known::Videos => "Videos",
            Known::Desktop => "Desktop",
            Known::Downloads => "Downloads",
            Known::Documents => "Documents",
        }))
    })?;
    p.is_dir().then_some(p)
}

/// Opens a web link in the default browser.
pub fn open_url(url: &str) {
    let _ = std::process::Command::new("rundll32.exe").arg("url.dll,FileProtocolHandler").arg(url).spawn();
}

pub fn open_in_explorer(path: &Path, select: bool) {
    use std::os::windows::process::CommandExt;
    let mut c = std::process::Command::new("explorer.exe");
    if select {
        c.raw_arg(format!("/select,\"{}\"", path.display()));
    } else {
        c.raw_arg(format!("\"{}\"", path.display()));
    }
    let _ = c.spawn();
}

/// Lists sub-folders (for the folder tree), sorted, without hidden/system ones.
pub fn subdirs(path: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = match fs::read_dir(path) {
        Ok(rd) => rd
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.path())
            .filter(|p| {
                let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                !is_skipped_dir(n) && !is_hidden(p)
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    v.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    v
}
