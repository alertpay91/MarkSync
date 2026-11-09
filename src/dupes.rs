//! Finds identical files (by content) inside one or more folders.

use crate::fsutil::{self, Hash, Kind, KindFilter};
use crate::i18n::t;
use crate::job::{Progress, Report};
use crate::settings::DupAction;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::SystemTime;

#[derive(Clone, Debug)]
pub struct DupFile {
    pub path: PathBuf,
    pub mtime: SystemTime,
}

#[derive(Clone, Debug)]
pub struct DupGroup {
    pub size: u64,
    pub kind: Kind,
    pub files: Vec<DupFile>,
    pub keep: usize,
}

impl DupGroup {
    pub fn wasted(&self) -> u64 {
        self.size * (self.files.len() as u64 - 1)
    }
}

/// Groups of identical files (indices into `files`), compared by size, then the first
/// 128 KB, then the full content.
pub fn identical_groups(files: &[(PathBuf, u64, SystemTime)], p: &Progress) -> Result<Vec<Vec<usize>>, String> {
    let mut by_size: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, f) in files.iter().enumerate().filter(|(_, f)| f.1 > 0) {
        by_size.entry(f.1).or_default().push(i);
    }
    let cand: Vec<usize> = by_size.values().filter(|v| v.len() > 1).flatten().copied().collect();
    p.stage("st_quick_hash", cand.len() as u64);
    let quick: Vec<(usize, Option<Hash>)> = cand
        .par_iter()
        .map(|&i| {
            let r = if p.cancelled() { None } else { fsutil::quick_hash(&files[i].0, files[i].1).ok() };
            p.tick();
            (i, r)
        })
        .collect();
    if p.cancelled() {
        return Err(t("cancelled").into());
    }
    let mut by_quick: HashMap<(u64, Hash), Vec<usize>> = HashMap::new();
    for (i, q) in quick {
        if let Some(q) = q {
            by_quick.entry((files[i].1, q)).or_default().push(i);
        }
    }
    let cand2: Vec<usize> = by_quick.values().filter(|v| v.len() > 1).flatten().copied().collect();
    p.stage("st_full_hash", cand2.len() as u64);
    p.bytes.store(0, Ordering::Relaxed);
    p.bytes_total.store(cand2.iter().map(|&i| files[i].1).sum(), Ordering::Relaxed);
    let full: Vec<(usize, Option<Hash>)> = cand2
        .par_iter()
        .map(|&i| {
            let (path, size, mtime) = &files[i];
            p.set_current(path.to_string_lossy());
            let r = if p.cancelled() { None } else { fsutil::full_hash(path, *size, *mtime, &p.cancel).ok() };
            p.bytes.fetch_add(*size, Ordering::Relaxed);
            p.tick();
            (i, r)
        })
        .collect();
    p.bytes_total.store(0, Ordering::Relaxed);
    fsutil::save_cache();
    if p.cancelled() {
        return Err(t("cancelled").into());
    }
    let mut groups: HashMap<(u64, Hash), Vec<usize>> = HashMap::new();
    for (i, h) in full {
        if let Some(h) = h {
            groups.entry((files[i].1, h)).or_default().push(i);
        }
    }
    Ok(groups.into_values().filter(|v| v.len() > 1).collect())
}

/// Which of two identical files is "the original": no copy markers, older, shorter path.
pub fn original_order(a: &Path, am: SystemTime, b: &Path, bm: SystemTime) -> std::cmp::Ordering {
    copy_score(a).cmp(&copy_score(b)).then(am.cmp(&bm)).then(a.as_os_str().len().cmp(&b.as_os_str().len()))
}

/// Lower is a better "original": no copy markers, older, shorter path.
fn copy_score(p: &Path) -> i32 {
    let n = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    let mut s = 0;
    for marker in ["copy", "kopija", "копија", "kopie", "копия", " - ", "(1)", "(2)", "(3)", "_1", "-1"] {
        if n.contains(marker) {
            s += 10;
        }
    }
    s
}

