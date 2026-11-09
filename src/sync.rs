//! Compare source folders with a target (external disk) by content, then copy.

use crate::fsutil::{self, Hash, Kind, KindFilter};
use crate::i18n::t;
use crate::job::{Progress, Report};
use crate::i18n::tf;
use crate::meta;
use crate::organize::{self, Scheme};
use crate::phone::{self, PhoneRef, PhoneSource};
use std::sync::Arc;
use crate::settings::LayoutChoice;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Status {
    New,
    Newer,
    Conflict,
    Exists,
    DupInSource,
}

impl Status {
    pub const ALL: [Status; 5] = [Status::New, Status::Newer, Status::Conflict, Status::Exists, Status::DupInSource];
    pub fn label(self) -> &'static str {
        match self {
            Status::New => t("st_new"),
            Status::Newer => t("st_newer"),
            Status::Conflict => t("st_conflict"),
            Status::Exists => t("st_exists"),
            Status::DupInSource => t("st_dup_src"),
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            Status::New => t("st_new_hint"),
            Status::Newer => t("st_newer_hint"),
            Status::Conflict => t("st_conflict_hint"),
            Status::Exists => t("st_exists_hint"),
            Status::DupInSource => t("st_dup_src_hint"),
        }
    }
    pub fn copyable(self) -> bool {
        matches!(self, Status::New | Status::Newer | Status::Conflict)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Copy,
    Replace,
    KeepBoth,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Copy => t("act_copy"),
            Action::Replace => t("act_replace"),
            Action::KeepBoth => t("act_keep_both"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    pub src: PathBuf,
    pub dest: PathBuf,
    pub size: u64,
    pub mtime: SystemTime,
    pub kind: Kind,
    pub status: Status,
    pub action: Action,
    pub selected: bool,
    /// Where the identical copy already is (Exists / DupInSource).
    pub same_as: Option<PathBuf>,
    pub dest_mtime: Option<SystemTime>,
    /// Name changed because another file wanted the same name.
    pub renamed: bool,
    /// Set when the file is on a phone (then `src` is only a display path).
    pub phone: Option<PhoneRef>,
}

#[derive(Clone)]
pub struct Source {
    pub root: PathBuf,
    pub states: HashMap<PathBuf, bool>,
}

impl Source {
    pub fn included(&self, dir: &Path) -> bool {
        let mut cur = Some(dir);
        while let Some(c) = cur {
            if let Some(&s) = self.states.get(c) {
                return s;
            }
            if c == self.root {
                break;
            }
            cur = c.parent();
        }
        true
    }
    /// True if no folder below `dir` can be included (lets the walker skip it).
    fn fully_excluded(&self, dir: &Path) -> bool {
        !self.included(dir) && !self.states.iter().any(|(k, &v)| v && k.starts_with(dir) && k != dir)
    }
}

pub struct CompareInput {
    pub sources: Vec<Source>,
    pub phones: Vec<PhoneSource>,
    pub target: PathBuf,
    pub kinds: KindFilter,
    pub layout: LayoutChoice,
    pub scheme: Scheme,
    pub skip_hidden: bool,
    /// albums / "check the date" for the organized layout
    pub org_opts: organize::OrgOptions,
}

struct SrcFile {
    path: PathBuf,
    rel: PathBuf,
    size: u64,
    mtime: SystemTime,
    kind: Kind,
    phone: Option<PhoneRef>,
}

/// Quick fingerprint for files on disk. Phone files skip it (see `compare`).
fn src_quick(f: &SrcFile) -> std::io::Result<Hash> {
    fsutil::quick_hash(&f.path, f.size)
}

fn src_full(f: &SrcFile, cancel: &std::sync::atomic::AtomicBool) -> std::io::Result<Hash> {
    match &f.phone {
        Some(r) => fsutil::full_hash_with(&f.path, f.size, f.mtime, cancel, || Ok(Box::new(phone::open(r)?))),
        None => fsutil::full_hash(&f.path, f.size, f.mtime, cancel),
    }
}

fn src_meta(f: &SrcFile, scheme: Scheme) -> meta::Meta {
    match &f.phone {
        Some(r) => {
            // date-only layouts: the name (PXL_20250905_…) is enough, no need to read the file
            if matches!(scheme, Scheme::YearMonth | Scheme::YearMonthDay | Scheme::Year) {
                let name = f.path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if let Some(d) = meta::date_from_name(name) {
                    return meta::Meta { date: Some(d), date_from: meta::DateFrom::Name, ..Default::default() };
                }
            }
            let head = if f.kind == Kind::Image { phone::read_prefix(r, 256 * 1024).unwrap_or_default() } else { Vec::new() };
            let name = f.path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            meta::read_meta_bytes(name, &head, f.kind, f.mtime)
        }
        None => meta::read_meta(&f.path, f.kind, f.mtime),
    }
}

fn naive_to_system(d: chrono::NaiveDateTime) -> Option<SystemTime> {
    use chrono::TimeZone;
    chrono::Local.from_local_datetime(&d).earliest().map(Into::into)
}

/// Adds the files of a phone folder.
fn scan_phone(ps: &PhoneSource, multi: bool, inp: &CompareInput, p: &Progress, src: &mut Vec<SrcFile>) -> Result<(), String> {
    p.stage("st_scan_phone", 0);
    p.set_current(ps.display());
    let dev = phone::device_id_by_name(&ps.device).ok_or_else(|| tf("err_no_phone", &[&ps.device]))?;
    let folder = phone::resolve(&dev, &ps.path).map_err(|e| tf("err_phone_folder", &[&ps.display(), &e.to_string()]))?;
    let dev = Arc::new(dev);
    let vroot = ps.virtual_root();
    let root_name = PathBuf::from(ps.path.last().cloned().unwrap_or_else(|| ps.device.clone()));
    let res = phone::walk(&dev, &folder, &p.cancel, &mut |rel, e| {
        if fsutil::is_junk(&e.name) {
            return;
        }
        let path = vroot.join(&rel);
        let kind = fsutil::kind_of(&path);
        if !inp.kinds.accepts(kind) {
            return;
        }
        let mtime = e
            .mtime
            .or_else(|| meta::date_from_name(&e.name).and_then(naive_to_system))
            .unwrap_or(std::time::UNIX_EPOCH);
        let rel = if multi { root_name.join(&rel) } else { rel };
        if src.len() % 50 == 0 {
            p.set_current(path.to_string_lossy());
        }
        src.push(SrcFile { path, rel, size: e.size, mtime, kind, phone: Some(PhoneRef { dev: dev.clone(), obj: e.obj }) });
        p.tick();
    });
    match res {
        Ok(0) => Ok(()),
        Ok(n) => {
            p.warn(tf("warn_phone_skipped", &[&n.to_string(), &ps.display()]));
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => Err(t("cancelled").into()),
        Err(e) => Err(tf("err_phone_read", &[&e.to_string()])),
    }
}

struct TgtFile {
    path: PathBuf,
    size: u64,
    mtime: SystemTime,
}

pub fn compare(inp: &CompareInput, p: &Progress) -> Result<Vec<Item>, String> {
    if !inp.target.exists() {
        std::fs::create_dir_all(&inp.target).map_err(|e| e.to_string())?;
    }
    let multi = inp.sources.len() + inp.phones.len() > 1;
    let canon = |p: &Path| fsutil::path_key(&std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()));
    let target_key = canon(&inp.target);
    if inp.sources.iter().any(|s| canon(&s.root) == target_key) {
        return Err(t("err_same_folder").into());
    }

    // 1. source files
    p.stage("st_scan_src", 0);
    let mut src: Vec<SrcFile> = Vec::new();
    let mut seen_paths: HashSet<String> = HashSet::new();
    for s in &inp.sources {
        let target = inp.target.clone();
        let walker = walkdir::WalkDir::new(&s.root).into_iter().filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            let n = e.file_name().to_string_lossy();
            if e.file_type().is_dir() {
                if fsutil::is_skipped_dir(&n) || fsutil::path_key(e.path()) == fsutil::path_key(&target) || s.fully_excluded(e.path()) {
                    return false;
                }
            }
            !(inp.skip_hidden && fsutil::is_hidden(e.path()))
        });
        let root_name = s.root.file_name().map(PathBuf::from).unwrap_or_else(|| {
            // a drive root like E:\ → "E"
            PathBuf::from(s.root.to_string_lossy().trim_end_matches(['\\', ':']).to_string())
        });
        for e in walker {
            if p.cancelled() {
                return Err(t("cancelled").into());
            }
            let e = match e {
                Ok(e) => e,
                Err(err) => {
                    p.warn(format!("{}: {err}", err.path().map(|x| x.display().to_string()).unwrap_or_default()));
                    continue;
                }
            };
            if !e.file_type().is_file() || fsutil::is_junk(&e.file_name().to_string_lossy()) {
                continue;
            }
            let path = e.path();
            if !s.included(path.parent().unwrap_or(path)) {
                continue;
            }
            let kind = fsutil::kind_of(path);
            if !inp.kinds.accepts(kind) {
                continue;
            }
            // the same folder may be ticked twice (overlapping sources)
            if !seen_paths.insert(fsutil::path_key(path)) {
                continue;
            }
            let md = match e.metadata() {
                Ok(m) => m,
                Err(err) => {
                    p.warn(format!("{}: {err}", path.display()));
                    continue;
                }
            };
            let inner = path.strip_prefix(&s.root).unwrap_or(path).to_path_buf();
            let rel = if multi { root_name.join(inner) } else { inner };
            src.push(SrcFile { path: path.to_path_buf(), rel, size: md.len(), mtime: fsutil::mtime_of(&md), kind, phone: None });
            p.tick();
            if src.len() % 200 == 0 {
                p.set_current(path.to_string_lossy());
            }
        }
    }
    for ps in &inp.phones {
        scan_phone(ps, multi, inp, p, &mut src)?;
    }

    // 2. target files (the whole target folder, any sub-folder)
    p.stage("st_scan_tgt", 0);
    let src_roots: Vec<PathBuf> = inp.sources.iter().map(|s| s.root.clone()).collect();
    let mut tgt: Vec<TgtFile> = Vec::new();
    let walker = walkdir::WalkDir::new(&inp.target).into_iter().filter_entry(|e| {
        if e.depth() == 0 {
            return true;
        }
        let n = e.file_name().to_string_lossy();
        if e.file_type().is_dir()
            && (fsutil::is_skipped_dir(&n) || src_roots.iter().any(|r| fsutil::path_key(r) == fsutil::path_key(e.path())))
        {
            return false;
        }
        true
    });
    for e in walker {
        if p.cancelled() {
            return Err(t("cancelled").into());
        }
        let e = match e {
            Ok(e) => e,
            Err(err) => {
                p.warn(format!("{}: {err}", err.path().map(|x| x.display().to_string()).unwrap_or_default()));
                continue;
            }
        };
        if !e.file_type().is_file() || fsutil::is_junk(&e.file_name().to_string_lossy()) {
            continue;
        }
        if !inp.kinds.accepts(fsutil::kind_of(e.path())) {
            continue;
        }
        let Ok(md) = e.metadata() else {
            p.warn(e.path().display().to_string());
            continue;
        };
        tgt.push(TgtFile { path: e.path().to_path_buf(), size: md.len(), mtime: fsutil::mtime_of(&md) });
        p.tick();
        if tgt.len() % 200 == 0 {
            p.set_current(e.path().to_string_lossy());
        }
    }

    // 3. fingerprints, only where sizes collide
    let mut size_count: HashMap<u64, (u32, u32)> = HashMap::new(); // (in source, in target)
    for f in &src {
        size_count.entry(f.size).or_default().0 += 1;
    }
    for f in &tgt {
        if let Some(c) = size_count.get_mut(&f.size) {
            c.1 += 1;
        }
    }
    let needs = |size: u64| size_count.get(&size).map(|&(s, t)| t > 0 || s > 1).unwrap_or(false);

    // (is_source, index)
    let mut todo: Vec<(bool, usize)> = Vec::new();
    for (i, f) in src.iter().enumerate() {
        if needs(f.size) {
            todo.push((true, i));
        }
    }
    for (i, f) in tgt.iter().enumerate() {
        if needs(f.size) {
            todo.push((false, i));
        }
    }
    p.stage("st_quick_hash", todo.len() as u64);
    // A phone that can't be read must stop the comparison: otherwise files that are
    // already on the disk would wrongly show up as new.
    let phone_err: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    let phone_failed = |p: &Progress| -> Option<String> {
        let e = phone_err.lock().unwrap().clone()?;
        if p.cancelled() {
            return Some(t("cancelled").into());
        }
        Some(tf("err_phone_lost", &[&e]))
    };
    let info = |(is_src, i): (bool, usize)| -> (&Path, u64, SystemTime) {
        if is_src {
            (&src[i].path, src[i].size, src[i].mtime)
        } else {
            (&tgt[i].path, tgt[i].size, tgt[i].mtime)
        }
    };
    let quick: Vec<Option<Hash>> = todo
        .par_iter()
        .map(|&k| {
            if p.cancelled() {
                return None;
            }
            let r = if k.0 {
                let f = &src[k.1];
                if f.phone.is_some() {
                    None // phones: no partial reads, straight to the full check
                } else {
                    src_quick(f).ok()
                }
            } else {
                let (path, size, _) = info(k);
                fsutil::quick_hash(path, size).ok()
            };
            p.tick();
            r
        })
        .collect();
    if p.cancelled() {
        return Err(t("cancelled").into());
    }
    if let Some(e) = phone_failed(p) {
        return Err(e);
    }
    // groups of equal (size, quick) that contain a source file and something else
    let mut qgroups: HashMap<(u64, Hash), (u32, u32)> = HashMap::new();
    for (j, &k) in todo.iter().enumerate() {
        if let Some(q) = quick[j] {
            let e = qgroups.entry((info(k).1, q)).or_default();
            if k.0 { e.0 += 1 } else { e.1 += 1 }
        }
    }
    // phone files are compared in full with every disk file of the same size
    let phone_sizes: HashSet<u64> = src.iter().filter(|f| f.phone.is_some()).map(|f| f.size).collect();
    let full_todo: Vec<usize> = (0..todo.len())
        .filter(|&j| {
            let (is_src, i) = todo[j];
            if is_src && src[i].phone.is_some() {
                return true; // already known: same size as something else
            }
            if !is_src && phone_sizes.contains(&tgt[i].size) {
                return true;
            }
            quick[j]
                .and_then(|q| qgroups.get(&(info(todo[j]).1, q)))
                .map(|&(s, t)| s > 0 && (t > 0 || s > 1))
                .unwrap_or(false)
        })
        .collect();
    let full_bytes: u64 = full_todo.iter().map(|&j| info(todo[j]).1).sum();
    p.stage("st_full_hash", full_todo.len() as u64);
    {
        // explain the numbers: both sides of every pair are read
        let (mut ns, mut bs, mut nt, mut bt) = (0u64, 0u64, 0u64, 0u64);
        for &j in &full_todo {
            let size = info(todo[j]).1;
            if todo[j].0 {
                ns += 1;
                bs += size;
            } else {
                nt += 1;
                bt += size;
            }
        }
        p.set_detail(tf(
            "full_detail",
            &[&crate::i18n::fmt_num(ns), &crate::i18n::fmt_size(bs), &crate::i18n::fmt_num(nt), &crate::i18n::fmt_size(bt)],
        ));
    }
    p.bytes.store(0, Ordering::Relaxed);
    p.bytes_total.store(full_bytes, Ordering::Relaxed);
    let full: Vec<(usize, Option<Hash>)> = full_todo
        .par_iter()
        .map(|&j| {
            if p.cancelled() {
                return (j, None);
            }
            let (path, size, mtime) = info(todo[j]);
            p.set_current(path.to_string_lossy());
            let r = if todo[j].0 {
                let f = &src[todo[j].1];
                match src_full(f, &p.cancel) {
                    Ok(h) => Some(h),
                    Err(e) => {
                        if f.phone.is_some() {
                            phone_err.lock().unwrap().get_or_insert(e.to_string());
                        }
                        None
                    }
                }
            } else {
                fsutil::full_hash(path, size, mtime, &p.cancel).ok()
            };
            p.bytes.fetch_add(size, Ordering::Relaxed);
            p.tick();
            (j, r)
        })
        .collect();
    p.bytes_total.store(0, Ordering::Relaxed);
    fsutil::save_cache();
    if p.cancelled() {
        return Err(t("cancelled").into());
    }
    if let Some(e) = phone_failed(p) {
        return Err(e);
    }
    let mut src_hash: HashMap<usize, Hash> = HashMap::new();
    let mut tgt_by_hash: HashMap<(u64, Hash), usize> = HashMap::new();
    for (j, h) in full {
        if let Some(h) = h {
            let (is_src, i) = todo[j];
            if is_src {
                src_hash.insert(i, h);
            } else {
                tgt_by_hash.entry((tgt[i].size, h)).or_insert(i);
            }
        }
    }

    // 4. classify
    p.stage("st_classify", src.len() as u64);
    let mut status: Vec<Option<(Status, Option<PathBuf>)>> = vec![None; src.len()];
    let mut first_src: HashMap<(u64, Hash), usize> = HashMap::new();
    for (i, f) in src.iter().enumerate() {
        if let Some(h) = src_hash.get(&i) {
            let key = (f.size, *h);
            let on_disk = tgt_by_hash.get(&key).filter(|&&ti| {
                // never "already on disk" because a file matched itself (overlapping folders,
                // the same disk under two drive letters, a network path to the same share)
                f.phone.is_some() || !fsutil::same_file(&f.path, &tgt[ti].path)
            });
            if let Some(&ti) = on_disk {
                status[i] = Some((Status::Exists, Some(tgt[ti].path.clone())));
            } else if let Some(&si) = first_src.get(&key) {
                status[i] = Some((Status::DupInSource, Some(src[si].path.clone())));
            } else {
                first_src.insert(key, i);
            }
        }
    }

    // destination folders (metadata only for files that will be copied)
    let need_folder: Vec<usize> = (0..src.len()).filter(|&i| status[i].is_none()).collect();
    let scheme = inp.scheme;
    let organized = inp.layout == LayoutChoice::Organized;
    if organized {
        p.stage("st_reading_meta", need_folder.len() as u64);
    }
    let rel_dirs: Vec<(usize, PathBuf)> = need_folder
        .par_iter()
        .map(|&i| {
            let f = &src[i];
            let dir = match inp.layout {
                LayoutChoice::Keep => f.rel.parent().map(Path::to_path_buf).unwrap_or_default(),
                LayoutChoice::Flat => PathBuf::new(),
                LayoutChoice::Organized => {
                    let m = if scheme.needs_meta() {
                        src_meta(f, scheme)
                    } else {
                        meta::Meta::default()
                    };
                    p.tick();
                    let album = f.rel.parent().and_then(organize::album_in);
                    organize::folder_with(scheme, &f.path, f.kind, &m, album.as_deref(), inp.org_opts).0
                }
            };
            (i, dir)
        })
        .collect();

    let mut taken: HashSet<String> = HashSet::new();
    let mut items: Vec<Item> = Vec::with_capacity(src.len());
    let mut dest_of: HashMap<usize, (PathBuf, bool)> = HashMap::new();
    for (i, dir) in rel_dirs {
        let f = &src[i];
        let want = inp.target.join(dir).join(f.path.file_name().unwrap_or_default());
        let key = fsutil::path_key(&want);
        if taken.contains(&key) {
            // two different source files want the same name
            let to = fsutil::unique_path(&want, &|c| taken.contains(&fsutil::path_key(c)));
            taken.insert(fsutil::path_key(&to));
            dest_of.insert(i, (to, true));
        } else {
            taken.insert(key);
            dest_of.insert(i, (want, false));
        }
    }

    for (i, f) in src.iter().enumerate() {
        let base = Item {
            src: f.path.clone(),
            dest: PathBuf::new(),
            size: f.size,
            mtime: f.mtime,
            kind: f.kind,
            status: Status::New,
            action: Action::Copy,
            selected: false,
            same_as: None,
            dest_mtime: None,
            renamed: false,
            phone: f.phone.clone(),
        };
        if let Some((st, same)) = status[i].clone() {
            items.push(Item { status: st, same_as: same.clone(), dest: same.unwrap_or_default(), ..base });
            continue;
        }
        let (dest, renamed) = dest_of.remove(&i).unwrap_or_default();
        let mut it = Item { dest: dest.clone(), renamed, selected: true, ..base };
        if let Ok(md) = std::fs::metadata(&dest) {
            let dm = fsutil::mtime_of(&md);
            it.dest_mtime = Some(dm);
            let same_content = md.len() == f.size && {
                let src_h = src_full(f, &p.cancel);
                if p.cancelled() {
                    return Err(t("cancelled").into());
                }
                if f.phone.is_some() {
                    if let Err(e) = &src_h {
                        return Err(tf("err_phone_lost", &[&e.to_string()]));
                    }
                }
                let dst_h = fsutil::full_hash(&dest, md.len(), dm, &p.cancel);
                // two failed reads are NOT "the same": only two real, equal fingerprints are
                matches!((&src_h, &dst_h), (Ok(a), Ok(b)) if a == b)
                    && (f.phone.is_some() || !fsutil::same_file(&f.path, &dest))
            };
            if p.cancelled() {
                return Err(t("cancelled").into());
            }
            if same_content {
                it.status = Status::Exists;
                it.same_as = Some(dest);
                it.selected = false;
            } else if f.mtime > dm + Duration::from_secs(2) {
                // safest default: keep both; replacing is the user's explicit choice
                it.status = Status::Newer;
                it.action = Action::KeepBoth;
            } else {
                it.status = Status::Conflict;
                it.action = Action::KeepBoth;
            }
        }
        items.push(it);
    }
    fsutil::save_cache();

    items.sort_by(|a, b| {
        let order = |s: Status| Status::ALL.iter().position(|x| *x == s).unwrap_or(9);
        order(a.status).cmp(&order(b.status)).then_with(|| a.src.cmp(&b.src))
    });
    Ok(items)
}

