//! File-system helpers: file kinds, sizes, hashing (with a persistent cache),
//! drive list, known folders and a cancellable copy with progress.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
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
    p.to_string_lossy().to_lowercase()
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

const QUICK_CHUNK: u64 = 64 * 1024;

/// Cheap fingerprint: size + first and last 64 KB. Equal full content ⇒ equal quick hash.
pub fn quick_hash(path: &Path, size: u64) -> io::Result<Hash> {
    let mut f = File::open(path)?;
    let mut h = blake3::Hasher::new();
    h.update(&size.to_le_bytes());
    if size <= QUICK_CHUNK * 2 {
        let mut buf = Vec::with_capacity(size as usize);
        f.read_to_end(&mut buf)?;
        h.update(&buf);
    } else {
        let mut buf = vec![0u8; QUICK_CHUNK as usize];
        f.read_exact(&mut buf)?;
        h.update(&buf);
        f.seek(SeekFrom::End(-(QUICK_CHUNK as i64)))?;
        f.read_exact(&mut buf)?;
        h.update(&buf);
    }
    Ok(*h.finalize().as_bytes())
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
    let key = path_key(path);
    let mt = secs(mtime);
    if let Some(e) = cache().lock().unwrap().get(&key) {
        if e.size == size && e.mtime == mt {
            return Ok(e.hash);
        }
    }
    let mut f = File::open(path)?;
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
    if let Some(p) = dst.parent() {
        fs::create_dir_all(p)?;
    }
    let meta = fs::metadata(src)?;
    let tmp = {
        let mut s = dst.as_os_str().to_owned();
        s.push(TMP_SUFFIX);
        PathBuf::from(s)
    };
    let res = (|| -> io::Result<()> {
        let mut r = File::open(src)?;
        let mut w = File::create(&tmp)?;
        let mut buf = vec![0u8; 1 << 20];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            let n = r.read(&mut buf)?;
            if n == 0 {
                break;
            }
            w.write_all(&buf[..n])?;
            bytes.fetch_add(n as u64, Ordering::Relaxed);
        }
        w.flush()?;
        w.sync_all()?;
        drop(w);
        let ft = filetime::FileTime::from_last_modification_time(&meta);
        let _ = filetime::set_file_mtime(&tmp, ft);
        if fs::metadata(&tmp)?.len() != meta.len() {
            return Err(io::Error::other("size mismatch after copy"));
        }
        fs::rename(&tmp, dst)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

/// Move: rename when possible, otherwise copy + delete.
pub fn move_file(src: &Path, dst: &Path, bytes: &AtomicU64, cancel: &AtomicBool) -> io::Result<()> {
    if let Some(p) = dst.parent() {
        fs::create_dir_all(p)?;
    }
    if dst.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "destination exists"));
    }
    match fs::rename(src, dst) {
        Ok(()) => {
            if let Ok(m) = fs::metadata(dst) {
                bytes.fetch_add(m.len(), Ordering::Relaxed);
            }
            Ok(())
        }
        Err(_) => {
            copy_file(src, dst, bytes, cancel)?;
            fs::remove_file(src)
        }
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