pub fn find(roots: &[PathBuf], kinds: KindFilter, skip_hidden: bool, p: &Progress) -> Result<Vec<DupGroup>, String> {
    p.stage("st_scanning", 0);
    let mut files: Vec<(PathBuf, u64, SystemTime)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for root in roots {
        let walker = walkdir::WalkDir::new(root).into_iter().filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            if e.file_type().is_dir() && fsutil::is_skipped_dir(&e.file_name().to_string_lossy()) {
                return false;
            }
            !(skip_hidden && fsutil::is_hidden(e.path()))
        });
        for e in walker.flatten() {
            if p.cancelled() {
                return Err(t("cancelled").into());
            }
            if !e.file_type().is_file() || fsutil::is_junk(&e.file_name().to_string_lossy()) {
                continue;
            }
            if !kinds.accepts(fsutil::kind_of(e.path())) || !seen.insert(fsutil::path_key(e.path())) {
                continue;
            }
            let Ok(md) = e.metadata() else { continue };
            if md.len() == 0 {
                continue;
            }
            files.push((e.path().to_path_buf(), md.len(), fsutil::mtime_of(&md)));
            p.tick();
            if files.len() % 200 == 0 {
                p.set_current(e.path().to_string_lossy());
            }
        }
    }

    let groups = identical_groups(&files, p)?;
    let mut out: Vec<DupGroup> = groups
        .into_iter()
        .map(|v| {
            let size = files[v[0]].1;
            let mut fl: Vec<DupFile> =
                v.iter().map(|&i| DupFile { path: files[i].0.clone(), mtime: files[i].2 }).collect();
            fl.sort_by(|a, b| original_order(&a.path, a.mtime, &b.path, b.mtime));
            let kind = fsutil::kind_of(&fl[0].path);
            DupGroup { size, kind, files: fl, keep: 0 }
        })
        .collect();
    out.sort_by(|a, b| b.wasted().cmp(&a.wasted()));
    Ok(out)
}

/// Removes every file except the kept one in each group: to the Recycle Bin or a side folder.
pub fn remove_extras(groups: &[DupGroup], roots: &[PathBuf], action: DupAction, p: &Progress) -> Report {
    let list: Vec<(&Path, u64, &Path)> = groups
        .iter()
        .flat_map(|g| {
            let kept = g.files[g.keep].path.as_path();
            g.files.iter().enumerate().filter(move |(i, _)| *i != g.keep).map(move |(_, f)| (f.path.as_path(), g.size, kept))
        })
        .collect();
    let mut rep = Report::default();
    p.stage("st_removing", list.len() as u64);
    for (path, size, kept) in list {
        if p.cancelled() {
            rep.cancelled = true;
            break;
        }
        p.set_current(path.to_string_lossy());
        // check again right before removing: the kept copy must exist and be identical
        if let Err(e) = fsutil::safe_to_remove_extra(path, kept, &p.cancel) {
            rep.errors.push((path.to_string_lossy().into(), e));
            p.tick();
            continue;
        }
        let res: Result<(), String> = match action {
            DupAction::RecycleBin => match trash::delete(path) {
                Ok(()) if !path.exists() => {
                    fsutil::log(t("log_dup_bin"), path, Some(kept));
                    Ok(())
                }
                Ok(()) => Err(t("err_trash").into()),
                Err(e) => Err(format!("{} ({e})", t("err_trash"))),
            },
            DupAction::MoveToFolder => {
                let root = roots.iter().find(|r| path.starts_with(r)).cloned().unwrap_or_else(|| {
                    path.parent().map(Path::to_path_buf).unwrap_or_default()
                });
                let rel = path.strip_prefix(&root).unwrap_or(path);
                let to = fsutil::unique_path(&root.join(fsutil::DUP_FOLDER).join(rel), &|_| false);
                rep.open_path.get_or_insert(root.join(fsutil::DUP_FOLDER));
                let r = fsutil::move_file(path, &to, &p.bytes, &p.cancel).map_err(|e| e.to_string());
                if r.is_ok() {
                    fsutil::log(t("log_moved"), path, Some(&to));
                }
                r
            }
        };
        match res {
            Ok(()) => {
                rep.ok_files += 1;
                rep.ok_bytes += size;
                if let Some(par) = path.parent() {
                    let root = roots.iter().find(|r| path.starts_with(r));
                    if let Some(r) = root {
                        fsutil::remove_empty_dirs(vec![par.to_path_buf()], r);
                    }
                }
            }
            Err(e) => rep.errors.push((path.to_string_lossy().into(), e)),
        }
        p.tick();
    }
    rep
}