/// Copies the selected items.
pub fn copy(items: &[Item], p: &Progress) -> Report {
    let list: Vec<(usize, &Item)> = items.iter().enumerate().filter(|(_, i)| i.selected && i.status.copyable()).collect();
    let mut rep = Report::default();
    p.stage("st_copying", list.len() as u64);
    p.bytes.store(0, Ordering::Relaxed);
    p.bytes_total.store(list.iter().map(|(_, i)| i.size).sum(), Ordering::Relaxed);
    let mut first_dir: Option<PathBuf> = None;
    for (idx, it) in list {
        if p.cancelled() {
            rep.cancelled = true;
            break;
        }
        p.set_current(it.src.to_string_lossy());
        let res = (|| -> std::io::Result<PathBuf> {
            match it.action {
                Action::Copy | Action::KeepBoth => {
                    let to = fsutil::unique_path(&it.dest, &|_| false);
                    copy_item(it, &to, p)?;
                    Ok(to)
                }
                Action::Replace => {
                    if it.dest.exists() {
                        retire_old(&it.dest)?;
                    }
                    copy_item(it, &it.dest, p)?;
                    Ok(it.dest.clone())
                }
            }
        })();
        match res {
            Ok(to) => {
                fsutil::log(t("log_copied"), &it.src, Some(&to));
                rep.ok_files += 1;
                rep.ok_bytes += it.size;
                rep.done_idx.push(idx);
                rep.done_dest.push(to.clone());
                if first_dir.is_none() {
                    first_dir = to.parent().map(Path::to_path_buf);
                }
            }
            Err(e) => rep.errors.push((it.src.to_string_lossy().into(), e.to_string())),
        }
        p.tick();
    }
    rep.open_path = first_dir;
    rep
}

fn copy_item(it: &Item, to: &Path, p: &Progress) -> std::io::Result<()> {
    match &it.phone {
        Some(r) => {
            // the phone may have been reconnected since the comparison: make sure the
            // object still is the same file (name and size) before copying it
            let e = phone::stat(r)?;
            let name = it.src.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if e.name != name || e.size != it.size {
                return Err(std::io::Error::other(t("err_phone_changed")));
            }
            fsutil::copy_from(Box::new(phone::open(r)?), it.size, it.mtime, to, &p.bytes, &p.cancel)
        }
        None => fsutil::copy_file(&it.src, to, &p.bytes, &p.cancel),
    }
}

/// The old version is never destroyed: it goes to the Recycle Bin,
/// or (on drives without one) is renamed to "name (old version DATE).ext".
fn retire_old(path: &Path) -> std::io::Result<()> {
    if trash::delete(path).is_ok() && !path.exists() {
        fsutil::log(t("log_to_bin"), path, None);
        return Ok(());
    }
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let date = chrono::Local::now().format("%Y-%m-%d");
    let name = match path.extension().and_then(|s| s.to_str()) {
        Some(e) => format!("{stem} ({} {date}).{e}", t("old_version")),
        None => format!("{stem} ({} {date})", t("old_version")),
    };
    let to = fsutil::unique_path(&path.with_file_name(name), &|_| false);
    fsutil::rename_noreplace(path, &to)?;
    fsutil::log(t("log_renamed"), path, Some(&to));
    Ok(())
}
